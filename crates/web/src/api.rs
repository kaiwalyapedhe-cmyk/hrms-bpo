//! Thin HTTP client for the HRMS API. The SPA is served by nginx, which
//! proxies `/api` and `/ws` to the axum service.

use gloo_net::http::Request;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const TOKEN_KEY: &str = "hrms.token";
pub const USER_KEY: &str = "hrms.user";
pub const API_BASE_KEY: &str = "hrms.api_base";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionUser {
    pub id: String,
    pub email: String,
    pub name: String,
    pub role: String,
    pub employee_id: Option<String>,
    pub employee_no: Option<String>,
}

impl SessionUser {
    pub fn is_super_admin(&self) -> bool {
        self.role == "super_admin"
    }
    pub fn is_supervisor(&self) -> bool {
        matches!(self.role.as_str(), "super_admin" | "manager" | "team_lead")
    }
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|w| w.local_storage().ok().flatten())
}

pub fn base() -> String {
    if let Some(s) = storage() {
        if let Ok(Some(value)) = s.get_item(API_BASE_KEY) {
            if !value.trim().is_empty() {
                return value.trim_end_matches('/').to_string();
            }
        }
    }
    "/api".to_string()
}

pub fn token() -> Option<String> {
    storage().and_then(|s| s.get_item(TOKEN_KEY).ok().flatten())
}

pub fn user() -> Option<SessionUser> {
    let raw = storage()?.get_item(USER_KEY).ok().flatten()?;
    serde_json::from_str(&raw).ok()
}

pub fn save_session(token: &str, user: &SessionUser) {
    if let Some(s) = storage() {
        let _ = s.set_item(TOKEN_KEY, token);
        let _ = s.set_item(USER_KEY, &serde_json::to_string(user).unwrap_or_default());
    }
}

pub fn clear_session() {
    if let Some(s) = storage() {
        let _ = s.remove_item(TOKEN_KEY);
        let _ = s.remove_item(USER_KEY);
    }
}

fn error_message(status: u16, body: &str) -> String {
    if let Ok(value) = serde_json::from_str::<Value>(body) {
        if let Some(message) = value
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
        {
            return message.to_string();
        }
    }
    format!("request failed ({status})")
}

pub async fn get_json(path: &str) -> Result<Value, String> {
    let token = token().ok_or_else(|| "not signed in".to_string())?;
    let response = Request::get(&format!("{}{}", base(), path))
        .header("Authorization", &format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let body = response.text().await.map_err(|e| e.to_string())?;
    if status >= 400 {
        return Err(error_message(status, &body));
    }
    serde_json::from_str(&body).map_err(|e| e.to_string())
}

pub async fn send_json(method: &str, path: &str, payload: &Value) -> Result<Value, String> {
    let token = token().ok_or_else(|| "not signed in".to_string())?;
    let url = format!("{}{}", base(), path);
    let builder = match method {
        "PUT" => Request::put(&url),
        "PATCH" => Request::patch(&url),
        _ => Request::post(&url),
    };
    let response = builder
        .header("Authorization", &format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .json(payload)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let body = response.text().await.map_err(|e| e.to_string())?;
    if status >= 400 {
        return Err(error_message(status, &body));
    }
    if body.trim().is_empty() {
        Ok(Value::Null)
    } else {
        serde_json::from_str(&body).map_err(|e| e.to_string())
    }
}

pub async fn post_json(path: &str, payload: &Value) -> Result<Value, String> {
    send_json("POST", path, payload).await
}

pub async fn login(email: &str, password: &str) -> Result<SessionUser, String> {
    let response = Request::post(&format!("{}/auth/login", base()))
        .header("Content-Type", "application/json")
        .json(&json!({ "email": email, "password": password }))
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let body = response.text().await.map_err(|e| e.to_string())?;
    if status >= 400 {
        return Err(error_message(status, &body));
    }
    let value: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    let token = value["token"].as_str().unwrap_or_default().to_string();
    let user: SessionUser =
        serde_json::from_value(value["user"].clone()).map_err(|e| e.to_string())?;
    save_session(&token, &user);
    Ok(user)
}

/// Persist the API base override from a local file or query string
/// (`?api=http://localhost:8080/api`) — handy with `trunk serve`.
pub fn capture_api_override() {
    let window = web_sys::window().expect("window");
    if let Ok(search) = window.location().search() {
        if let Ok(params) = web_sys::UrlSearchParams::new_with_str(&search) {
            if let Some(value) = params.get("api") {
                if let Some(s) = storage() {
                    let _ = s.set_item(API_BASE_KEY, &value);
                }
            }
        }
    }
    let _ = window.document().expect("document").set_title("Sampark HRMS");
}
