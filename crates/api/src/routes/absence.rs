//! Absence Management — leave types, balances, requests, accruals, forecasting.

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{Datelike, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::auth::{AnyRole, AuthUser, Supervisor};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/leave/types", get(leave_types_list))
        .route("/leave/balances", get(leave_balances))
        .route("/leave/requests", get(leave_requests_list).post(leave_requests_create))
        .route("/leave/requests/{id}/decide", post(leave_decide))
        .route("/leave/forecast", get(leave_forecast))
        .route("/leave/accrual/run", post(leave_accrual_run))
        .route("/holidays", get(holidays_list))
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct LeaveType {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    pub annual_quota: Decimal,
    pub monthly_accrual: Decimal,
    pub carry_forward: bool,
    pub encashable: bool,
    pub paid: bool,
    pub max_balance: Option<Decimal>,
}

#[utoipa::path(get, path = "/api/leave/types", tag = "absence", responses((status = 200, body = Vec<LeaveType>)))]
pub async fn leave_types_list(
    _user: AuthUser<AnyRole>,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<LeaveType>>> {
    let rows = sqlx::query_as::<_, LeaveType>(
        "select id, code, name, annual_quota, monthly_accrual, carry_forward, encashable, paid, max_balance
         from leave_types order by name",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct LeaveBalance {
    pub employee_id: Uuid,
    pub leave_type_id: Uuid,
    pub leave_code: String,
    pub leave_name: String,
    pub year: i32,
    pub accrued: Decimal,
    pub used: Decimal,
    pub available: Decimal,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct BalanceQuery {
    pub employee_id: Option<Uuid>,
    pub year: Option<i32>,
}

#[utoipa::path(get, path = "/api/leave/balances", tag = "absence",
    params(("employee_id" = Option<Uuid>, Query), ("year" = Option<i32>, Query)),
    responses((status = 200, body = Vec<LeaveBalance>)))]
pub async fn leave_balances(
    user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Query(query): Query<BalanceQuery>,
) -> ApiResult<Json<Vec<LeaveBalance>>> {
    let employee_id = match user.session.role {
        hrms_core::Role::Agent => user.session.employee_id,
        _ => query.employee_id,
    };
    let year = query.year.unwrap_or_else(|| Utc::now().year());
    let rows = sqlx::query_as::<_, LeaveBalance>(
        r#"select b.employee_id, b.leave_type_id, lt.code as leave_code, lt.name as leave_name,
                  b.year, b.accrued, b.used, b.available
           from leave_balances b join leave_types lt on lt.id = b.leave_type_id
           where ($1::uuid is null or b.employee_id = $1) and b.year = $2
           order by leave_name"#,
    )
    .bind(employee_id)
    .bind(year)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct LeaveRequest {
    pub id: Uuid,
    pub employee_id: Uuid,
    pub employee_name: String,
    pub leave_code: String,
    pub from_date: NaiveDate,
    pub to_date: NaiveDate,
    pub days: Decimal,
    pub reason: Option<String>,
    pub status: String,
    pub approver_id: Option<Uuid>,
    pub decided_at: Option<chrono::DateTime<Utc>>,
    pub created_at: chrono::DateTime<Utc>,
}

const LEAVE_REQUEST_SELECT: &str = r#"
select r.id, r.employee_id, (e.first_name || ' ' || e.last_name) as employee_name,
       lt.code as leave_code, r.from_date, r.to_date, r.days, r.reason, r.status,
       r.approver_id, r.decided_at, r.created_at
from leave_requests r
join employees e on e.id = r.employee_id
join leave_types lt on lt.id = r.leave_type_id
"#;

#[derive(Debug, Deserialize, ToSchema)]
pub struct LeaveQuery {
    pub employee_id: Option<Uuid>,
    pub status: Option<String>,
    pub pending_for_manager: Option<bool>,
}

#[utoipa::path(get, path = "/api/leave/requests", tag = "absence",
    params(("employee_id" = Option<Uuid>, Query), ("status" = Option<String>, Query),
           ("pending_for_manager" = Option<bool>, Query)),
    responses((status = 200, body = Vec<LeaveRequest>)))]
pub async fn leave_requests_list(
    user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Query(query): Query<LeaveQuery>,
) -> ApiResult<Json<Vec<LeaveRequest>>> {
    let scoped = match user.session.role {
        hrms_core::Role::Agent => user.session.employee_id,
        _ => query.employee_id,
    };
    let manager = if query.pending_for_manager.unwrap_or(false) {
        user.session.employee_id
    } else {
        None
    };
    let sql = format!(
        "{LEAVE_REQUEST_SELECT}
         where ($1::uuid is null or r.employee_id = $1)
           and ($2::text is null or r.status = $2)
           and ($3::uuid is null or e.manager_id = $3)
         order by r.created_at desc limit 300"
    );
    let rows = sqlx::query_as::<_, LeaveRequest>(&sql)
        .bind(scoped)
        .bind(query.status)
        .bind(manager)
        .fetch_all(&state.db)
        .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct LeaveRequestInput {
    pub employee_id: Option<Uuid>,
    pub leave_code: String,
    pub from_date: NaiveDate,
    pub to_date: NaiveDate,
    pub reason: Option<String>,
}

/// Apply for leave. Days are calendar days in the range minus declared
/// holidays; the balance is checked before the request is accepted.
#[utoipa::path(post, path = "/api/leave/requests", tag = "absence", request_body = LeaveRequestInput,
    responses((status = 200, body = LeaveRequest), (status = 400, description = "insufficient balance / bad range")))]
