use std::sync::Arc;

use hrms_core::{IndiaPayrollConfig, PayrollEngine};
use sqlx::PgPool;

use crate::auth::AuthConfig;
use crate::storage::ObjectStorage;

/// Organisation-level settings that are not statutory (those live in
/// `config/india_payroll.toml`).
#[derive(Debug, Clone)]
pub struct Settings {
    pub org_name: String,
    pub org_tan: String,
    pub pf_establishment_code: String,
    pub esi_establishment_code: String,
    pub bind_addr: String,
    pub web_origin: String,
    pub default_seed_password: String,
    pub demo_clock_skew_seconds: i64,
}

impl Settings {
    pub fn from_env() -> Self {
        let env_or = |key: &str, default: &str| std::env::var(key).unwrap_or_else(|_| default.to_string());
        Settings {
            org_name: env_or("ORG_NAME", "Nimbus Contact Solutions Pvt Ltd"),
            org_tan: env_or("ORG_TAN", "BLRN12345E"),
            pf_establishment_code: env_or("PF_ESTABLISHMENT_CODE", "KN/BNG/0045678"),
            esi_establishment_code: env_or("ESI_ESTABLISHMENT_CODE", "53000123450000199"),
            bind_addr: env_or("BIND_ADDR", "0.0.0.0:8080"),
            web_origin: env_or("WEB_ORIGIN", "http://localhost:3000"),
            default_seed_password: env_or("SEED_PASSWORD", "ChangeMe#2026"),
            demo_clock_skew_seconds: env_or("DEMO_CLOCK_SKEW_SECONDS", "0").parse().unwrap_or(0),
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub config: Arc<IndiaPayrollConfig>,
    pub engine: Arc<PayrollEngine>,
    pub auth: Arc<AuthConfig>,
    pub storage: Arc<ObjectStorage>,
    pub settings: Arc<Settings>,
}

impl AppState {
    pub fn new(
        db: PgPool,
        config: IndiaPayrollConfig,
        auth: AuthConfig,
        storage: Arc<ObjectStorage>,
        settings: Settings,
    ) -> Self {
        let config = Arc::new(config);
        AppState {
            db,
            engine: Arc::new(PayrollEngine::new((*config).clone())),
            config,
            auth: Arc::new(auth),
            storage,
            settings: Arc::new(settings),
        }
    }

    /// Insert an audit row. Failures are logged, never propagated: an audit
    /// write must not break the business transaction the user asked for.
    pub async fn audit(&self, actor: Option<&crate::auth::Session>, action: &str, entity: &str, entity_id: Option<String>, payload: serde_json::Value) {
        let actor_id = actor.map(|a| a.user_id);
        let actor_email = actor.map(|a| a.email.clone());
        if let Err(err) = sqlx::query(
            "insert into audit_log (actor_id, actor_email, action, entity, entity_id, payload)
             values ($1, $2, $3, $4, $5, $6)",
        )
        .bind(actor_id)
        .bind(actor_email)
        .bind(action)
        .bind(entity)
        .bind(entity_id)
        .bind(payload)
        .execute(&self.db)
        .await
        {
            tracing::warn!("audit write failed: {err}");
        }
    }
}
