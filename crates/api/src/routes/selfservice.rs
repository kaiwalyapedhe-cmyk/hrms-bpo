//! Employee and Manager self-service summaries.

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{Datelike, Utc};
use rust_decimal::Decimal;
use serde::Serialize;
use sqlx::FromRow;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::auth::{AnyRole, AuthUser, Supervisor};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/me/summary", get(me_summary))
        .route("/me/notifications", get(me_notifications))
        .route("/me/notifications/{id}/read", post(me_notification_read))
        .route("/manager/overview", get(manager_overview))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MeSummary {
    pub employee: serde_json::Value,
    pub leave_balances: serde_json::Value,
    pub latest_payslip: serde_json::Value,
    pub today_shift: serde_json::Value,
    pub open_goals: serde_json::Value,
    pub unread_notifications: i64,
    pub aux_state: serde_json::Value,
}

/// Everything an employee needs on the self-service home page.
#[utoipa::path(get, path = "/api/me/summary", tag = "selfservice",
    responses((status = 200, body = MeSummary), (status = 400, description = "account not linked to an employee")))]
pub async fn me_summary(
    user: AuthUser<AnyRole>,
    State(state): State<AppState>,
) -> ApiResult<Json<MeSummary>> {
    let employee_id = user
        .session
        .employee_id
        .ok_or_else(|| ApiError::bad("this account is not linked to an employee record"))?;

    let employee: serde_json::Value = sqlx::query_scalar(
        r#"select jsonb_build_object(
                    'id', e.id, 'emp_no', e.emp_no,
                    'name', e.first_name || ' ' || e.last_name,
                    'email', e.email, 'phone', e.phone,
                    'designation', p.title, 'department', d.name,
                    'work_state', e.work_state, 'status', e.status,
                    'date_of_joining', e.date_of_joining, 'tax_regime', e.tax_regime,
                    'manager', m.first_name || ' ' || m.last_name, 'uan', e.uan, 'pan', e.pan)
           from employees e
           left join positions p on p.id = e.position_id
           left join departments d on d.id = e.department_id
           left join employees m on m.id = e.manager_id
           where e.id = $1"#,
    )
    .bind(employee_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| ApiError::not_found("employee"))?;

    let leave_balances: serde_json::Value = sqlx::query_scalar(
        r#"select coalesce(jsonb_agg(jsonb_build_object(
                     'code', lt.code, 'name', lt.name, 'accrued', b.accrued,
                     'used', b.used, 'available', b.available)), '[]'::jsonb)
           from leave_balances b join leave_types lt on lt.id = b.leave_type_id
           where b.employee_id = $1 and b.year = $2"#,
    )
    .bind(employee_id)
    .bind(Utc::now().date_naive().year())
    .fetch_one(&state.db)
    .await?;

    let latest_payslip: serde_json::Value = sqlx::query_scalar(
        r#"select coalesce(jsonb_build_object(
                     'id', id, 'period_year', period_year, 'period_month', period_month,
                     'gross', gross, 'net_pay', net_pay, 'tds', tds, 'status', status), '{}'::jsonb)
           from payslips
           where employee_id = $1 and not is_reversal and status <> 'reversed'
           order by period_year desc, period_month desc limit 1"#,
    )
    .bind(employee_id)
    .fetch_optional(&state.db)
    .await?
    .unwrap_or_else(|| serde_json::json!({}));

    let today_shift: serde_json::Value = sqlx::query_scalar(
        r#"select coalesce(jsonb_build_object(
                     'shift', s.name, 'code', s.code, 'start_time', s.start_time,
                     'end_time', s.end_time, 'break_minutes', s.break_minutes), '{}'::jsonb)
           from shift_assignments sa join shifts s on s.id = sa.shift_id
           where sa.employee_id = $1 and sa.work_date = current_date"#,
    )
    .bind(employee_id)
    .fetch_optional(&state.db)
    .await?
    .unwrap_or_else(|| serde_json::json!({}));

    let open_goals: serde_json::Value = sqlx::query_scalar(
        r#"select coalesce(jsonb_agg(jsonb_build_object(
                     'id', id, 'title', title, 'weight', weight, 'progress', progress,
                     'status', status)), '[]'::jsonb)
           from goals where employee_id = $1 and status in ('active','draft')"#,
    )
    .bind(employee_id)
    .fetch_one(&state.db)
    .await?;

    let aux_state: serde_json::Value = sqlx::query_scalar(
        r#"select coalesce(jsonb_build_object(
                     'code', ac.code, 'category', ac.category, 'colour', ac.colour,
                     'since', s.started_at), '{}'::jsonb)
           from agent_aux_state s join aux_codes ac on ac.id = s.aux_code_id
           where s.employee_id = $1 and s.ended_at is null
           order by s.started_at desc limit 1"#,
    )
    .bind(employee_id)
    .fetch_optional(&state.db)
    .await?
    .unwrap_or_else(|| serde_json::json!({}));

    let unread_notifications: i64 = sqlx::query_scalar(
        "select count(*) from notifications where employee_id = $1 and not read",
    )
    .bind(employee_id)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(MeSummary {
        employee,
        leave_balances,
        latest_payslip,
        today_shift,
        open_goals,
        unread_notifications,
        aux_state,
    }))
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct Notification {
    pub id: Uuid,
    pub title: String,
    pub body: String,
    pub category: String,
    pub read: bool,
    pub created_at: chrono::DateTime<Utc>,
}

