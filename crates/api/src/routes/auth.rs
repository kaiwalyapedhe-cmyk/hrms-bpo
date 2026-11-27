use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use utoipa::ToSchema;
use uuid::Uuid;
use validator::Validate;

use crate::auth::{hash_password, verify_password, AnyRole, AuthUser};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/login", post(auth_login))
        .route("/auth/me", get(auth_me))
        .route("/auth/change-password", post(auth_change_password))
        .route("/auth/roles", get(auth_roles))
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct LoginRequest {
    #[validate(email)]
    pub email: String,
    #[validate(length(min = 6))]
    pub password: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UserProfile {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub role: String,
    pub employee_id: Option<Uuid>,
    pub employee_no: Option<String>,
    pub dashboard_scope: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LoginResponse {
    pub token: String,
    pub expires_at: i64,
    pub user: UserProfile,
}

#[derive(Debug, FromRow)]
struct UserRow {
    id: Uuid,
    employee_id: Option<Uuid>,
    email: String,
    name: String,
    password_hash: String,
    role: String,
    active: bool,
    employee_no: Option<String>,
}

/// Authenticate with email + password and receive a JWT.
#[utoipa::path(
    post, path = "/api/auth/login", tag = "auth",
    request_body = LoginRequest,
    responses((status = 200, body = LoginResponse), (status = 401, description = "invalid credentials"))
)]
pub async fn auth_login(
    State(state): State<AppState>,
    Json(payload): Json<LoginRequest>,
) -> ApiResult<Json<LoginResponse>> {
    payload
        .validate()
        .map_err(|e| ApiError::bad(format!("invalid login payload: {e}")))?;

    let row: Option<UserRow> = sqlx::query_as(
        "select u.id, u.employee_id, u.email, u.name, u.password_hash, u.role, u.active,
                e.emp_no as employee_no
         from users u left join employees e on e.id = u.employee_id
         where lower(u.email) = lower($1)",
    )
    .bind(&payload.email)
    .fetch_optional(&state.db)
    .await?;

    let row = row.ok_or_else(|| ApiError::unauthorized("invalid email or password"))?;
    if !row.active {
        return Err(ApiError::forbidden("this account is disabled"));
    }
    if !verify_password(&payload.password, &row.password_hash) {
        return Err(ApiError::unauthorized("invalid email or password"));
    }

    let role = hrms_core::Role::from_str_opt(&row.role)
        .ok_or_else(|| ApiError::internal("user has an invalid role"))?;
    let session = crate::auth::Session {
        user_id: row.id,
        employee_id: row.employee_id,
        email: row.email.clone(),
        name: row.name.clone(),
        role,
    };
    let (token, expires_at) = state.auth.issue(&session)?;

    sqlx::query("update users set last_login_at = now() where id = $1")
        .bind(row.id)
        .execute(&state.db)
        .await?;
    state
        .audit(Some(&session), "login", "user", Some(row.id.to_string()), serde_json::json!({}))
        .await;

    Ok(Json(LoginResponse {
        token,
        expires_at,
        user: UserProfile {
            id: row.id,
            email: row.email,
            name: row.name,
            role: row.role,
            employee_id: row.employee_id,
            employee_no: row.employee_no,
            dashboard_scope: format!("{:?}", role.dashboard_scope()).to_lowercase(),
        },
    }))
}

/// Current session profile.
#[utoipa::path(get, path = "/api/auth/me", tag = "auth", responses((status = 200, body = UserProfile)))]
pub async fn auth_me(
    user: AuthUser<AnyRole>,
    State(state): State<AppState>,
) -> ApiResult<Json<UserProfile>> {
    let employee_no: Option<String> = match user.session.employee_id {
        Some(id) => sqlx::query_scalar("select emp_no from employees where id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await?,
        None => None,
    };
    Ok(Json(UserProfile {
        id: user.session.user_id,
        email: user.session.email.clone(),
        name: user.session.name.clone(),
        role: user.session.role.as_str().to_string(),
        employee_id: user.session.employee_id,
        employee_no,
        dashboard_scope: format!("{:?}", user.session.role.dashboard_scope()).to_lowercase(),
    }))
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    #[validate(length(min = 10))]
    pub new_password: String,
}

/// Rotate the password of the signed-in user.
#[utoipa::path(post, path = "/api/auth/change-password", tag = "auth", request_body = ChangePasswordRequest,
    responses((status = 200, description = "password changed")))]
pub async fn auth_change_password(
    user: AuthUser<AnyRole>,
    State(state): State<AppState>,
    Json(payload): Json<ChangePasswordRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    payload
        .validate()
        .map_err(|e| ApiError::bad(format!("{e}")))?;
    let hash: String = sqlx::query_scalar("select password_hash from users where id = $1")
        .bind(user.session.user_id)
        .fetch_one(&state.db)
        .await?;
    if !verify_password(&payload.current_password, &hash) {
        return Err(ApiError::unauthorized("current password is incorrect"));
    }
    let new_hash = hash_password(&payload.new_password)?;
    sqlx::query("update users set password_hash = $1 where id = $2")
        .bind(new_hash)
        .bind(user.session.user_id)
        .execute(&state.db)
        .await?;
    state
        .audit(
            Some(&user.session),
            "change_password",
            "user",
            Some(user.session.user_id.to_string()),
            serde_json::json!({}),
        )
        .await;
    Ok(Json(serde_json::json!({ "status": "ok" })))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RoleInfo {
    pub role: String,
    pub dashboard_scope: String,
}

/// Roles understood by the RBAC layer.
#[utoipa::path(get, path = "/api/auth/roles", tag = "auth", responses((status = 200, body = Vec<RoleInfo>)))]
pub async fn auth_roles() -> Json<Vec<RoleInfo>> {
    Json(
        hrms_core::Role::ALL
            .iter()
            .map(|role| RoleInfo {
                role: role.as_str().to_string(),
                dashboard_scope: format!("{:?}", role.dashboard_scope()).to_lowercase(),
            })
            .collect(),
    )
}
