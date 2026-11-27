//! Time & Labor — punches, timesheets, breaks, overtime approval, shift schedule.

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::auth::{AnyRole, AuthUser, Supervisor};
use crate::error::{ApiError, ApiResult};
use crate::realtime::AgentStatus;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/punches", get(punches_list).post(punches_create))
        .route("/timesheets", get(timesheets_list).post(timesheets_submit))
        .route("/timesheets/pending", get(timesheets_pending))
        .route("/timesheets/{id}/approve", post(timesheets_approve))
        .route("/timesheets/{id}/reject", post(timesheets_reject))
        .route("/ot-requests", get(ot_list).post(ot_create))
        .route("/ot-requests/{id}/decide", post(ot_decide))
        .route("/shifts", get(shifts_list).post(shifts_create))
        .route("/shifts/assign", post(shifts_assign))
        .route("/schedule", get(schedule_list))
}

fn agent_scope(user: &crate::auth::Session, requested: Option<Uuid>) -> Option<Uuid> {
    match user.role {
        hrms_core::Role::Agent => user.employee_id,
        _ => requested,
    }
}

// ---------------------------------------------------------------------------
// Punches
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct Punch {
    pub id: Uuid,
    pub employee_id: Uuid,
    pub employee_name: String,
    pub punch_time: DateTime<Utc>,
    pub punch_type: String,
    pub source: String,
    pub device_id: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PunchQuery {
    pub employee_id: Option<Uuid>,
    pub date: Option<NaiveDate>,
}

#[utoipa::path(get, path = "/api/punches", tag = "time",
    params(("employee_id" = Option<Uuid>, Query), ("date" = Option<NaiveDate>, Query)),
    responses((status = 200, body = Vec<Punch>)))]
pub async fn punches_list(
    user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Query(query): Query<PunchQuery>,
) -> ApiResult<Json<Vec<Punch>>> {
    let employee_id = agent_scope(&user.session, query.employee_id);
    let date = query.date.unwrap_or_else(|| Utc::now().date_naive());
    let rows = sqlx::query_as::<_, Punch>(
        r#"select p.id, p.employee_id, (e.first_name || ' ' || e.last_name) as employee_name,
                  p.punch_time, p.punch_type, p.source, p.device_id
           from punches p join employees e on e.id = p.employee_id
           where ($1::uuid is null or p.employee_id = $1)
             and p.punch_time::date = $2
           order by p.punch_time desc"#,
    )
    .bind(employee_id)
    .bind(date)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PunchInput {
    pub employee_id: Option<Uuid>,
    pub punch_type: String,
    pub punch_time: Option<DateTime<Utc>>,
    pub source: Option<String>,
    pub device_id: Option<String>,
}

/// Record a punch. Punching in clears the closed aux state; punching out sets
/// the agent offline. The dashboard is refreshed right after the write.
#[utoipa::path(post, path = "/api/punches", tag = "time", request_body = PunchInput,
    responses((status = 200, body = Punch), (status = 400, description = "invalid punch type")))]
pub async fn punches_create(
    user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Json(input): Json<PunchInput>,
) -> ApiResult<Json<Punch>> {
    if !["in", "out", "break_start", "break_end"].contains(&input.punch_type.as_str()) {
        return Err(ApiError::bad("punch_type must be in|out|break_start|break_end"));
    }
    let employee_id = match user.session.role {
        hrms_core::Role::Agent => user
            .session
            .employee_id
            .ok_or_else(|| ApiError::bad("agent account is not linked to an employee"))?,
        _ => input
            .employee_id
            .or(user.session.employee_id)
            .ok_or_else(|| ApiError::bad("employee_id is required"))?,
    };
    let punch_time = input.punch_time.unwrap_or_else(Utc::now);

    let row = sqlx::query_as::<_, Punch>(
        r#"with inserted as (
             insert into punches (employee_id, punch_time, punch_type, source, device_id)
             values ($1,$2,$3,coalesce($4,'web'),$5)
             returning id, employee_id, punch_time, punch_type, source, device_id
           )
           select i.id, i.employee_id, (e.first_name || ' ' || e.last_name) as employee_name,
                  i.punch_time, i.punch_type, i.source, i.device_id
           from inserted i join employees e on e.id = i.employee_id"#,
    )
    .bind(employee_id)
    .bind(punch_time)
    .bind(&input.punch_type)
    .bind(&input.source)
    .bind(&input.device_id)
    .fetch_one(&state.db)
    .await?;

    sync_aux_for_punch(&state, employee_id, &input.punch_type).await;
    let state_clone = state.clone();
    tokio::spawn(async move {
        let _ = crate::metrics::refresh(&state_clone, false).await;
    });

    Ok(Json(row))
}