#[utoipa::path(get, path = "/api/me/notifications", tag = "selfservice",
    responses((status = 200, body = Vec<Notification>)))]
pub async fn me_notifications(
    user: AuthUser<AnyRole>,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<Notification>>> {
    let employee_id = user
        .session
        .employee_id
        .ok_or_else(|| ApiError::bad("this account is not linked to an employee record"))?;
    let rows = sqlx::query_as::<_, Notification>(
        "select id, title, body, category, read, created_at from notifications
         where employee_id = $1 order by created_at desc limit 100",
    )
    .bind(employee_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[utoipa::path(post, path = "/api/me/notifications/{id}/read", tag = "selfservice",
    params(("id" = Uuid, Path)), responses((status = 200, description = "marked read")))]
pub async fn me_notification_read(
    user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let employee_id = user
        .session
        .employee_id
        .ok_or_else(|| ApiError::bad("this account is not linked to an employee record"))?;
    sqlx::query("update notifications set read = true where id = $1 and employee_id = $2")
        .bind(id)
        .bind(employee_id)
        .execute(&state.db)
        .await?;
    Ok(Json(serde_json::json!({ "status": "read" })))
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct ManagerOverview {
    pub team_size: i64,
    pub present_today: i64,
    pub pending_timesheets: i64,
    pub pending_leave: i64,
    pub pending_overtime: i64,
    pub calls_today: i64,
    pub avg_qa_score: Option<Decimal>,
    pub avg_csat: Option<Decimal>,
}

/// Manager/TeamLead home: team headcount and the approvals waiting on them.
#[utoipa::path(get, path = "/api/manager/overview", tag = "selfservice",
    responses((status = 200, body = ManagerOverview)))]
pub async fn manager_overview(
    user: AuthUser<Supervisor>,
    State(state): State<AppState>,
) -> ApiResult<Json<ManagerOverview>> {
    let manager_id = user.session.employee_id;
    let row = sqlx::query_as::<_, ManagerOverview>(
        r#"select
             (select count(*) from employees e
              where ($1::uuid is null or e.manager_id = $1) and e.status <> 'separated') as team_size,
             (select count(distinct p.employee_id) from punches p join employees e on e.id = p.employee_id
              where ($1::uuid is null or e.manager_id = $1)
                and p.punch_time::date = current_date and p.punch_type = 'in') as present_today,
             (select count(*) from timesheets t join employees e on e.id = t.employee_id
              where t.status = 'submitted' and ($1::uuid is null or e.manager_id = $1)) as pending_timesheets,
             (select count(*) from leave_requests r join employees e on e.id = r.employee_id
              where r.status = 'pending' and ($1::uuid is null or e.manager_id = $1)) as pending_leave,
             (select count(*) from ot_requests o join employees e on e.id = o.employee_id
              where o.status = 'pending' and ($1::uuid is null or e.manager_id = $1)) as pending_overtime,
             (select count(*) from cdr_records c join employees e on e.id = c.employee_id
              where c.call_start::date = current_date and ($1::uuid is null or e.manager_id = $1)) as calls_today,
             (select avg(q.weighted_pct) from qa_scores q join employees e on e.id = q.employee_id
              where q.evaluated_at >= now() - interval '7 days'
                and ($1::uuid is null or e.manager_id = $1)) as avg_qa_score,
             (select avg(q.csat_score) from qa_scores q join employees e on e.id = q.employee_id
              where q.evaluated_at >= now() - interval '7 days'
                and ($1::uuid is null or e.manager_id = $1)) as avg_csat"#,
    )
    .bind(manager_id)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(row))
}
