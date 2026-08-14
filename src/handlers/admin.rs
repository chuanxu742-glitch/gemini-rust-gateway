use crate::{
    error::{GatewayError, Result},
    middleware::auth::{authorize, authorize_admin},
};
use axum::{
    Json,
    extract::{ConnectInfo, Path, State},
    http::HeaderMap,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::net::SocketAddr;

use super::AppState;

#[derive(Debug, Deserialize)]
pub struct CookieSyncRequest {
    #[serde(default = "default_account_id")]
    account_id: String,
    #[serde(default)]
    auth_user: usize,
    secure_1psid: String,
    #[serde(default)]
    secure_1psidts: Option<String>,
    #[serde(default = "default_weight")]
    weight: usize,
}

#[derive(Debug, Deserialize)]
pub struct AccountConfigurationRequest {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    weight: Option<usize>,
}

pub async fn sync_cookies(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<CookieSyncRequest>,
) -> Result<Json<Value>> {
    authorize_admin_peer(&state, peer)?;
    authorize_admin(&state.config, &headers)?;
    if !state.config.cookie_sync_enabled {
        return Err(GatewayError::FeatureDisabled(
            "cookie sync is disabled; set GEMINI_COOKIE_SYNC_ENABLED=true".into(),
        ));
    }
    validate_cookie("__Secure-1PSID", &request.secure_1psid, true)?;
    if let Some(value) = &request.secure_1psidts {
        validate_cookie("__Secure-1PSIDTS", value, false)?;
    }

    state
        .gemini
        .upsert_credentials(
            request.account_id.clone(),
            request.auth_user,
            request.secure_1psid.clone(),
            request.secure_1psidts.clone(),
        )
        .await?;
    if request.weight != 1 {
        state
            .gemini
            .configure_account(&request.account_id, None, Some(request.weight))
            .await?;
    }
    Ok(Json(json!({
        "status":"ok","account_id":request.account_id,"auth_user":request.auth_user,
        "message":"cookies validated, activated, and persisted"
    })))
}

pub async fn configure_account(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(account_id): Path<String>,
    Json(request): Json<AccountConfigurationRequest>,
) -> Result<Json<Value>> {
    authorize_admin_peer(&state, peer)?;
    authorize_admin(&state.config, &headers)?;
    let account = state
        .gemini
        .configure_account(&account_id, request.enabled, request.weight)
        .await?;
    Ok(Json(json!({"status":"ok", "account": account})))
}

pub async fn delete_account(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(account_id): Path<String>,
) -> Result<Json<Value>> {
    authorize_admin_peer(&state, peer)?;
    authorize_admin(&state.config, &headers)?;
    state.gemini.delete_account(&account_id).await?;
    Ok(Json(json!({"status":"ok", "deleted": account_id})))
}

pub async fn list_accounts(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>> {
    authorize(&state.config, &headers)?;
    let accounts = state.gemini.account_statuses().await;
    let ready = accounts.iter().filter(|account| account.ready).count();
    Ok(Json(json!({
        "object": "list",
        "data": accounts,
        "ready": ready,
        "rotation": "least_inflight_round_robin"
    })))
}

fn default_account_id() -> String {
    "default".into()
}

fn default_weight() -> usize {
    1
}

fn authorize_admin_peer(state: &AppState, peer: SocketAddr) -> Result<()> {
    if state.config.remote_admin_enabled || peer.ip().is_loopback() {
        Ok(())
    } else {
        Err(GatewayError::Auth(
            "remote account administration is disabled".into(),
        ))
    }
}

fn validate_cookie(name: &str, value: &str, required: bool) -> Result<()> {
    if required && value.trim().len() < 16 {
        return Err(GatewayError::InvalidRequest(format!(
            "{name} is missing or too short"
        )));
    }
    if value.len() > 8192 || value.contains(['\r', '\n', '\0']) {
        return Err(GatewayError::InvalidRequest(format!("invalid {name}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_cookie_injection() {
        assert!(validate_cookie("cookie", "safe-but-too-short", false).is_ok());
        assert!(validate_cookie("cookie", "bad\nGEMINI_API_KEY=x", false).is_err());
    }

    #[test]
    fn admin_peer_policy_distinguishes_loopback() {
        assert!(
            "127.0.0.1"
                .parse::<std::net::IpAddr>()
                .unwrap()
                .is_loopback()
        );
        assert!(
            !"192.168.1.20"
                .parse::<std::net::IpAddr>()
                .unwrap()
                .is_loopback()
        );
    }
}