async fn sync_aux_for_punch(state: &AppState, employee_id: Uuid, punch_type: &str) {
    let code = match punch_type {
        "in" => "AVAILABLE",
        "break_start" => "BREAK",
        "break_end" => "AVAILABLE",
        _ => "OFFLINE",
    };
    let aux_code_id: Option<Uuid> =
        sqlx::query_scalar("select id from aux_codes where code = $1")
            .bind(code)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None);
    let Some(aux_code_id) = aux_code_id else {
        return;
    };
    let _ = sqlx::query("update agent_aux_state set ended_at = now() where employee_id = $1 and ended_at is null")
        .bind(employee_id)
        .execute(&state.db)
        .await;
    let _ = sqlx::query("insert into agent_aux_state (employee_id, aux_code_id) values ($1,$2)")
        .bind(employee_id)
        .bind(aux_code_id)
        .execute(&state.db)
        .await;

    let row: Option<(String, String, String, DateTime<Utc>, String)> = sqlx::query_as(
        r#"select ac.code, ac.category, ac.colour, s.started_at, (e.first_name || ' ' || e.last_name)
           from agent_aux_state s
           join aux_codes ac on ac.id = s.aux_code_id
           join employees e on e.id = s.employee_id
           where s.employee_id = $1 and s.ended_at is null
           order by s.started_at desc limit 1"#,
    )
    .bind(employee_id)
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);

    if let Some((code, category, colour, since, name)) = row {
        state.hub.publish_agent(AgentStatus {
            employee_id,
            employee_name: name,
            aux_code: code,
            category: category.clone(),
            colour,
            on_call: category == "on_call",
            since,
        });
    }
}

// ---------------------------------------------------------------------------
// Timesheets
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct Timesheet {
    pub id: Uuid,
    pub employee_id: Uuid,
    pub employee_name: String,
    pub work_date: NaiveDate,
    pub in_time: Option<DateTime<Utc>>,
    pub out_time: Option<DateTime<Utc>>,
    pub break_minutes: i32,
    pub regular_hours: Decimal,
    pub ot_hours: Decimal,
    pub status: String,
    pub notes: Option<String>,
    pub approved_at: Option<DateTime<Utc>>,
}

const TIMESHEET_SELECT: &str = r#"
select t.id, t.employee_id, (e.first_name || ' ' || e.last_name) as employee_name,
       t.work_date, t.in_time, t.out_time, t.break_minutes, t.regular_hours, t.ot_hours,
       t.status, t.notes, t.approved_at
from timesheets t join employees e on e.id = t.employee_id
"#;

#[derive(Debug, Deserialize, ToSchema)]
pub struct TimesheetQuery {
    pub employee_id: Option<Uuid>,
    pub status: Option<String>,
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
}

#[utoipa::path(get, path = "/api/timesheets", tag = "time",
    params(("employee_id" = Option<Uuid>, Query), ("status" = Option<String>, Query),
           ("from" = Option<NaiveDate>, Query), ("to" = Option<NaiveDate>, Query)),
    responses((status = 200, body = Vec<Timesheet>)))]
pub async fn timesheets_list(
    user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Query(query): Query<TimesheetQuery>,
) -> ApiResult<Json<Vec<Timesheet>>> {
    let employee_id = agent_scope(&user.session, query.employee_id);
    let sql = format!(
        "{TIMESHEET_SELECT}
         where ($1::uuid is null or t.employee_id = $1)
           and ($2::text is null or t.status = $2)
           and ($3::date is null or t.work_date >= $3)
           and ($4::date is null or t.work_date <= $4)
         order by t.work_date desc, employee_name
         limit 500"
    );
    let rows = sqlx::query_as::<_, Timesheet>(&sql)
        .bind(employee_id)
        .bind(query.status)
        .bind(query.from)
        .bind(query.to)
        .fetch_all(&state.db)
        .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct TimesheetInput {
    pub employee_id: Option<Uuid>,
    pub work_date: NaiveDate,
    pub in_time: Option<DateTime<Utc>>,
    pub out_time: Option<DateTime<Utc>>,
    pub break_minutes: Option<i32>,
    pub ot_hours: Option<Decimal>,
    pub notes: Option<String>,
}

/// Submit (or replace) a timesheet for a day. Regular hours are derived from
/// in/out minus breaks; overtime is what the employee claims and a supervisor
/// approves.
#[utoipa::path(post, path = "/api/timesheets", tag = "time", request_body = TimesheetInput,
    responses((status = 200, body = Timesheet)))]
