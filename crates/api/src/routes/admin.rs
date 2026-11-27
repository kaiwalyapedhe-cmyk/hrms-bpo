//! Platform administration — user accounts, RBAC, audit trail and runtime
//! diagnostics (object storage, payroll year and record counts).

use axum::extract::{Path, Query, State};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use utoipa::ToSchema;
use uuid::Uuid;
use validator::Validate;

use crate::auth::{hash_password, AdminOnly, AuthUser};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/admin/users", get(users_list).post(users_create))
        .route("/admin/users/{id}", patch(users_update))
        .route("/admin/users/{id}/reset-password", post(users_reset_password))
        .route("/admin/audit", get(audit_list))
        .route("/admin/system", get(system_status))
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct UserAccount {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub role: String,
    pub active: bool,
    pub employee_id: Option<Uuid>,
    pub employee_no: Option<String>,
    pub last_login_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[utoipa::path(get, path = "/api/admin/users", tag = "admin", responses((status = 200, body = Vec<UserAccount>)))]
pub async fn users_list(
    _user: AuthUser<AdminOnly>,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<UserAccount>>> {
    let rows = sqlx::query_as::<_, UserAccount>(
        r#"select u.id, u.email, u.name, u.role, u.active, u.employee_id,
                  e.emp_no as employee_no, u.last_login_at, u.created_at
           from users u left join employees e on e.id = u.employee_id
           order by u.role, u.email"#,
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UserInput {
    #[validate(email)]
    pub email: String,
    pub name: String,
    pub role: String,
    pub employee_id: Option<Uuid>,
    #[validate(length(min = 10))]
    pub password: String,
}

#[utoipa::path(post, path = "/api/admin/users", tag = "admin", request_body = UserInput,
    responses((status = 200, body = UserAccount), (status = 400, description = "invalid role/password")))]
pub async fn users_create(
    user: AuthUser<AdminOnly>,
    State(state): State<AppState>,
    Json(input): Json<UserInput>,
) -> ApiResult<Json<UserAccount>> {
    input
        .validate()
        .map_err(|e| ApiError::bad(format!("{e}")))?;
    if hrms_core::Role::from_str_opt(&input.role).is_none() {
        return Err(ApiError::bad("unknown role"));
    }
    let hash = hash_password(&input.password)?;
    sqlx::query(
        "insert into users (email, name, role, employee_id, password_hash) values ($1,$2,$3,$4,$5)",
    )
    .bind(&input.email)
    .bind(&input.name)
    .bind(&input.role)
    .bind(input.employee_id)
    .bind(hash)
    .execute(&state.db)
    .await?;
    state
        .audit(Some(&user.session), "create", "user", None, serde_json::json!({"email": input.email, "role": input.role}))
        .await;
    let row = sqlx::query_as::<_, UserAccount>(
        r#"select u.id, u.email, u.name, u.role, u.active, u.employee_id,
                  e.emp_no as employee_no, u.last_login_at, u.created_at
           from users u left join employees e on e.id = u.employee_id
           where u.email = $1"#,
    )
    .bind(&input.email)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(row))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UserUpdate {
    pub role: Option<String>,
    pub active: Option<bool>,
    pub employee_id: Option<Uuid>,
    pub name: Option<String>,
}

#[utoipa::path(patch, path = "/api/admin/users/{id}", tag = "admin",
    params(("id" = Uuid, Path)), request_body = UserUpdate,
    responses((status = 200, body = UserAccount)))]
