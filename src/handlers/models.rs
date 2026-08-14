use axum::{Json, extract::State, http::HeaderMap};
use serde_json::{Value, json};

use crate::{error::Result, middleware::auth::authorize};

use super::AppState;

pub async fn models(State(state): State<AppState>, headers: HeaderMap) -> Result<Json<Value>> {
    authorize(&state.config, &headers)?;
    let mut names = vec![
        state.config.default_model.clone(),
        state.config.advanced_model.clone(),
    ];
    names.sort();
    names.dedup();
    Ok(Json(json!({
        "object": "list",
        "data": names.into_iter().map(|name| json!({
            "id": name,
            "object": "model",
            "owned_by": "google-gemini-web-rust",
        })).collect::<Vec<_>>()
    })))
}

pub async fn capabilities(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>> {
    authorize(&state.config, &headers)?;
    let implemented = [
        "image",
        "video",
        "audio",
        "music",
        "deep_research",
        "canvas",
    ];
    let feature_map = implemented
        .iter()
        .map(|name| {
            (
                (*name).to_owned(),
                json!({
                    "implemented": true,
                    "enabled": state.config.enabled_generations.iter().any(|item| item == name),
                }),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    let accounts = state.gemini.account_statuses().await;
    let ready_accounts = accounts.iter().filter(|account| account.ready).count();
    Ok(Json(json!({
        "chat_completions": true,
        "responses_api": true,
        "gemini_v1beta": true,
        "function_calling": "prompt_translated",
        "streaming": true,
        "openai_images": true,
        "async_generations": state.config.enabled_generations,
        "generation_features": feature_map,
        "native_canvas_protocol": false,
        "canvas_output": "self-contained-html",
        "runtime": "rust",
        "multi_account": true,
        "account_count": accounts.len(),
        "ready_accounts": ready_accounts,
        "account_selection_header": "X-Gemini-Account",
        "rotation": "least_inflight_round_robin",
        "credential_protection": crate::gemini::accounts::protection_backend(),
        "cookie_sync": state.config.cookie_sync_enabled,
        "retry_attempts": state.config.retry_attempts,
    })))
}