pub async fn timesheets_submit(
    user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Json(input): Json<TimesheetInput>,
) -> ApiResult<Json<Timesheet>> {
    let employee_id = match user.session.role {
        hrms_core::Role::Agent => user
            .session
            .employee_id
            .ok_or_else(|| ApiError::bad("agent account is not linked to an employee"))?,
        _ => input
            .employee_id
            .or(user.session.employee_id)
            .ok_or_else(|| ApiError::bad("employee_id is required"))?,
    };
    let break_minutes = input.break_minutes.unwrap_or(0).max(0);
    let regular_hours = match (input.in_time, input.out_time) {
        (Some(in_time), Some(out_time)) => {
            let minutes = (out_time - in_time).num_minutes() - break_minutes as i64;
            Decimal::from(minutes.max(0)) / Decimal::from(60)
        }
        _ => Decimal::ZERO,
    };
    let shift_id: Option<Uuid> = sqlx::query_scalar(
        "select shift_id from shift_assignments where employee_id = $1 and work_date = $2",
    )
    .bind(employee_id)
    .bind(input.work_date)
    .fetch_optional(&state.db)
    .await?;

    let row = sqlx::query_as::<_, Timesheet>(
        r#"with upserted as (
             insert into timesheets
               (employee_id, work_date, shift_id, in_time, out_time, break_minutes, regular_hours, ot_hours, notes, status)
             values ($1,$2,$3,$4,$5,$6,$7,coalesce($8,0),$9,'submitted')
             on conflict (employee_id, work_date) do update set
               in_time = excluded.in_time,
               out_time = excluded.out_time,
               break_minutes = excluded.break_minutes,
               regular_hours = excluded.regular_hours,
               ot_hours = excluded.ot_hours,
               notes = excluded.notes,
               status = case when timesheets.status = 'approved' then 'approved' else 'submitted' end
             returning id, employee_id, work_date, in_time, out_time, break_minutes,
                       regular_hours, ot_hours, status, notes, approved_at
           )
           select u.id, u.employee_id, (e.first_name || ' ' || e.last_name) as employee_name,
                  u.work_date, u.in_time, u.out_time, u.break_minutes, u.regular_hours,
                  u.ot_hours, u.status, u.notes, u.approved_at
           from upserted u join employees e on e.id = u.employee_id"#,
    )
    .bind(employee_id)
    .bind(input.work_date)
    .bind(shift_id)
    .bind(input.in_time)
    .bind(input.out_time)
    .bind(break_minutes)
    .bind(regular_hours)
    .bind(input.ot_hours)
    .bind(&input.notes)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(row))
}

#[utoipa::path(get, path = "/api/timesheets/pending", tag = "time",
    responses((status = 200, body = Vec<Timesheet>)))]
pub async fn timesheets_pending(
    user: AuthUser<Supervisor>,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<Timesheet>>> {
    let sql = format!(
        "{TIMESHEET_SELECT}
         where t.status = 'submitted'
           and ($1::uuid is null or e.manager_id = $1 or e.id = $1)
         order by t.work_date, employee_name limit 500"
    );
    let rows = sqlx::query_as::<_, Timesheet>(&sql)
        .bind(user.session.employee_id)
        .fetch_all(&state.db)
        .await?;
    Ok(Json(rows))
}

#[utoipa::path(post, path = "/api/timesheets/{id}/approve", tag = "time",
    params(("id" = Uuid, Path)), responses((status = 200, body = Timesheet)))]
pub async fn timesheets_approve(
    user: AuthUser<Supervisor>,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Timesheet>> {
    decide_timesheet(&state, &user.session, id, "approved").await
}

#[utoipa::path(post, path = "/api/timesheets/{id}/reject", tag = "time",
    params(("id" = Uuid, Path)), responses((status = 200, body = Timesheet)))]
pub async fn timesheets_reject(
    user: AuthUser<Supervisor>,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Timesheet>> {
    decide_timesheet(&state, &user.session, id, "rejected").await
}

async fn decide_timesheet(
    state: &AppState,
    session: &crate::auth::Session,
    id: Uuid,
    status: &str,
) -> ApiResult<Json<Timesheet>> {
    sqlx::query(
        "update timesheets set status = $2, approved_by = $3, approved_at = now() where id = $1",
    )
    .bind(id)
    .bind(status)
    .bind(session.employee_id)
    .execute(&state.db)
    .await?;
    let sql = format!("{TIMESHEET_SELECT} where t.id = $1");
    let row = sqlx::query_as::<_, Timesheet>(&sql)
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| ApiError::not_found("timesheet"))?;
    state
        .audit(Some(session), status, "timesheet", Some(id.to_string()), serde_json::json!({}))
        .await;
    Ok(Json(row))
}