pub async fn leave_requests_create(
    user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Json(input): Json<LeaveRequestInput>,
) -> ApiResult<Json<LeaveRequest>> {
    if input.to_date < input.from_date {
        return Err(ApiError::bad("to_date must be on or after from_date"));
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

    let leave_type: Option<(Uuid, bool)> =
        sqlx::query_as("select id, paid from leave_types where code = $1")
            .bind(&input.leave_code)
            .fetch_optional(&state.db)
            .await?;
    let (leave_type_id, paid) =
        leave_type.ok_or_else(|| ApiError::bad("unknown leave_code"))?;

    let holiday_count: i64 = sqlx::query_scalar(
        "select count(*) from holidays where holiday_date between $1 and $2",
    )
    .bind(input.from_date)
    .bind(input.to_date)
    .fetch_one(&state.db)
    .await?;
    let calendar_days = (input.to_date - input.from_date).num_days() + 1 - holiday_count;
    if calendar_days <= 0 {
        return Err(ApiError::bad("the selected range contains no working days"));
    }
    let days = Decimal::from(calendar_days);

    if paid {
        let available: Option<Decimal> = sqlx::query_scalar(
            "select available from leave_balances
             where employee_id = $1 and leave_type_id = $2 and year = $3",
        )
        .bind(employee_id)
        .bind(leave_type_id)
        .bind(input.from_date.year())
        .fetch_optional(&state.db)
        .await?;
        match available {
            Some(balance) if balance >= days => {}
            Some(balance) => {
                return Err(ApiError::bad(format!(
                    "insufficient {code} balance: requested {days}, available {balance}",
                    code = input.leave_code
                )))
            }
            None => {
                return Err(ApiError::bad(
                    "no leave balance exists for this type/year; run the accrual job first",
                ))
            }
        }
    }

    let id: Uuid = sqlx::query_scalar(
        "insert into leave_requests (employee_id, leave_type_id, from_date, to_date, days, reason)
         values ($1,$2,$3,$4,$5,$6) returning id",
    )
    .bind(employee_id)
    .bind(leave_type_id)
    .bind(input.from_date)
    .bind(input.to_date)
    .bind(days)
    .bind(&input.reason)
    .fetch_one(&state.db)
    .await?;

    let row = sqlx::query_as::<_, LeaveRequest>(&format!("{LEAVE_REQUEST_SELECT} where r.id = $1"))
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    Ok(Json(row))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct LeaveDecision {
    pub approve: bool,
    pub note: Option<String>,
}

#[utoipa::path(post, path = "/api/leave/requests/{id}/decide", tag = "absence",
    params(("id" = Uuid, Path)), request_body = LeaveDecision,
    responses((status = 200, body = LeaveRequest)))]