pub async fn users_update(
    user: AuthUser<AdminOnly>,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(input): Json<UserUpdate>,
) -> ApiResult<Json<UserAccount>> {
    if let Some(role) = &input.role {
        if hrms_core::Role::from_str_opt(role).is_none() {
            return Err(ApiError::bad("unknown role"));
        }
    }
    if id == user.session.user_id && input.active == Some(false) {
        return Err(ApiError::bad("you cannot deactivate your own account"));
    }
    let affected = sqlx::query(
        "update users set role = coalesce($2, role), active = coalesce($3, active),
                          employee_id = coalesce($4, employee_id), name = coalesce($5, name)
         where id = $1",
    )
    .bind(id)
    .bind(&input.role)
    .bind(input.active)
    .bind(input.employee_id)
    .bind(&input.name)
    .execute(&state.db)
    .await?
    .rows_affected();
    if affected == 0 {
        return Err(ApiError::not_found("user"));
    }
    state
        .audit(Some(&user.session), "update", "user", Some(id.to_string()), serde_json::json!({"role": input.role, "active": input.active}))
        .await;
    let row = sqlx::query_as::<_, UserAccount>(
        r#"select u.id, u.email, u.name, u.role, u.active, u.employee_id,
                  e.emp_no as employee_no, u.last_login_at, u.created_at
           from users u left join employees e on e.id = u.employee_id where u.id = $1"#,
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(row))
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct PasswordReset {
    #[validate(length(min = 10))]
    pub password: String,
}

#[utoipa::path(post, path = "/api/admin/users/{id}/reset-password", tag = "admin",
    params(("id" = Uuid, Path)), request_body = PasswordReset,
    responses((status = 200, description = "password reset")))]
pub async fn users_reset_password(
    user: AuthUser<AdminOnly>,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(input): Json<PasswordReset>,
) -> ApiResult<Json<serde_json::Value>> {
    input
        .validate()
        .map_err(|e| ApiError::bad(format!("{e}")))?;
    let hash = hash_password(&input.password)?;
    let affected = sqlx::query("update users set password_hash = $2 where id = $1")
        .bind(id)
        .bind(hash)
        .execute(&state.db)
        .await?
        .rows_affected();
    if affected == 0 {
        return Err(ApiError::not_found("user"));
    }
    state
        .audit(Some(&user.session), "reset_password", "user", Some(id.to_string()), serde_json::json!({}))
        .await;
    Ok(Json(serde_json::json!({ "status": "ok" })))
}

#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct AuditEntry {
    pub id: Uuid,
    pub actor_email: Option<String>,
    pub action: String,
    pub entity: String,
    pub entity_id: Option<String>,
    pub payload: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AuditQuery {
    pub limit: Option<i64>,
    pub entity: Option<String>,
}

#[utoipa::path(get, path = "/api/admin/audit", tag = "admin",
    params(("limit" = Option<i64>, Query), ("entity" = Option<String>, Query)),
    responses((status = 200, body = Vec<AuditEntry>)))]
pub async fn audit_list(
    _user: AuthUser<AdminOnly>,
    State(state): State<AppState>,
    Query(query): Query<AuditQuery>,
) -> ApiResult<Json<Vec<AuditEntry>>> {
    let rows = sqlx::query_as::<_, AuditEntry>(
        "select id, actor_email, action, entity, entity_id, payload, created_at
         from audit_log
         where ($1::text is null or entity = $1)
         order by created_at desc limit $2",
    )
    .bind(query.entity)
    .bind(query.limit.unwrap_or(100).clamp(1, 500))
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SystemStatus {
    pub version: String,
    pub org_name: String,
    pub payroll_fy: String,
    pub epf_ceiling: f64,
    pub esi_ceiling: f64,
    pub epf_ceiling_hike_enabled: bool,
    pub object_storage_enabled: bool,
    pub object_bucket: String,
    pub employees: i64,
    pub users: i64,
    pub database_ok: bool,
}

/// Runtime diagnostics for on-prem operators.
#[utoipa::path(get, path = "/api/admin/system", tag = "admin", responses((status = 200, body = SystemStatus)))]
pub async fn system_status(
    _user: AuthUser<AdminOnly>,
    State(state): State<AppState>,
) -> ApiResult<Json<SystemStatus>> {
    let employees: i64 = sqlx::query_scalar("select count(*) from employees")
        .fetch_one(&state.db)
        .await?;
    let users: i64 = sqlx::query_scalar("select count(*) from users")
        .fetch_one(&state.db)
        .await?;
    Ok(Json(SystemStatus {
        version: env!("CARGO_PKG_VERSION").to_string(),
        org_name: state.settings.org_name.clone(),
        payroll_fy: state.config.meta.financial_year.clone(),
        epf_ceiling: state.config.epf.wage_ceiling,
        esi_ceiling: state.config.esi.wage_ceiling,
        epf_ceiling_hike_enabled: state.config.epf.ceiling_hike.enabled,
        object_storage_enabled: state.storage.enabled(),
        object_bucket: state.storage.bucket().to_string(),
        employees,
        users,
        database_ok: true,
    }))
}
