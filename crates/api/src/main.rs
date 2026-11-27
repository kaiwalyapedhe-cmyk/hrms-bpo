//! `hrms-api` — on-premises HRMS for call centres.
//!
//! Boot order: env → statutory config → Postgres (migrate) → Redis/MinIO
//! (optional) → metrics ticker + job worker → HTTP/WebSocket server.

mod auth;
mod error;
mod jobs;
mod metrics;
mod pdf;
mod queue;
mod realtime;
mod routes;
mod state;
mod storage;

use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderValue, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use tower_http::compression::CompressionLayer;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tower_http::trace::TraceLayer;

use crate::queue::JobQueue;
use crate::realtime::DashboardHub;
use crate::state::{AppState, Settings};
use crate::storage::ObjectStorage;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,hrms_api=debug,sqlx=warn".into()),
        )
        .init();

    let settings = Arc::new(Settings::from_env());
    let config = hrms_core::IndiaPayrollConfig::load();

    let database_url = std::env::var("DATABASE_URL")
        .map_err(|_| anyhow::anyhow!("DATABASE_URL is required (postgres://user:pass@host/db)"))?;
    tracing::info!("connecting to postgres");
    let db = PgPoolOptions::new()
        .max_connections(20)
        .acquire_timeout(Duration::from_secs(10))
        .connect(&database_url)
        .await?;

    tracing::info!("applying migrations");
    sqlx::migrate!("../../migrations").run(&db).await?;

    let auth = auth::AuthConfig::from_env();
    let hub = Arc::new(DashboardHub::new());
    let storage = Arc::new(ObjectStorage::connect().await);
    let queue = Arc::new(
        JobQueue::connect(&std::env::var("REDIS_URL").unwrap_or_default()).await,
    );

    let state = AppState::new(
        db,
        config,
        auth,
        hub,
        storage,
        queue,
        (*settings).clone(),
    );

    // Prime the dashboard so the first client sees real numbers immediately.
    if let Err(err) = metrics::refresh(&state, true).await {
        tracing::warn!("initial metrics computation failed: {err}");
    }
    spawn_metrics_ticker(state.clone());
    jobs::spawn_worker(state.clone());

    let app = Router::new()
        .merge(routes::api_routes())
        .route("/ws/dashboard", get(realtime::ws_dashboard))
        .route("/health", get(health))
        .route("/health/ready", get(ready))
        .with_state(state)
        .merge(routes::swagger())
        .layer(TraceLayer::new_for_http())
        .layer(CompressionLayer::new())
        .layer(cors_layer(&settings.web_origin));

    let listener = tokio::net::TcpListener::bind(&settings.bind_addr).await?;
    tracing::info!(
        addr = %settings.bind_addr,
        org = %settings.org_name,
        swagger = "/swagger-ui",
        "hrms-api listening"
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

fn spawn_metrics_ticker(state: AppState) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(60));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ticker.tick().await; // the first tick is immediate; skip it
        loop {
            ticker.tick().await;
            metrics::tick(&state).await;
        }
    });
}

fn cors_layer(origin: &str) -> CorsLayer {
    let base = CorsLayer::new().allow_methods(Any).allow_headers(Any);
    let origin = origin.trim();
    if origin.is_empty() || origin == "*" {
        return base.allow_origin(Any);
    }
    let values: Vec<HeaderValue> = origin
        .split(',')
        .filter_map(|value| value.trim().parse::<HeaderValue>().ok())
        .collect();
    base.allow_origin(AllowOrigin::list(values))
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "service": "hrms-api",
        "version": env!("CARGO_PKG_VERSION")
    }))
}

/// Readiness includes dependencies; `/health` is a plain liveness probe.
async fn ready(State(state): State<AppState>) -> (StatusCode, Json<serde_json::Value>) {
    let database_ok = sqlx::query_scalar::<_, i32>("select 1")
        .fetch_one(&state.db)
        .await
        .is_ok();
    let body = json!({
        "status": if database_ok { "ready" } else { "degraded" },
        "database": if database_ok { "up" } else { "down" },
        "redis_queue": if state.queue.enabled() { "up" } else { "disabled" },
        "object_storage": if state.storage.enabled() { "up" } else { "disabled" },
        "dashboard_subscribers": state.hub.receiver_count(),
    });
    let code = if database_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (code, Json(body))
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install ctrl-c handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received");
}
