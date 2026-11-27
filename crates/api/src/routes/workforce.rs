//! Workforce Data Management — departments, locations, positions, person/job
//! records, onboarding activity guides and global assignments.

use axum::extract::{Multipart, Path, Query, State};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use utoipa::ToSchema;
use uuid::Uuid;
use validator::Validate;

use crate::auth::{AnyRole, AuthUser, PeopleAdmin, Supervisor};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/departments", get(departments_list).post(departments_create))
        .route("/locations", get(locations_list).post(locations_create))
        .route("/positions", get(positions_list).post(positions_create))
        .route("/employees", get(employees_list).post(employees_create))
        .route("/employees/import", post(employees_import))
        .route("/employees/{id}", get(employees_get).patch(employees_update))
        .route("/employees/{id}/terminate", post(employees_terminate))
        .route("/employees/{id}/job-history", get(job_history_list).post(job_history_create))
        .route("/employees/{id}/onboarding", get(onboarding_list).post(onboarding_create))
        .route("/onboarding/{id}/complete", post(onboarding_complete))
        .route("/employees/{id}/assignments", get(assignments_list).post(assignments_create))
        .route("/team/members", get(team_members))
}

// ---------------------------------------------------------------------------
// Departments / locations / positions
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct Department {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    pub cost_center: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct DepartmentInput {
    pub code: String,
    pub name: String,
    pub cost_center: Option<String>,
}

#[utoipa::path(get, path = "/api/departments", tag = "workforce", responses((status = 200, body = Vec<Department>)))]
pub async fn departments_list(
    _user: AuthUser<AnyRole>,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<Department>>> {
    let rows = sqlx::query_as::<_, Department>(
        "select id, code, name, cost_center from departments order by name",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[utoipa::path(post, path = "/api/departments", tag = "workforce", request_body = DepartmentInput, responses((status = 200, body = Department)))]
pub async fn departments_create(
    user: AuthUser<PeopleAdmin>,
    State(state): State<AppState>,
    Json(input): Json<DepartmentInput>,
) -> ApiResult<Json<Department>> {
    let row = sqlx::query_as::<_, Department>(
        "insert into departments (code, name, cost_center) values ($1,$2,$3)
         returning id, code, name, cost_center",
    )
    .bind(&input.code)
    .bind(&input.name)
    .bind(&input.cost_center)
    .fetch_one(&state.db)
    .await?;
    state
        .audit(Some(&user.session), "create", "department", Some(row.id.to_string()), serde_json::json!({"code": row.code}))
        .await;
    Ok(Json(row))
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct Location {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    pub city: String,
    pub state_code: String,
    pub timezone: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct LocationInput {
    pub code: String,
    pub name: String,
    pub city: String,
    pub state_code: String,
}

#[utoipa::path(get, path = "/api/locations", tag = "workforce", responses((status = 200, body = Vec<Location>)))]
pub async fn locations_list(
    _user: AuthUser<AnyRole>,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<Location>>> {
    let rows = sqlx::query_as::<_, Location>(
        "select id, code, name, city, state_code, timezone from locations order by name",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[utoipa::path(post, path = "/api/locations", tag = "workforce", request_body = LocationInput, responses((status = 200, body = Location)))]
pub async fn locations_create(
    _user: AuthUser<PeopleAdmin>,
    State(state): State<AppState>,
    Json(input): Json<LocationInput>,
) -> ApiResult<Json<Location>> {
    if input.state_code.len() != 2 {
        return Err(ApiError::bad("state_code must be a two-letter code (MH, KA, WB, TN)"));
    }
    let row = sqlx::query_as::<_, Location>(
        "insert into locations (code, name, city, state_code) values ($1,$2,$3,upper($4))
         returning id, code, name, city, state_code, timezone",
    )
    .bind(&input.code)
    .bind(&input.name)
    .bind(&input.city)
    .bind(&input.state_code)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(row))
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct Position {
    pub id: Uuid,
    pub code: String,
    pub title: String,
    pub department_id: Option<Uuid>,
    pub location_id: Option<Uuid>,
    pub fte: rust_decimal::Decimal,
    pub budgeted_annual_salary: rust_decimal::Decimal,
    pub status: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PositionInput {
    pub code: String,
    pub title: String,
    pub department_id: Option<Uuid>,
    pub location_id: Option<Uuid>,
    pub reports_to_position_id: Option<Uuid>,
    pub fte: Option<rust_decimal::Decimal>,
    pub budgeted_annual_salary: Option<rust_decimal::Decimal>,
}

#[utoipa::path(get, path = "/api/positions", tag = "workforce", responses((status = 200, body = Vec<Position>)))]
pub async fn positions_list(
    _user: AuthUser<AnyRole>,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<Position>>> {
    let rows = sqlx::query_as::<_, Position>(
        "select id, code, title, department_id, location_id, fte, budgeted_annual_salary, status
         from positions order by title",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[utoipa::path(post, path = "/api/positions", tag = "workforce", request_body = PositionInput, responses((status = 200, body = Position)))]
pub async fn positions_create(
    user: AuthUser<PeopleAdmin>,
    State(state): State<AppState>,
    Json(input): Json<PositionInput>,
) -> ApiResult<Json<Position>> {
    let row = sqlx::query_as::<_, Position>(
        "insert into positions (code, title, department_id, location_id, reports_to_position_id, fte, budgeted_annual_salary)
         values ($1,$2,$3,$4,$5,coalesce($6,1.0),coalesce($7,0))
         returning id, code, title, department_id, location_id, fte, budgeted_annual_salary, status",
    )
    .bind(&input.code)
    .bind(&input.title)
    .bind(input.department_id)
    .bind(input.location_id)
    .bind(input.reports_to_position_id)
    .bind(input.fte)
    .bind(input.budgeted_annual_salary)
    .fetch_one(&state.db)
    .await?;
    state
        .audit(Some(&user.session), "create", "position", Some(row.id.to_string()), serde_json::json!({"code": row.code}))
        .await;
    Ok(Json(row))
}

// ---------------------------------------------------------------------------
// Employees
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct EmployeeRecord {
    pub id: Uuid,
    pub emp_no: String,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub phone: Option<String>,
    pub gender: String,
    pub date_of_joining: NaiveDate,
    pub date_of_exit: Option<NaiveDate>,
    pub status: String,
    pub employment_type: String,
    pub position_id: Option<Uuid>,
    pub manager_id: Option<Uuid>,
    pub department_id: Option<Uuid>,
    pub location_id: Option<Uuid>,
    pub work_state: String,
    pub pan: Option<String>,
    pub uan: Option<String>,
    pub esic_ip: Option<String>,
    pub pf_exempt: bool,
    pub esi_exempt: bool,
    pub tax_regime: String,
    pub disability: bool,
    pub bank_account: Option<String>,
    pub ifsc: Option<String>,
    pub extension: Option<String>,
    pub department_name: Option<String>,
    pub position_title: Option<String>,
    pub manager_name: Option<String>,
}

const EMPLOYEE_SELECT: &str = r#"
select e.id, e.emp_no, e.first_name, e.last_name, e.email, e.phone, e.gender,
       e.date_of_joining, e.date_of_exit, e.status, e.employment_type,
       e.position_id, e.manager_id, e.department_id, e.location_id, e.work_state,
       e.pan, e.uan, e.esic_ip, e.pf_exempt, e.esi_exempt, e.tax_regime, e.disability,
       e.bank_account, e.ifsc, e.extension,
       d.name as department_name,
       p.title as position_title,
       (m.first_name || ' ' || m.last_name) as manager_name
from employees e
left join departments d on d.id = e.department_id
left join positions p on p.id = e.position_id
left join employees m on m.id = e.manager_id
"#;

#[derive(Debug, Deserialize, ToSchema)]
pub struct ListEmployeesQuery {
    pub q: Option<String>,
    pub department_id: Option<Uuid>,
    pub status: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// List person records with optional search/filter.
#[utoipa::path(get, path = "/api/employees", tag = "workforce",
    params(("q" = Option<String>, Query, description = "name / emp no / email contains"),
           ("department_id" = Option<Uuid>, Query),
           ("status" = Option<String>, Query),
           ("limit" = Option<i64>, Query), ("offset" = Option<i64>, Query)),
    responses((status = 200, body = Vec<EmployeeRecord>)))]
pub async fn employees_list(
    _user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Query(query): Query<ListEmployeesQuery>,
) -> ApiResult<Json<Vec<EmployeeRecord>>> {
    let sql = format!(
        "{EMPLOYEE_SELECT}
         where ($1::text is null or e.emp_no ilike '%'||$1||'%'
                or e.email ilike '%'||$1||'%'
                or (e.first_name || ' ' || e.last_name) ilike '%'||$1||'%')
           and ($2::uuid is null or e.department_id = $2)
           and ($3::text is null or e.status = $3)
         order by e.emp_no
         limit $4 offset $5"
    );
    let rows = sqlx::query_as::<_, EmployeeRecord>(&sql)
        .bind(query.q.clone())
        .bind(query.department_id)
        .bind(query.status.clone())
        .bind(query.limit.unwrap_or(200).clamp(1, 1000))
        .bind(query.offset.unwrap_or(0).max(0))
        .fetch_all(&state.db)
        .await?;
    Ok(Json(rows))
}

#[utoipa::path(get, path = "/api/employees/{id}", tag = "workforce",
    params(("id" = Uuid, Path)),
    responses((status = 200, body = EmployeeRecord), (status = 404)))]
pub async fn employees_get(
    _user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<EmployeeRecord>> {
    let sql = format!("{EMPLOYEE_SELECT} where e.id = $1");
    let row = sqlx::query_as::<_, EmployeeRecord>(&sql)
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| ApiError::not_found("employee"))?;
    Ok(Json(row))
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct EmployeeInput {
    #[validate(length(min = 1))]
    pub emp_no: String,
    #[validate(length(min = 1))]
    pub first_name: String,
    #[validate(length(min = 1))]
    pub last_name: String,
    #[validate(email)]
    pub email: String,
    pub phone: Option<String>,
    pub date_of_birth: Option<NaiveDate>,
    pub gender: Option<String>,
    pub date_of_joining: NaiveDate,
    pub employment_type: Option<String>,
    pub position_id: Option<Uuid>,
    pub manager_id: Option<Uuid>,
    pub department_id: Option<Uuid>,
    pub location_id: Option<Uuid>,
    pub work_state: Option<String>,
    pub pan: Option<String>,
    pub uan: Option<String>,
    pub esic_ip: Option<String>,
    pub tax_regime: Option<String>,
    pub disability: Option<bool>,
    pub bank_account: Option<String>,
    pub ifsc: Option<String>,
    pub extension: Option<String>,
}

/// Create a person record.
#[utoipa::path(post, path = "/api/employees", tag = "workforce", request_body = EmployeeInput,
    responses((status = 200, body = EmployeeRecord), (status = 409, description = "duplicate emp_no/email")))]
pub async fn employees_create(
    user: AuthUser<PeopleAdmin>,
    State(state): State<AppState>,
    Json(input): Json<EmployeeInput>,
) -> ApiResult<Json<EmployeeRecord>> {
    input
        .validate()
        .map_err(|e| ApiError::bad(format!("{e}")))?;
    let work_state = input
        .work_state
        .clone()
        .unwrap_or_else(|| "KA".to_string())
        .to_uppercase();

    let id: Uuid = sqlx::query_scalar(
        r#"insert into employees
           (emp_no, first_name, last_name, email, phone, date_of_birth, gender, date_of_joining,
            employment_type, position_id, manager_id, department_id, location_id, work_state,
            pan, uan, esic_ip, tax_regime, disability, bank_account, ifsc, extension)
           values ($1,$2,$3,$4,$5,$6,coalesce($7,'other'),$8,coalesce($9,'full_time'),$10,$11,$12,$13,$14,
                   $15,$16,$17,coalesce($18,'new'),coalesce($19,false),$20,$21,$22)
           returning id"#,
    )
    .bind(&input.emp_no)
    .bind(&input.first_name)
    .bind(&input.last_name)
    .bind(&input.email)
    .bind(&input.phone)
    .bind(input.date_of_birth)
    .bind(&input.gender)
    .bind(input.date_of_joining)
    .bind(&input.employment_type)
    .bind(input.position_id)
    .bind(input.manager_id)
    .bind(input.department_id)
    .bind(input.location_id)
    .bind(&work_state)
    .bind(&input.pan)
    .bind(&input.uan)
    .bind(&input.esic_ip)
    .bind(&input.tax_regime)
    .bind(input.disability)
    .bind(&input.bank_account)
    .bind(&input.ifsc)
    .bind(&input.extension)
    .fetch_one(&state.db)
    .await?;

    sqlx::query(
        "insert into job_history (employee_id, position_id, effective_date, action, reason)
         values ($1,$2,$3,'HIRE','Joined the organisation')",
    )
    .bind(id)
    .bind(input.position_id)
    .bind(input.date_of_joining)
    .execute(&state.db)
    .await?;

    // Default onboarding activity guide.
    for (title, category, days) in [
        ("Signed offer letter and NDA", "documentation", 0),
        ("PAN, Aadhaar and bank details verified", "documentation", 2),
        ("PF / ESI nomination submitted", "statutory", 7),
        ("System access and telephony extension provisioned", "it", 1),
        ("Privacy and POSH training completed", "training", 14),
        ("Nesting / product training sign-off", "training", 21),
    ] {
        sqlx::query(
            "insert into onboarding_tasks (employee_id, title, category, due_date)
             values ($1,$2,$3,$4 + $5::int)",
        )
        .bind(id)
        .bind(title)
        .bind(category)
        .bind(input.date_of_joining)
        .bind(days)
        .execute(&state.db)
        .await?;
    }

    state
        .audit(
            Some(&user.session),
            "create",
            "employee",
            Some(id.to_string()),
            serde_json::json!({"emp_no": input.emp_no}),
        )
        .await;

    let sql = format!("{EMPLOYEE_SELECT} where e.id = $1");
    let row = sqlx::query_as::<_, EmployeeRecord>(&sql)
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    Ok(Json(row))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct EmployeeUpdate {
    pub phone: Option<String>,
    pub email: Option<String>,
    pub position_id: Option<Uuid>,
    pub manager_id: Option<Uuid>,
    pub department_id: Option<Uuid>,
    pub location_id: Option<Uuid>,
    pub work_state: Option<String>,
    pub pan: Option<String>,
    pub uan: Option<String>,
    pub esic_ip: Option<String>,
    pub pf_exempt: Option<bool>,
    pub esi_exempt: Option<bool>,
    pub tax_regime: Option<String>,
    pub disability: Option<bool>,
    pub bank_account: Option<String>,
    pub ifsc: Option<String>,
    pub status: Option<String>,
    pub extension: Option<String>,
}

/// Update mutable person/job fields. Missing fields are left untouched.
#[utoipa::path(patch, path = "/api/employees/{id}", tag = "workforce",
    params(("id" = Uuid, Path)), request_body = EmployeeUpdate,
    responses((status = 200, body = EmployeeRecord)))]
pub async fn employees_update(
    user: AuthUser<PeopleAdmin>,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(input): Json<EmployeeUpdate>,
) -> ApiResult<Json<EmployeeRecord>> {
    let affected = sqlx::query(
        r#"update employees set
             phone = coalesce($2, phone),
             email = coalesce($3, email),
             position_id = coalesce($4, position_id),
             manager_id = coalesce($5, manager_id),
             department_id = coalesce($6, department_id),
             location_id = coalesce($7, location_id),
             work_state = coalesce(upper($8), work_state),
             pan = coalesce($9, pan),
             uan = coalesce($10, uan),
             esic_ip = coalesce($11, esic_ip),
             pf_exempt = coalesce($12, pf_exempt),
             esi_exempt = coalesce($13, esi_exempt),
             tax_regime = coalesce($14, tax_regime),
             disability = coalesce($15, disability),
             bank_account = coalesce($16, bank_account),
             ifsc = coalesce($17, ifsc),
             status = coalesce($18, status),
             extension = coalesce($19, extension),
             updated_at = now()
           where id = $1"#,
    )
    .bind(id)
    .bind(&input.phone)
    .bind(&input.email)
    .bind(input.position_id)
    .bind(input.manager_id)
    .bind(input.department_id)
    .bind(input.location_id)
    .bind(&input.work_state)
    .bind(&input.pan)
    .bind(&input.uan)
    .bind(&input.esic_ip)
    .bind(input.pf_exempt)
    .bind(input.esi_exempt)
    .bind(&input.tax_regime)
    .bind(input.disability)
    .bind(&input.bank_account)
    .bind(&input.ifsc)
    .bind(&input.status)
    .bind(&input.extension)
    .execute(&state.db)
    .await?
    .rows_affected();
    if affected == 0 {
        return Err(ApiError::not_found("employee"));
    }
    state
        .audit(Some(&user.session), "update", "employee", Some(id.to_string()), serde_json::json!({}))
        .await;
    let sql = format!("{EMPLOYEE_SELECT} where e.id = $1");
    let row = sqlx::query_as::<_, EmployeeRecord>(&sql)
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    Ok(Json(row))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct TerminateInput {
    pub exit_date: NaiveDate,
    pub reason: Option<String>,
}

/// Separate an employee (person record is retained for statutory retention).
#[utoipa::path(post, path = "/api/employees/{id}/terminate", tag = "workforce",
    params(("id" = Uuid, Path)), request_body = TerminateInput,
    responses((status = 200, body = EmployeeRecord)))]
pub async fn employees_terminate(
    user: AuthUser<PeopleAdmin>,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(input): Json<TerminateInput>,
) -> ApiResult<Json<EmployeeRecord>> {
    sqlx::query("update employees set status = 'separated', date_of_exit = $2, updated_at = now() where id = $1")
        .bind(id)
        .bind(input.exit_date)
        .execute(&state.db)
        .await?;
    sqlx::query(
        "insert into job_history (employee_id, effective_date, action, reason) values ($1,$2,'TERMINATION',$3)",
    )
    .bind(id)
    .bind(input.exit_date)
    .bind(input.reason.clone())
    .execute(&state.db)
    .await?;
    state
        .audit(Some(&user.session), "terminate", "employee", Some(id.to_string()), serde_json::json!({"exit_date": input.exit_date}))
        .await;
    let sql = format!("{EMPLOYEE_SELECT} where e.id = $1");
    let row = sqlx::query_as::<_, EmployeeRecord>(&sql)
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    Ok(Json(row))
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct TeamMember {
    pub id: Uuid,
    pub emp_no: String,
    pub name: String,
    pub email: String,
    pub status: String,
    pub work_state: String,
    pub extension: Option<String>,
}

/// Direct reports of the signed-in manager / team lead.
#[utoipa::path(get, path = "/api/team/members", tag = "workforce", responses((status = 200, body = Vec<TeamMember>)))]
pub async fn team_members(
    user: AuthUser<Supervisor>,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<TeamMember>>> {
    let manager_id = user.session.employee_id;
    let rows = sqlx::query_as::<_, TeamMember>(
        r#"select id, emp_no, (first_name || ' ' || last_name) as name, email, status, work_state, extension
           from employees
           where ($1::uuid is null or manager_id = $1) and status <> 'separated'
           order by emp_no"#,
    )
    .bind(manager_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

// ---------------------------------------------------------------------------
// Job history / onboarding / global assignments
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct JobHistoryEntry {
    pub id: Uuid,
    pub employee_id: Uuid,
    pub position_id: Option<Uuid>,
    pub effective_date: NaiveDate,
    pub action: String,
    pub reason: Option<String>,
    pub annual_salary: Option<rust_decimal::Decimal>,
}

#[utoipa::path(get, path = "/api/employees/{id}/job-history", tag = "workforce",
    params(("id" = Uuid, Path)), responses((status = 200, body = Vec<JobHistoryEntry>)))]
pub async fn job_history_list(
    _user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<JobHistoryEntry>>> {
    let rows = sqlx::query_as::<_, JobHistoryEntry>(
        "select id, employee_id, position_id, effective_date, action, reason, annual_salary
         from job_history where employee_id = $1 order by effective_date desc",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct JobHistoryInput {
    pub position_id: Option<Uuid>,
    pub effective_date: NaiveDate,
    pub action: String,
    pub reason: Option<String>,
    pub annual_salary: Option<rust_decimal::Decimal>,
}

#[utoipa::path(post, path = "/api/employees/{id}/job-history", tag = "workforce",
    params(("id" = Uuid, Path)), request_body = JobHistoryInput,
    responses((status = 200, body = JobHistoryEntry)))]
pub async fn job_history_create(
    user: AuthUser<PeopleAdmin>,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(input): Json<JobHistoryInput>,
) -> ApiResult<Json<JobHistoryEntry>> {
    let row = sqlx::query_as::<_, JobHistoryEntry>(
        "insert into job_history (employee_id, position_id, effective_date, action, reason, annual_salary)
         values ($1,$2,$3,$4,$5,$6)
         returning id, employee_id, position_id, effective_date, action, reason, annual_salary",
    )
    .bind(id)
    .bind(input.position_id)
    .bind(input.effective_date)
    .bind(&input.action)
    .bind(&input.reason)
    .bind(input.annual_salary)
    .fetch_one(&state.db)
    .await?;
    if let Some(position_id) = input.position_id {
        sqlx::query("update employees set position_id = $2, updated_at = now() where id = $1")
            .bind(id)
            .bind(position_id)
            .execute(&state.db)
            .await?;
    }
    state
        .audit(Some(&user.session), "create", "job_history", Some(row.id.to_string()), serde_json::json!({"employee_id": id}))
        .await;
    Ok(Json(row))
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct OnboardingTask {
    pub id: Uuid,
    pub employee_id: Uuid,
    pub title: String,
    pub category: String,
    pub due_date: Option<NaiveDate>,
    pub status: String,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[utoipa::path(get, path = "/api/employees/{id}/onboarding", tag = "workforce",
    params(("id" = Uuid, Path)), responses((status = 200, body = Vec<OnboardingTask>)))]
pub async fn onboarding_list(
    _user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<OnboardingTask>>> {
    let rows = sqlx::query_as::<_, OnboardingTask>(
        "select id, employee_id, title, category, due_date, status, completed_at
         from onboarding_tasks where employee_id = $1 order by due_date nulls last, title",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct OnboardingTaskInput {
    pub title: String,
    pub category: Option<String>,
    pub due_date: Option<NaiveDate>,
    pub assigned_to: Option<Uuid>,
}

#[utoipa::path(post, path = "/api/employees/{id}/onboarding", tag = "workforce",
    params(("id" = Uuid, Path)), request_body = OnboardingTaskInput,
    responses((status = 200, body = OnboardingTask)))]
pub async fn onboarding_create(
    _user: AuthUser<PeopleAdmin>,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(input): Json<OnboardingTaskInput>,
) -> ApiResult<Json<OnboardingTask>> {
    let row = sqlx::query_as::<_, OnboardingTask>(
        "insert into onboarding_tasks (employee_id, title, category, due_date, assigned_to)
         values ($1,$2,coalesce($3,'documentation'),$4,$5)
         returning id, employee_id, title, category, due_date, status, completed_at",
    )
    .bind(id)
    .bind(&input.title)
    .bind(&input.category)
    .bind(input.due_date)
    .bind(input.assigned_to)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(row))
}

#[utoipa::path(post, path = "/api/onboarding/{id}/complete", tag = "workforce",
    params(("id" = Uuid, Path)), responses((status = 200, body = OnboardingTask)))]
pub async fn onboarding_complete(
    _user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<OnboardingTask>> {
    let row = sqlx::query_as::<_, OnboardingTask>(
        "update onboarding_tasks set status = 'completed', completed_at = now()
         where id = $1
         returning id, employee_id, title, category, due_date, status, completed_at",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| ApiError::not_found("onboarding task"))?;
    Ok(Json(row))
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct GlobalAssignment {
    pub id: Uuid,
    pub employee_id: Uuid,
    pub host_country: String,
    pub host_location: String,
    pub start_date: NaiveDate,
    pub end_date: Option<NaiveDate>,
    pub allowance_monthly: rust_decimal::Decimal,
    pub status: String,
}

#[utoipa::path(get, path = "/api/employees/{id}/assignments", tag = "workforce",
    params(("id" = Uuid, Path)), responses((status = 200, body = Vec<GlobalAssignment>)))]
pub async fn assignments_list(
    _user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<GlobalAssignment>>> {
    let rows = sqlx::query_as::<_, GlobalAssignment>(
        "select id, employee_id, host_country, host_location, start_date, end_date, allowance_monthly, status
         from global_assignments where employee_id = $1 order by start_date desc",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AssignmentInput {
    pub host_country: String,
    pub host_location: String,
    pub start_date: NaiveDate,
    pub end_date: Option<NaiveDate>,
    pub allowance_monthly: Option<rust_decimal::Decimal>,
}

#[utoipa::path(post, path = "/api/employees/{id}/assignments", tag = "workforce",
    params(("id" = Uuid, Path)), request_body = AssignmentInput,
    responses((status = 200, body = GlobalAssignment)))]
pub async fn assignments_create(
    _user: AuthUser<PeopleAdmin>,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(input): Json<AssignmentInput>,
) -> ApiResult<Json<GlobalAssignment>> {
    let row = sqlx::query_as::<_, GlobalAssignment>(
        "insert into global_assignments (employee_id, host_country, host_location, start_date, end_date, allowance_monthly, status)
         values ($1,upper($2),$3,$4,$5,coalesce($6,0),'planned')
         returning id, employee_id, host_country, host_location, start_date, end_date, allowance_monthly, status",
    )
    .bind(id)
    .bind(&input.host_country)
    .bind(&input.host_location)
    .bind(input.start_date)
    .bind(input.end_date)
    .bind(input.allowance_monthly)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(row))
}

// ---------------------------------------------------------------------------
// CSV bulk import (template: csv-templates/employee_import.csv)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, ToSchema)]
pub struct ImportResult {
    pub imported: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
}

/// Bulk employee import. Multipart field name: `file`.
#[utoipa::path(post, path = "/api/employees/import", tag = "workforce",
    request_body(content = String, content_type = "multipart/form-data", description = "CSV file field `file`"),
    responses((status = 200, body = ImportResult)))]
pub async fn employees_import(
    user: AuthUser<PeopleAdmin>,
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> ApiResult<Json<ImportResult>> {
    let mut imported = 0usize;
    let mut skipped = 0usize;
    let mut errors = Vec::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad(format!("multipart: {e}")))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let bytes = field
            .bytes()
            .await
            .map_err(|e| ApiError::bad(format!("file read: {e}")))?;
        let mut reader = csv::ReaderBuilder::new()
            .trim(csv::Trim::All)
            .flexible(true)
            .from_reader(bytes.as_ref());
        let headers = reader.headers().cloned().unwrap_or_default();

        for (index, record) in reader.records().enumerate() {
            let line = index + 2;
            let record = match record {
                Ok(r) => r,
                Err(err) => {
                    errors.push(format!("line {line}: {err}"));
                    skipped += 1;
                    continue;
                }
            };
            let get = |name: &str| -> Option<String> {
                headers
                    .iter()
                    .position(|c| c.eq_ignore_ascii_case(name))
                    .and_then(|i| record.get(i))
                    .map(|v| v.trim().to_string())
                    .filter(|v| !v.is_empty())
            };

            let emp_no = match get("emp_no") {
                Some(v) => v,
                None => {
                    errors.push(format!("line {line}: emp_no is required"));
                    skipped += 1;
                    continue;
                }
            };
            let first_name = get("first_name").unwrap_or_default();
            let last_name = get("last_name").unwrap_or_default();
            let email = get("email").unwrap_or_default();
            let doj = match get("date_of_joining")
                .and_then(|v| NaiveDate::parse_from_str(&v, "%Y-%m-%d").ok())
            {
                Some(d) => d,
                None => {
                    errors.push(format!("line {line}: date_of_joining must be YYYY-MM-DD"));
                    skipped += 1;
                    continue;
                }
            };

            let department_id: Option<Uuid> = match get("department_code") {
                Some(code) => sqlx::query_scalar("select id from departments where code = $1")
                    .bind(&code)
                    .fetch_optional(&state.db)
                    .await
                    .unwrap_or(None),
                None => None,
            };
            let position_id: Option<Uuid> = match get("position_code") {
                Some(code) => sqlx::query_scalar("select id from positions where code = $1")
                    .bind(&code)
                    .fetch_optional(&state.db)
                    .await
                    .unwrap_or(None),
                None => None,
            };

            let result = sqlx::query(
                r#"insert into employees
                   (emp_no, first_name, last_name, email, phone, date_of_joining, department_id,
                    position_id, work_state, pan, uan, extension, employment_type, gender)
                   values ($1,$2,$3,$4,$5,$6,$7,$8,upper(coalesce($9,'KA')),$10,$11,$12,
                           coalesce($13,'full_time'), coalesce($14,'other'))
                   on conflict (emp_no) do nothing"#,
            )
            .bind(&emp_no)
            .bind(&first_name)
            .bind(&last_name)
            .bind(&email)
            .bind(get("phone"))
            .bind(doj)
            .bind(department_id)
            .bind(position_id)
            .bind(get("work_state"))
            .bind(get("pan"))
            .bind(get("uan"))
            .bind(get("extension"))
            .bind(get("employment_type"))
            .bind(get("gender"))
            .execute(&state.db)
            .await;

            match result {
                Ok(done) if done.rows_affected() == 1 => imported += 1,
                Ok(_) => {
                    skipped += 1;
                    errors.push(format!("line {line}: emp_no {emp_no} already exists"));
                }
                Err(err) => {
                    skipped += 1;
                    errors.push(format!("line {line}: {err}"));
                }
            }
        }
    }

    state
        .audit(
            Some(&user.session),
            "import",
            "employee",
            None,
            serde_json::json!({"imported": imported, "skipped": skipped}),
        )
        .await;

    Ok(Json(ImportResult {
        imported,
        skipped,
        errors,
    }))
}
