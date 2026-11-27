//! Benefits Administration — plans, dependent records and enrollments linked
//! to the `BENEFIT_DEDUCTION` payroll component.

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::auth::{AnyRole, AuthUser, BenefitsOffice};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/benefits/plans", get(plans_list).post(plans_create))
        .route("/benefits/enrollments", get(enrollments_list).post(enrollments_create))
        .route("/benefits/enrollments/{id}/terminate", post(enrollments_terminate))
        .route("/benefits/dependents", get(dependents_list).post(dependents_create))
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct BenefitPlan {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    pub provider: Option<String>,
    pub category: String,
    pub employee_cost: Decimal,
    pub employer_cost: Decimal,
    pub coverage_amount: Decimal,
    pub payroll_component: String,
    pub description: Option<String>,
    pub active: bool,
}

#[utoipa::path(get, path = "/api/benefits/plans", tag = "benefits", responses((status = 200, body = Vec<BenefitPlan>)))]
pub async fn plans_list(
    _user: AuthUser<AnyRole>,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<BenefitPlan>>> {
    let rows = sqlx::query_as::<_, BenefitPlan>(
        "select id, code, name, provider, category, employee_cost, employer_cost, coverage_amount,
                payroll_component, description, active
         from benefit_plans order by category, name",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PlanInput {
    pub code: String,
    pub name: String,
    pub provider: Option<String>,
    pub category: Option<String>,
    pub employee_cost: Decimal,
    pub employer_cost: Decimal,
    pub coverage_amount: Decimal,
    pub description: Option<String>,
}

#[utoipa::path(post, path = "/api/benefits/plans", tag = "benefits", request_body = PlanInput,
    responses((status = 200, body = BenefitPlan)))]
pub async fn plans_create(
    _user: AuthUser<BenefitsOffice>,
    State(state): State<AppState>,
    Json(input): Json<PlanInput>,
) -> ApiResult<Json<BenefitPlan>> {
    let row = sqlx::query_as::<_, BenefitPlan>(
        "insert into benefit_plans (code, name, provider, category, employee_cost, employer_cost, coverage_amount, description)
         values ($1,$2,$3,coalesce($4,'health'),$5,$6,$7,$8)
         returning id, code, name, provider, category, employee_cost, employer_cost, coverage_amount,
                   payroll_component, description, active",
    )
    .bind(&input.code)
    .bind(&input.name)
    .bind(&input.provider)
    .bind(&input.category)
    .bind(input.employee_cost)
    .bind(input.employer_cost)
    .bind(input.coverage_amount)
    .bind(&input.description)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(row))
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct Enrollment {
    pub id: Uuid,
    pub employee_id: Uuid,
    pub employee_name: String,
    pub plan_code: String,
    pub plan_name: String,
    pub effective_from: NaiveDate,
    pub effective_to: Option<NaiveDate>,
    pub employee_cost: Decimal,
    pub employer_cost: Decimal,
    pub dependents_covered: i32,
    pub status: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct EnrollmentQuery {
    pub employee_id: Option<Uuid>,
}

#[utoipa::path(get, path = "/api/benefits/enrollments", tag = "benefits",
    params(("employee_id" = Option<Uuid>, Query)),
    responses((status = 200, body = Vec<Enrollment>)))]
pub async fn enrollments_list(
    user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Query(query): Query<EnrollmentQuery>,
) -> ApiResult<Json<Vec<Enrollment>>> {
    let employee_id = match user.session.role {
        hrms_core::Role::Agent => user.session.employee_id,
        _ => query.employee_id,
    };
    let rows = sqlx::query_as::<_, Enrollment>(
        r#"select b.id, b.employee_id, (e.first_name || ' ' || e.last_name) as employee_name,
                  p.code as plan_code, p.name as plan_name, b.effective_from, b.effective_to,
                  b.employee_cost, b.employer_cost, b.dependents_covered, b.status
           from benefit_enrollments b
           join employees e on e.id = b.employee_id
           join benefit_plans p on p.id = b.plan_id
           where ($1::uuid is null or b.employee_id = $1)
           order by b.created_at desc limit 300"#,
    )
    .bind(employee_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct EnrollmentInput {
    pub employee_id: Option<Uuid>,
    pub plan_code: String,
    pub effective_from: Option<NaiveDate>,
    pub dependents_covered: Option<i32>,
}

/// Enrol an employee (or self) in a plan. The plan's employee cost becomes a
/// recurring payroll deduction through `payroll_component`.
#[utoipa::path(post, path = "/api/benefits/enrollments", tag = "benefits",
    request_body = EnrollmentInput, responses((status = 200, body = Enrollment)))]