// ---------------------------------------------------------------------------
// Overtime
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct OtRequest {
    pub id: Uuid,
    pub employee_id: Uuid,
    pub employee_name: String,
    pub work_date: NaiveDate,
    pub hours: Decimal,
    pub reason: Option<String>,
    pub status: String,
    pub decided_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct OtQuery {
    pub status: Option<String>,
    pub employee_id: Option<Uuid>,
}

#[utoipa::path(get, path = "/api/ot-requests", tag = "time",
    params(("status" = Option<String>, Query), ("employee_id" = Option<Uuid>, Query)),
    responses((status = 200, body = Vec<OtRequest>)))]
pub async fn ot_list(
    user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Query(query): Query<OtQuery>,
) -> ApiResult<Json<Vec<OtRequest>>> {
    let employee_id = agent_scope(&user.session, query.employee_id);
    let rows = sqlx::query_as::<_, OtRequest>(
        r#"select o.id, o.employee_id, (e.first_name || ' ' || e.last_name) as employee_name,
                  o.work_date, o.hours, o.reason, o.status, o.decided_at
           from ot_requests o join employees e on e.id = o.employee_id
           where ($1::text is null or o.status = $1)
             and ($2::uuid is null or o.employee_id = $2)
           order by o.work_date desc limit 300"#,
    )
    .bind(query.status)
    .bind(employee_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct OtInput {
    pub employee_id: Option<Uuid>,
    pub work_date: NaiveDate,
    pub hours: Decimal,
    pub reason: Option<String>,
}

#[utoipa::path(post, path = "/api/ot-requests", tag = "time", request_body = OtInput,
    responses((status = 200, body = OtRequest)))]
pub async fn ot_create(
    user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Json(input): Json<OtInput>,
) -> ApiResult<Json<OtRequest>> {
    if input.hours <= Decimal::ZERO || input.hours > Decimal::from(12) {
        return Err(ApiError::bad("overtime hours must be between 0 and 12"));
    }
    let employee_id = input
        .employee_id
        .or(user.session.employee_id)
        .ok_or_else(|| ApiError::bad("employee_id is required"))?;
    let row = sqlx::query_as::<_, OtRequest>(
        r#"with inserted as (
             insert into ot_requests (employee_id, work_date, hours, reason)
             values ($1,$2,$3,$4)
             returning id, employee_id, work_date, hours, reason, status, decided_at
           )
           select i.id, i.employee_id, (e.first_name || ' ' || e.last_name) as employee_name,
                  i.work_date, i.hours, i.reason, i.status, i.decided_at
           from inserted i join employees e on e.id = i.employee_id"#,
    )
    .bind(employee_id)
    .bind(input.work_date)
    .bind(input.hours)
    .bind(&input.reason)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(row))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct DecisionInput {
    pub approve: bool,
}

#[utoipa::path(post, path = "/api/ot-requests/{id}/decide", tag = "time",
    params(("id" = Uuid, Path)), request_body = DecisionInput,
    responses((status = 200, body = OtRequest)))]
pub async fn ot_decide(
    user: AuthUser<Supervisor>,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(input): Json<DecisionInput>,
) -> ApiResult<Json<OtRequest>> {
    let status = if input.approve { "approved" } else { "rejected" };
    sqlx::query("update ot_requests set status = $2, approver_id = $3, decided_at = now() where id = $1")
        .bind(id)
        .bind(status)
        .bind(user.session.employee_id)
        .execute(&state.db)
        .await?;
    if input.approve {
        // Approved overtime flows into the day's timesheet so payroll sees it.
        sqlx::query(
            r#"update timesheets t set ot_hours = greatest(t.ot_hours, o.hours)
               from ot_requests o
               where o.id = $1 and t.employee_id = o.employee_id and t.work_date = o.work_date"#,
        )
        .bind(id)
        .execute(&state.db)
        .await?;
    }
    let row = sqlx::query_as::<_, OtRequest>(
        r#"select o.id, o.employee_id, (e.first_name || ' ' || e.last_name) as employee_name,
                  o.work_date, o.hours, o.reason, o.status, o.decided_at
           from ot_requests o join employees e on e.id = o.employee_id where o.id = $1"#,
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| ApiError::not_found("overtime request"))?;
    state
        .audit(Some(&user.session), status, "ot_request", Some(id.to_string()), serde_json::json!({}))
        .await;
    Ok(Json(row))
}

