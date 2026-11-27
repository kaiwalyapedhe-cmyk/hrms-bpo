//! Dashboard REST endpoints. The live numbers arrive over `/ws/dashboard`;
//! these routes provide the initial snapshot, the 60-second history series and
//! the supervisor team view.

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;
use utoipa::ToSchema;

use crate::auth::{AnyRole, AuthUser, Supervisor};
use crate::error::ApiResult;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/dashboard/snapshot", get(dashboard_snapshot))
        .route("/dashboard/history", get(dashboard_history))
        .route("/dashboard/team", get(dashboard_team))
}

/// Current KPI snapshot for the caller's role scope.
#[utoipa::path(get, path = "/api/dashboard/snapshot", tag = "dashboard",
    responses((status = 200, description = "metrics + agent status grid")))]
pub async fn dashboard_snapshot(
    user: AuthUser<AnyRole>,
    State(state): State<AppState>,
) -> ApiResult<Json<Value>> {
    if state.hub.current_metric("clocked_in").is_none() {
        crate::metrics::refresh(&state, false).await.ok();
    }
    let scope = user.session.role.dashboard_scope();
    let mut snapshot = state.hub.snapshot(scope, user.session.employee_id);
    if scope == hrms_core::DashboardScope::Personal {
        if let Some(employee_id) = user.session.employee_id {
            let personal = crate::metrics::personal_metrics(&state.db, employee_id).await?;
            if let Some(metrics) = snapshot.get_mut("metrics").and_then(|m| m.as_object_mut()) {
                for (key, value) in personal {
                    metrics.insert(key, serde_json::json!((value * 100.0).round() / 100.0));
                }
            }
        }
    }
    Ok(Json(snapshot))
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct KpiPoint {
    pub captured_at: DateTime<Utc>,
    pub clocked_in: i32,
    pub scheduled: i32,
    pub schedule_adherence: Decimal,
    pub aht_15m: Decimal,
    pub aht_30m: Decimal,
    pub queue_wait: i32,
    pub csat_avg: Decimal,
    pub occupancy_pct: Decimal,
    pub shrinkage_pct: Decimal,
    pub calls_offered: i32,
    pub calls_answered: i32,
    pub abandoned_calls: i32,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct HistoryQuery {
    pub hours: Option<i32>,
}

/// 60-second KPI history for the charts (default 4 hours).
#[utoipa::path(get, path = "/api/dashboard/history", tag = "dashboard",
    params(("hours" = Option<i32>, Query)), responses((status = 200, body = Vec<KpiPoint>)))]
pub async fn dashboard_history(
    user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Query(query): Query<HistoryQuery>,
) -> ApiResult<Json<Vec<KpiPoint>>> {
    let hours = query.hours.unwrap_or(4).clamp(1, 168);
    if matches!(
        user.session.role.dashboard_scope(),
        hrms_core::DashboardScope::Personal
    ) {
        return Ok(Json(Vec::new()));
    }
    let rows = sqlx::query_as::<_, KpiPoint>(
        r#"select captured_at, clocked_in, scheduled, schedule_adherence, aht_15m, aht_30m,
                  queue_wait, csat_avg, occupancy_pct, shrinkage_pct, calls_offered,
                  calls_answered, abandoned_calls
           from kpi_snapshots
           where captured_at >= now() - ($1::int || ' hours')::interval
           order by captured_at"#,
    )
    .bind(hours)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct TeamRow {
    pub employee_id: uuid::Uuid,
    pub employee_name: String,
    pub emp_no: String,
    pub extension: Option<String>,
    pub aux_code: Option<String>,
    pub category: Option<String>,
    pub calls_today: i64,
    pub avg_handle_seconds: Option<Decimal>,
    pub qa_score: Option<Decimal>,
    pub csat: Option<Decimal>,
    pub ot_hours_today: Option<Decimal>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct TeamQuery {
    pub date: Option<NaiveDate>,
}

/// Supervisor view: per-agent calls, AHT, QA and adherence for a day.
#[utoipa::path(get, path = "/api/dashboard/team", tag = "dashboard",
    params(("date" = Option<NaiveDate>, Query)), responses((status = 200, body = Vec<TeamRow>)))]
pub async fn dashboard_team(
    user: AuthUser<Supervisor>,
    State(state): State<AppState>,
    Query(query): Query<TeamQuery>,
) -> ApiResult<Json<Vec<TeamRow>>> {
    let date = query.date.unwrap_or_else(|| Utc::now().date_naive());
    let rows = sqlx::query_as::<_, TeamRow>(
        r#"select e.id as employee_id, (e.first_name || ' ' || e.last_name) as employee_name,
                  e.emp_no, e.extension,
                  ac.code as aux_code, ac.category,
                  coalesce((select count(*) from cdr_records c
                            where c.employee_id = e.id and c.call_start::date = $2 and not c.abandoned), 0) as calls_today,
                  (select avg(c.talk_seconds + c.hold_seconds + c.wrap_seconds)
                   from cdr_records c
                   where c.employee_id = e.id and c.call_start::date = $2 and not c.abandoned) as avg_handle_seconds,
                  (select avg(q.weighted_pct) from qa_scores q
                   where q.employee_id = e.id and q.evaluated_at::date >= $2 - 7) as qa_score,
                  (select avg(q.csat_score) from qa_scores q
                   where q.employee_id = e.id and q.evaluated_at::date >= $2 - 7) as csat,
                  (select sum(t.ot_hours) from timesheets t
                   where t.employee_id = e.id and t.work_date = $2) as ot_hours_today
           from employees e
           left join lateral (
               select a.aux_code_id from agent_aux_state a
               where a.employee_id = e.id and a.ended_at is null
               order by a.started_at desc limit 1
           ) s on true
           left join aux_codes ac on ac.id = s.aux_code_id
           where e.status in ('active','probation','on_leave','notice')
             and ($1::uuid is null or e.manager_id = $1 or e.id = $1)
           order by employee_name"#,
    )
    .bind(user.session.employee_id)
    .bind(date)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}