pub async fn enrollments_create(
    user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Json(input): Json<EnrollmentInput>,
) -> ApiResult<Json<Enrollment>> {
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
    let plan: Option<(Uuid, Decimal, Decimal)> = sqlx::query_as(
        "select id, employee_cost, employer_cost from benefit_plans where code = $1 and active",
    )
    .bind(&input.plan_code)
    .fetch_optional(&state.db)
    .await?;
    let (plan_id, employee_cost, employer_cost) =
        plan.ok_or_else(|| ApiError::bad("unknown or inactive plan_code"))?;
    let effective_from = input.effective_from.unwrap_or_else(|| Utc::now().date_naive());

    let id: Uuid = sqlx::query_scalar(
        r#"insert into benefit_enrollments
           (employee_id, plan_id, effective_from, employee_cost, employer_cost, dependents_covered, status)
           values ($1,$2,$3,$4,$5,coalesce($6,0),'active')
           on conflict (employee_id, plan_id, effective_from) do update
             set employee_cost = excluded.employee_cost,
                 employer_cost = excluded.employer_cost,
                 dependents_covered = excluded.dependents_covered,
                 status = 'active'
           returning id"#,
    )
    .bind(employee_id)
    .bind(plan_id)
    .bind(effective_from)
    .bind(employee_cost)
    .bind(employer_cost)
    .bind(input.dependents_covered)
    .fetch_one(&state.db)
    .await?;

    let row = sqlx::query_as::<_, Enrollment>(
        r#"select b.id, b.employee_id, (e.first_name || ' ' || e.last_name) as employee_name,
                  p.code as plan_code, p.name as plan_name, b.effective_from, b.effective_to,
                  b.employee_cost, b.employer_cost, b.dependents_covered, b.status
           from benefit_enrollments b
           join employees e on e.id = b.employee_id
           join benefit_plans p on p.id = b.plan_id
           where b.id = $1"#,
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    state
        .audit(Some(&user.session), "enroll", "benefit", Some(id.to_string()), serde_json::json!({"plan": input.plan_code}))
        .await;
    Ok(Json(row))
}

#[utoipa::path(post, path = "/api/benefits/enrollments/{id}/terminate", tag = "benefits",
    params(("id" = Uuid, Path)), responses((status = 200, body = Enrollment)))]
pub async fn enrollments_terminate(
    user: AuthUser<BenefitsOffice>,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Enrollment>> {
    sqlx::query("update benefit_enrollments set status = 'terminated', effective_to = current_date where id = $1")
        .bind(id)
        .execute(&state.db)
        .await?;
    let row = sqlx::query_as::<_, Enrollment>(
        r#"select b.id, b.employee_id, (e.first_name || ' ' || e.last_name) as employee_name,
                  p.code as plan_code, p.name as plan_name, b.effective_from, b.effective_to,
                  b.employee_cost, b.employer_cost, b.dependents_covered, b.status
           from benefit_enrollments b
           join employees e on e.id = b.employee_id
           join benefit_plans p on p.id = b.plan_id
           where b.id = $1"#,
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| ApiError::not_found("enrollment"))?;
    Ok(Json(row))
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct Dependent {
    pub id: Uuid,
    pub employee_id: Uuid,
    pub name: String,
    pub relation: String,
    pub date_of_birth: Option<NaiveDate>,
    pub aadhaar_last4: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct DependentQuery {
    pub employee_id: Option<Uuid>,
}

#[utoipa::path(get, path = "/api/benefits/dependents", tag = "benefits",
    params(("employee_id" = Option<Uuid>, Query)), responses((status = 200, body = Vec<Dependent>)))]
pub async fn dependents_list(
    user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Query(query): Query<DependentQuery>,
) -> ApiResult<Json<Vec<Dependent>>> {
    let employee_id = match user.session.role {
        hrms_core::Role::Agent => user.session.employee_id,
        _ => query.employee_id,
    };
    let rows = sqlx::query_as::<_, Dependent>(
        "select id, employee_id, name, relation, date_of_birth, aadhaar_last4
         from dependents where ($1::uuid is null or employee_id = $1) order by name",
    )
    .bind(employee_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct DependentInput {
    pub employee_id: Option<Uuid>,
    pub name: String,
    pub relation: String,
    pub date_of_birth: Option<NaiveDate>,
    pub aadhaar_last4: Option<String>,
}

#[utoipa::path(post, path = "/api/benefits/dependents", tag = "benefits",
    request_body = DependentInput, responses((status = 200, body = Dependent)))]
pub async fn dependents_create(
    user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Json(input): Json<DependentInput>,
) -> ApiResult<Json<Dependent>> {
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
    let row = sqlx::query_as::<_, Dependent>(
        "insert into dependents (employee_id, name, relation, date_of_birth, aadhaar_last4)
         values ($1,$2,$3,$4,$5)
         returning id, employee_id, name, relation, date_of_birth, aadhaar_last4",
    )
    .bind(employee_id)
    .bind(&input.name)
    .bind(&input.relation)
    .bind(input.date_of_birth)
    .bind(&input.aadhaar_last4)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(row))
}