pub async fn leave_decide(
    user: AuthUser<Supervisor>,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(input): Json<LeaveDecision>,
) -> ApiResult<Json<LeaveRequest>> {
    #[derive(FromRow)]
    struct Pending {
        employee_id: Uuid,
        leave_type_id: Uuid,
        from_date: NaiveDate,
        days: Decimal,
        status: String,
    }
    let pending = sqlx::query_as::<_, Pending>(
        "select employee_id, leave_type_id, from_date, days, status from leave_requests where id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| ApiError::not_found("leave request"))?;
    if pending.status != "pending" {
        return Err(ApiError::bad("this request has already been decided"));
    }

    let status = if input.approve { "approved" } else { "rejected" };
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "update leave_requests set status = $2, approver_id = $3, decided_at = now() where id = $1",
    )
    .bind(id)
    .bind(status)
    .bind(user.session.employee_id)
    .execute(&mut *tx)
    .await?;

    if input.approve {
        sqlx::query(
            r#"update leave_balances
               set used = used + $4, available = available - $4, updated_at = now()
               where employee_id = $1 and leave_type_id = $2 and year = $3"#,
        )
        .bind(pending.employee_id)
        .bind(pending.leave_type_id)
        .bind(pending.from_date.year())
        .bind(pending.days)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "insert into leave_accrual_ledger (employee_id, leave_type_id, period, delta, reason)
             values ($1,$2,$3,$4,$5)",
        )
        .bind(pending.employee_id)
        .bind(pending.leave_type_id)
        .bind(pending.from_date)
        .bind(-pending.days)
        .bind(format!("leave taken (request {id})"))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    state
        .audit(Some(&user.session), status, "leave_request", Some(id.to_string()), serde_json::json!({"note": input.note}))
        .await;

    let row = sqlx::query_as::<_, LeaveRequest>(&format!("{LEAVE_REQUEST_SELECT} where r.id = $1"))
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    Ok(Json(row))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LeaveForecast {
    pub employee_id: Uuid,
    pub leave_code: String,
    pub as_of: NaiveDate,
    pub current_available: Decimal,
    pub projected_accrual: Decimal,
    pub approved_future_days: Decimal,
    pub projected_available: Decimal,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ForecastQuery {
    pub employee_id: Option<Uuid>,
    pub as_of: Option<NaiveDate>,
}

/// Forecast balances at a future date: accruals still to come minus approved
/// leave already booked.
#[utoipa::path(get, path = "/api/leave/forecast", tag = "absence",
    params(("employee_id" = Option<Uuid>, Query), ("as_of" = Option<NaiveDate>, Query)),
    responses((status = 200, body = Vec<LeaveForecast>)))]
pub async fn leave_forecast(
    user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Query(query): Query<ForecastQuery>,
) -> ApiResult<Json<Vec<LeaveForecast>>> {
    let employee_id = match user.session.role {
        hrms_core::Role::Agent => user.session.employee_id,
        _ => query.employee_id,
    }
    .ok_or_else(|| ApiError::bad("employee_id is required"))?;
    let as_of = query.as_of.unwrap_or_else(|| Utc::now().date_naive());
    let today = Utc::now().date_naive();
    if as_of < today {
        return Err(ApiError::bad("as_of must be today or later"));
    }
    let months_ahead = ((as_of.year() - today.year()) * 12 + as_of.month() as i32
        - today.month() as i32)
        .max(0);

    #[derive(FromRow)]
    struct Row {
        leave_code: String,
        monthly_accrual: Decimal,
        available: Decimal,
        max_balance: Option<Decimal>,
    }
    let rows = sqlx::query_as::<_, Row>(
        r#"select lt.code as leave_code, lt.monthly_accrual, coalesce(b.available,0) as available,
                  lt.max_balance
           from leave_types lt
           left join leave_balances b
             on b.leave_type_id = lt.id and b.employee_id = $1 and b.year = $2
           order by lt.name"#,
    )
    .bind(employee_id)
    .bind(today.year())
    .fetch_all(&state.db)
    .await?;

    let mut forecast = Vec::new();
    for row in rows {
        let projected_accrual = row.monthly_accrual * Decimal::from(months_ahead);
        let booked: Option<Decimal> = sqlx::query_scalar(
            r#"select coalesce(sum(r.days),0) from leave_requests r
               join leave_types lt on lt.id = r.leave_type_id
               where r.employee_id = $1 and lt.code = $2 and r.status = 'approved'
                 and r.from_date > $3 and r.from_date <= $4"#,
        )
        .bind(employee_id)
        .bind(&row.leave_code)
        .bind(today)
        .bind(as_of)
        .fetch_one(&state.db)
        .await?;
        let booked = booked.unwrap_or(Decimal::ZERO);
        let mut projected = row.available + projected_accrual - booked;
        if let Some(cap) = row.max_balance {
            if projected > cap {
                projected = cap;
            }
        }
        forecast.push(LeaveForecast {
            employee_id,
            leave_code: row.leave_code,
            as_of,
            current_available: row.available,
            projected_accrual,
            approved_future_days: booked,
            projected_available: projected,
        });
    }
    Ok(Json(forecast))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AccrualInput {
    pub year: i32,
    pub month: u32,
}

/// Post monthly leave accruals for every active employee. Idempotent: an
/// employee/type/period that already has an accrual ledger row is skipped.
#[utoipa::path(post, path = "/api/leave/accrual/run", tag = "absence", request_body = AccrualInput,
    responses((status = 200, description = "accrual posted")))]
