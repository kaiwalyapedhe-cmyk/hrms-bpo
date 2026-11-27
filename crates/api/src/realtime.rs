//! Real-time dashboard fan-out.
//!
//! A single `tokio::sync::broadcast` channel carries *deltas* — only metrics
//! whose value changed since the last publish. Every connected WebSocket gets
//! the frames its role is allowed to see; nothing triggers a page reload.

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::Response;
use chrono::{DateTime, Utc};
use futures::{SinkExt, StreamExt};
use hrms_core::{DashboardScope, Role};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::auth::session_from_query_token;
use crate::error::ApiError;
use crate::state::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Audience {
    Full,
    QualityOnly,
    Personal,
    Workforce,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct AgentStatus {
    pub employee_id: Uuid,
    pub employee_name: String,
    pub aux_code: String,
    pub category: String,
    pub colour: String,
    pub on_call: bool,
    pub since: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct DashboardEvent {
    pub audience: Audience,
    pub personal_for: Option<Uuid>,
    pub payload: Value,
}

pub struct DashboardHub {
    tx: broadcast::Sender<DashboardEvent>,
    metrics: RwLock<HashMap<String, f64>>,
    agents: RwLock<HashMap<Uuid, AgentStatus>>,
}

impl Default for DashboardHub {
    fn default() -> Self {
        Self::new()
    }
}

impl DashboardHub {
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(512);
        DashboardHub {
            tx,
            metrics: RwLock::new(HashMap::new()),
            agents: RwLock::new(HashMap::new()),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<DashboardEvent> {
        self.tx.subscribe()
    }

    pub fn receiver_count(&self) -> usize {
        self.tx.receiver_count()
    }

    fn send(&self, event: DashboardEvent) {
        // No subscribers is normal (nobody has the wallboard open).
        let _ = self.tx.send(event);
    }

    /// Publish metric values; only changed ones are sent.
    pub fn publish_metrics(&self, updates: &[(&str, f64)], audience: Audience) {
        let mut delta = Map::new();
        {
            let mut guard = self.metrics.write().expect("metrics lock");
            for (key, value) in updates {
                let changed = guard
                    .get(*key)
                    .map(|previous| (previous - value).abs() > 1e-9)
                    .unwrap_or(true);
                if changed {
                    guard.insert((*key).to_string(), *value);
                    delta.insert((*key).to_string(), json!(round2(*value)));
                }
            }
        }
        if delta.is_empty() {
            return;
        }
        self.send(DashboardEvent {
            audience,
            personal_for: None,
            payload: json!({
                "type": "metric_delta",
                "metrics": delta,
                "ts": Utc::now(),
            }),
        });
    }

    /// Personal metric push for one agent (their own AHT / CSAT / status).
    pub fn publish_personal(&self, employee_id: Uuid, updates: &[(&str, f64)]) {
        if updates.is_empty() {
            return;
        }
        let metrics: Map<String, Value> = updates
            .iter()
            .map(|(k, v)| ((*k).to_string(), json!(round2(*v))))
            .collect();
        self.send(DashboardEvent {
            audience: Audience::Personal,
            personal_for: Some(employee_id),
            payload: json!({
                "type": "personal_metrics",
                "metrics": metrics,
                "ts": Utc::now(),
            }),
        });
    }

    pub fn publish_agent(&self, status: AgentStatus) {
        {
            let mut guard = self.agents.write().expect("agents lock");
            guard.insert(status.employee_id, status.clone());
        }
        self.send(DashboardEvent {
            audience: Audience::Full,
            personal_for: None,
            payload: json!({ "type": "agent_status", "agent": status }),
        });
    }

    pub fn seed_agents(&self, statuses: Vec<AgentStatus>) {
        let mut guard = self.agents.write().expect("agents lock");
        for status in statuses {
            guard.insert(status.employee_id, status);
        }
    }

    /// Snapshot used when a client connects (or the SPA refreshes).
    pub fn snapshot(&self, scope: DashboardScope, employee_id: Option<Uuid>) -> Value {
        let metrics = self.metrics.read().expect("metrics lock");
        let visible: Map<String, Value> = metrics
            .iter()
            .filter(|(key, _)| metric_visible(scope, key))
            .map(|(k, v)| (k.clone(), json!(round2(*v))))
            .collect();
        let agents = self.agents.read().expect("agents lock");
        let agent_list: Vec<AgentStatus> = match (scope, employee_id) {
            (DashboardScope::Personal, Some(emp)) => agents
                .get(&emp)
                .cloned()
                .into_iter()
                .collect(),
            (DashboardScope::Personal, None) => Vec::new(),
            _ => agents.values().cloned().collect(),
        };
        json!({
            "type": "snapshot",
            "scope": format!("{scope:?}").to_lowercase(),
            "metrics": visible,
            "agents": agent_list,
            "ts": Utc::now(),
        })
    }

    pub fn current_metric(&self, key: &str) -> Option<f64> {
        self.metrics.read().expect("metrics lock").get(key).copied()
    }
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn metric_visible(scope: DashboardScope, key: &str) -> bool {
    match scope {
        DashboardScope::Full | DashboardScope::Workforce => true,
        DashboardScope::QualityOnly => {
            key.starts_with("csat") || key.starts_with("qa_") || key.starts_with("aht")
        }
        DashboardScope::Personal => key.starts_with("my_"),
    }
}

fn event_visible(scope: DashboardScope, employee_id: Option<Uuid>, event: &DashboardEvent) -> bool {
    match (scope, event.audience) {
        (DashboardScope::Full, _) => true,
        (DashboardScope::Workforce, Audience::Workforce | Audience::Full) => true,
        (DashboardScope::QualityOnly, Audience::QualityOnly | Audience::Full) => true,
        (DashboardScope::Personal, Audience::Personal) => {
            event.personal_for.is_some() && event.personal_for == employee_id
        }
        _ => false,
    }
}

#[derive(Debug, Deserialize)]
pub struct WsQuery {
    pub token: String,
}

/// `GET /ws/dashboard?token=<jwt>`
///
/// The browser WebSocket API cannot send an Authorization header, so the JWT
/// is passed as a query parameter, validated before the upgrade.
pub async fn ws_dashboard(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Query(query): Query<WsQuery>,
) -> Result<Response, ApiError> {
    let session = session_from_query_token(&state, &query.token)?;
    let scope = session.scope();
    let employee_id = session.employee_id;
    tracing::debug!(
        role = session.role.as_str(),
        "websocket dashboard connected"
    );
    Ok(ws.on_upgrade(move |socket| handle_socket(socket, state, scope, employee_id, session.role)))
}

async fn handle_socket(
    socket: WebSocket,
    state: AppState,
    scope: DashboardScope,
    employee_id: Option<Uuid>,
    role: Role,
) {
    let (mut sender, mut receiver) = socket.split();

    let mut snapshot = state.hub.snapshot(scope, employee_id);
    if scope == DashboardScope::Personal {
        if let Some(emp) = employee_id {
            if let Ok(personal) = crate::metrics::personal_metrics(&state.db, emp).await {
                if let Some(obj) = snapshot.as_object_mut() {
                    if let Some(metrics) = obj.get_mut("metrics").and_then(|m| m.as_object_mut()) {
                        for (k, v) in personal {
                            metrics.insert(k, json!(round2(v)));
                        }
                    }
                }
            }
        }
    }
    if sender
        .send(Message::Text(snapshot.to_string().into()))
        .await
        .is_err()
    {
        return;
    }

    let mut rx = state.hub.subscribe();
    let mut ping = tokio::time::interval(Duration::from_secs(25));
    ping.tick().await;

    loop {
        tokio::select! {
            event = rx.recv() => {
                match event {
                    Ok(event) => {
                        if event_visible(scope, employee_id, &event) {
                            if sender.send(Message::Text(event.payload.to_string().into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::debug!(skipped, "dashboard client lagged; resyncing snapshot");
                        let snapshot = state.hub.snapshot(scope, employee_id);
                        if sender.send(Message::Text(snapshot.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            incoming = receiver.next() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Ping(payload))) => {
                        if sender.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Some(Err(_)) => break,
                    _ => {}
                }
            }
            _ = ping.tick() => {
                if sender.send(Message::Ping(Default::default())).await.is_err() {
                    break;
                }
            }
        }
    }

    tracing::debug!(role = role.as_str(), "websocket dashboard disconnected");
}