// ---------------------------------------------------------------------------
// Shifts & schedule
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct Shift {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    pub start_time: chrono::NaiveTime,
    pub end_time: chrono::NaiveTime,
    pub break_minutes: i32,
    pub grace_minutes: i32,
    pub is_night: bool,
    pub active: bool,
}

#[utoipa::path(get, path = "/api/shifts", tag = "time", responses((status = 200, body = Vec<Shift>)))]
pub async fn shifts_list(
    _user: AuthUser<AnyRole>,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<Shift>>> {
    let rows = sqlx::query_as::<_, Shift>(
        "select id, code, name, start_time, end_time, break_minutes, grace_minutes, is_night, active
         from shifts order by start_time",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ShiftInput {
    pub code: String,
    pub name: String,
    pub start_time: chrono::NaiveTime,
    pub end_time: chrono::NaiveTime,
    pub break_minutes: Option<i32>,
    pub grace_minutes: Option<i32>,
    pub is_night: Option<bool>,
}

#[utoipa::path(post, path = "/api/shifts", tag = "time", request_body = ShiftInput,
    responses((status = 200, body = Shift)))]
pub async fn shifts_create(
    _user: AuthUser<Supervisor>,
    State(state): State<AppState>,
    Json(input): Json<ShiftInput>,
) -> ApiResult<Json<Shift>> {
    let row = sqlx::query_as::<_, Shift>(
        "insert into shifts (code, name, start_time, end_time, break_minutes, grace_minutes, is_night)
         values ($1,$2,$3,$4,coalesce($5,45),coalesce($6,5),coalesce($7,false))
         returning id, code, name, start_time, end_time, break_minutes, grace_minutes, is_night, active",
    )
    .bind(&input.code)
    .bind(&input.name)
    .bind(input.start_time)
    .bind(input.end_time)
    .bind(input.break_minutes)
    .bind(input.grace_minutes)
    .bind(input.is_night)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(row))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AssignShiftInput {
    pub employee_id: Uuid,
    pub shift_id: Uuid,
    pub work_date: NaiveDate,
}

#[utoipa::path(post, path = "/api/shifts/assign", tag = "time", request_body = AssignShiftInput,
    responses((status = 200, description = "assignment stored")))]
pub async fn shifts_assign(
    _user: AuthUser<Supervisor>,
    State(state): State<AppState>,
    Json(input): Json<AssignShiftInput>,
) -> ApiResult<Json<serde_json::Value>> {
    sqlx::query(
        "insert into shift_assignments (employee_id, shift_id, work_date) values ($1,$2,$3)
         on conflict (employee_id, work_date) do update set shift_id = excluded.shift_id",
    )
    .bind(input.employee_id)
    .bind(input.shift_id)
    .bind(input.work_date)
    .execute(&state.db)
    .await?;
    Ok(Json(serde_json::json!({ "status": "assigned" })))
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct ScheduleEntry {
    pub employee_id: Uuid,
    pub employee_name: String,
    pub work_date: NaiveDate,
    pub shift_code: String,
    pub start_time: chrono::NaiveTime,
    pub end_time: chrono::NaiveTime,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ScheduleQuery {
    pub date: Option<NaiveDate>,
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
}

#[utoipa::path(get, path = "/api/schedule", tag = "time",
    params(("date" = Option<NaiveDate>, Query), ("from" = Option<NaiveDate>, Query), ("to" = Option<NaiveDate>, Query)),
    responses((status = 200, body = Vec<ScheduleEntry>)))]
pub async fn schedule_list(
    _user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Query(query): Query<ScheduleQuery>,
) -> ApiResult<Json<Vec<ScheduleEntry>>> {
    let from = query
        .from
        .or(query.date)
        .unwrap_or_else(|| Utc::now().date_naive());
    let to = query
        .to
        .or(query.date)
        .unwrap_or_else(|| from + chrono::Duration::days(13));
    let rows = sqlx::query_as::<_, ScheduleEntry>(
        r#"select sa.employee_id, (e.first_name || ' ' || e.last_name) as employee_name,
                  sa.work_date, s.code as shift_code, s.start_time, s.end_time
           from shift_assignments sa
           join shifts s on s.id = sa.shift_id
           join employees e on e.id = sa.employee_id
           where sa.work_date between $1 and $2
           order by sa.work_date, s.start_time, employee_name"#,
    )
    .bind(from)
    .bind(to)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}