pub async fn leave_accrual_run(
    user: AuthUser<Supervisor>,
    State(state): State<AppState>,
    Json(input): Json<AccrualInput>,
) -> ApiResult<Json<serde_json::Value>> {
    if !(1..=12).contains(&input.month) {
        return Err(ApiError::bad("month must be 1..12"));
    }
    let period = NaiveDate::from_ymd_opt(input.year, input.month, 1)
        .ok_or_else(|| ApiError::bad("invalid period"))?;
    let fiscal_year = period.year();

    #[derive(FromRow)]
    struct TypeRow {
        id: Uuid,
        monthly_accrual: Decimal,
    }
    let types = sqlx::query_as::<_, TypeRow>(
        "select id, monthly_accrual from leave_types where monthly_accrual > 0",
    )
    .fetch_all(&state.db)
    .await?;
    let employees: Vec<Uuid> = sqlx::query_scalar(
        "select id from employees where status in ('active','probation','on_leave')",
    )
    .fetch_all(&state.db)
    .await?;

    let mut posted = 0usize;
    for employee_id in &employees {
        for leave_type in &types {
            let already: Option<Uuid> = sqlx::query_scalar(
                "select id from leave_accrual_ledger
                 where employee_id = $1 and leave_type_id = $2 and period = $3 and reason = 'monthly accrual'",
            )
            .bind(employee_id)
            .bind(leave_type.id)
            .bind(period)
            .fetch_optional(&state.db)
            .await?;
            if already.is_some() {
                continue;
            }
            sqlx::query(
                r#"insert into leave_balances (employee_id, leave_type_id, year, accrued, used, available)
                   values ($1,$2,$3,$4,0,$4)
                   on conflict (employee_id, leave_type_id, year) do update
                   set accrued = leave_balances.accrued + $4,
                       available = leave_balances.available + $4,
                       updated_at = now()"#,
            )
            .bind(employee_id)
            .bind(leave_type.id)
            .bind(fiscal_year)
            .bind(leave_type.monthly_accrual)
            .execute(&state.db)
            .await?;
            sqlx::query(
                "insert into leave_accrual_ledger (employee_id, leave_type_id, period, delta, reason)
                 values ($1,$2,$3,$4,'monthly accrual')",
            )
            .bind(employee_id)
            .bind(leave_type.id)
            .bind(period)
            .bind(leave_type.monthly_accrual)
            .execute(&state.db)
            .await?;
            posted += 1;
        }
    }

    state
        .audit(
            Some(&user.session),
            "accrual_run",
            "leave_balance",
            None,
            serde_json::json!({"period": period, "posted": posted}),
        )
        .await;
    Ok(Json(serde_json::json!({
        "period": period,
        "employees": employees.len(),
        "leave_types": types.len(),
        "posted": posted,
    })))
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct Holiday {
    pub id: Uuid,
    pub holiday_date: NaiveDate,
    pub name: String,
    pub location_id: Option<Uuid>,
}

#[utoipa::path(get, path = "/api/holidays", tag = "absence", responses((status = 200, body = Vec<Holiday>)))]
pub async fn holidays_list(
    _user: AuthUser<AnyRole>,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<Holiday>>> {
    let rows = sqlx::query_as::<_, Holiday>(
        "select id, holiday_date, name, location_id from holidays order by holiday_date",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}
