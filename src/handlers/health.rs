use axum::{
    Json,
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};

use crate::observability::metrics::Metrics;

use super::AppState;

pub async fn health() -> Json<Value> {
    Json(json!({"status": "ok", "service": "gemini-rust-gateway"}))
}

pub async fn healthz() -> Response {
    (StatusCode::OK, "ok").into_response()
}

pub async fn readyz(State(state): State<AppState>) -> Response {
    if state.gemini.is_ready() {
        (StatusCode::OK, "ready").into_response()
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"status": "not_ready", "reason": "gemini session not initialized"})),
        )
            .into_response()
    }
}

pub async fn metrics(State(state): State<AppState>) -> Response {
    let mut body = state.metrics.render_prometheus();
    body.push_str("# HELP gateway_account_ready Whether a Gemini account is ready.\n");
    body.push_str("# TYPE gateway_account_ready gauge\n");
    body.push_str("# HELP gateway_account_inflight In-flight requests for a Gemini account.\n");
    body.push_str("# TYPE gateway_account_inflight gauge\n");
    body.push_str("# HELP gateway_account_cooldown_seconds Remaining account cooldown.\n");
    body.push_str("# TYPE gateway_account_cooldown_seconds gauge\n");
    for account in state.gemini.account_statuses().await {
        let id = account.id.replace('\\', "\\\\").replace('"', "\\\"");
        body.push_str(&format!(
            "gateway_account_ready{{account=\"{id}\"}} {}\n",
            u8::from(account.ready && account.enabled)
        ));
        body.push_str(&format!(
            "gateway_account_inflight{{account=\"{id}\"}} {}\n",
            account.inflight
        ));
        body.push_str(&format!(
            "gateway_account_cooldown_seconds{{account=\"{id}\"}} {}\n",
            account.cooldown_seconds
        ));
    }
    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
        .into_response()
}

#[allow(dead_code)]
fn _metrics_marker(_: &Metrics) {}
