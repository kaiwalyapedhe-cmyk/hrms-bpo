//! JWT authentication + compile-time role gates used by every handler.
//!
//! Handlers declare the permission they need in their signature, e.g.
//! `AuthUser<PayrollOffice>`, and axum rejects the request inside the extractor
//! before the handler body runs.

use std::marker::PhantomData;

use argon2::password_hash::{rand_core::OsRng, SaltString};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use chrono::{Duration, Utc};
use hrms_core::{DashboardScope, Role};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Debug, Clone)]
pub struct AuthConfig {
    secret: String,
    pub ttl_minutes: i64,
    pub issuer: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub email: String,
    pub name: String,
    pub role: String,
    pub emp: Option<String>,
    pub iss: String,
    pub iat: i64,
    pub exp: i64,
}

#[derive(Debug, Clone)]
pub struct Session {
    pub user_id: Uuid,
    pub employee_id: Option<Uuid>,
    pub email: String,
    pub name: String,
    pub role: Role,
}

impl Session {
    pub fn scope(&self) -> DashboardScope {
        self.role.dashboard_scope()
    }

    pub fn is(&self, role: Role) -> bool {
        self.role == role
    }
}

impl AuthConfig {
    pub fn from_env() -> Self {
        let secret = std::env::var("JWT_SECRET").unwrap_or_else(|_| {
            tracing::warn!(
                "JWT_SECRET is not set — using the development secret. Set it in .env for any real deployment."
            );
            "dev-only-secret-change-me-in-production".to_string()
        });
        let ttl_minutes = std::env::var("JWT_TTL_MINUTES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(480);
        AuthConfig {
            secret,
            ttl_minutes,
            issuer: "hrms-onprem".to_string(),
        }
    }

    pub fn issue(&self, session: &Session) -> Result<(String, i64), ApiError> {
        let now = Utc::now();
        let exp = now + Duration::minutes(self.ttl_minutes);
        let claims = Claims {
            sub: session.user_id.to_string(),
            email: session.email.clone(),
            name: session.name.clone(),
            role: session.role.as_str().to_string(),
            emp: session.employee_id.map(|id| id.to_string()),
            iss: self.issuer.clone(),
            iat: now.timestamp(),
            exp: exp.timestamp(),
        };
        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(self.secret.as_bytes()),
        )
        .map_err(|e| ApiError::internal(format!("jwt encode: {e}")))?;
        Ok((token, exp.timestamp()))
    }

    pub fn session_from_token(&self, token: &str) -> Result<Session, ApiError> {
        let mut validation = Validation::default();
        validation.set_issuer(&[self.issuer.as_str()]);
        let data = decode::<Claims>(
            token,
            &DecodingKey::from_secret(self.secret.as_bytes()),
            &validation,
        )
        .map_err(|e| match e.kind() {
            jsonwebtoken::errors::ErrorKind::ExpiredSignature => {
                ApiError::unauthorized("token expired")
            }
            _ => ApiError::unauthorized("invalid token"),
        })?;

        let claims = data.claims;
        let role = Role::from_str_opt(&claims.role)
            .ok_or_else(|| ApiError::unauthorized("token carries an unknown role"))?;
        let user_id = Uuid::parse_str(&claims.sub)
            .map_err(|_| ApiError::unauthorized("malformed token subject"))?;
        let employee_id = claims
            .emp
            .as_deref()
            .and_then(|v| Uuid::parse_str(v).ok());

        Ok(Session {
            user_id,
            employee_id,
            email: claims.email,
            name: claims.name,
            role,
        })
    }
}

pub fn hash_password(password: &str) -> Result<String, ApiError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| ApiError::internal(format!("argon2: {e}")))
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    match PasswordHash::new(hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

fn bearer_token(parts: &Parts) -> Result<String, ApiError> {
    let header = parts
        .headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| ApiError::unauthorized("missing Authorization header"))?;
    let token = header
        .strip_prefix("Bearer ")
        .ok_or_else(|| ApiError::unauthorized("Authorization must use the Bearer scheme"))?;
    if token.trim().is_empty() {
        return Err(ApiError::unauthorized("empty bearer token"));
    }
    Ok(token.trim().to_string())
}

/// Permission marker implemented by the gates below.
pub trait RoleGate: Send + Sync + 'static {
    fn allows(role: Role) -> bool;
    fn label() -> &'static str;
}

/// Extractor carrying the authenticated session, gated by `R`.
pub struct AuthUser<R: RoleGate = AnyRole> {
    pub session: Session,
    _gate: PhantomData<R>,
}

impl<R: RoleGate> AuthUser<R> {
    pub fn employee_id(&self) -> Result<Uuid, ApiError> {
        self.session
            .employee_id
            .ok_or_else(|| ApiError::bad("this account is not linked to an employee record"))
    }

    pub fn require(&self, allowed: bool, what: &str) -> Result<(), ApiError> {
        if allowed {
            Ok(())
        } else {
            Err(ApiError::forbidden(format!("{} cannot {what}", self.session.role.as_str())))
        }
    }
}

impl std::ops::Deref for AuthUser<AnyRole> {
    type Target = Session;
    fn deref(&self) -> &Session {
        &self.session
    }
}

impl<R: RoleGate> FromRequestParts<AppState> for AuthUser<R> {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let token = bearer_token(parts)?;
        let session = state.auth.session_from_token(&token)?;
        if !R::allows(session.role) {
            return Err(ApiError::forbidden(format!(
                "{} may not access this resource (requires {})",
                session.role.as_str(),
                R::label()
            )));
        }
        Ok(AuthUser {
            session,
            _gate: PhantomData,
        })
    }
}

macro_rules! gates {
    ($($name:ident => $label:literal : $($role:ident)|+),* $(,)?) => {
        $(
            pub struct $name;
            impl RoleGate for $name {
                fn allows(role: Role) -> bool {
                    matches!(role, $(Role::$role)|+)
                }
                fn label() -> &'static str { $label }
            }
        )*
    };
}

gates! {
    AnyRole => "an authenticated user" : SuperAdmin | HrAdmin | PayrollAdmin | BenefitsAdmin | Recruiter | Manager | TeamLead | Agent | QaAnalyst | Trainer,
    PeopleAdmin => "HR administration" : SuperAdmin | HrAdmin,
    PayrollOffice => "payroll administration" : SuperAdmin | PayrollAdmin,
    BenefitsOffice => "benefits administration" : SuperAdmin | HrAdmin | BenefitsAdmin,
    RecruitingOffice => "recruiting" : SuperAdmin | HrAdmin | Recruiter,
    Supervisor => "manager or team lead" : SuperAdmin | Manager | TeamLead,
    QualityOffice => "quality assurance" : SuperAdmin | QaAnalyst | TeamLead,
    TrainingOffice => "training administration" : SuperAdmin | HrAdmin | Trainer,
    AdminOnly => "super administrator" : SuperAdmin,
}

/// Try to resolve a session from a bare token (used by the WebSocket handler,
/// where the browser cannot set an Authorization header).
pub fn session_from_query_token(state: &AppState, token: &str) -> Result<Session, ApiError> {
    state.auth.session_from_token(token)
}
