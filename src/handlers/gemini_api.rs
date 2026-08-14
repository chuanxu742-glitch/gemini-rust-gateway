use std::{collections::HashMap, convert::Infallible, time::Duration};

use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
    response::{
        IntoResponse, Response, Sse,
        sse::{Event, KeepAlive},
    },
};
use futures_util::StreamExt;
use serde_json::{Value, json};

use crate::{
    compat::{
        append_tool_instructions, extract_tool_calls, gemini_contents_to_prompt,
        gemini_tool_required, gemini_tools,
    },
    error::{GatewayError, Result},
    middleware::auth::{authorize_gemini, requested_account},
};

use super::AppState;

pub async fn list_models(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<Value>> {
    authorize_gemini(
        &state.config,
        &headers,
        query.get("key").map(String::as_str),
    )?;
    let mut names = vec![
        state.config.default_model.clone(),
        state.config.advanced_model.clone(),
    ];
    names.sort();
    names.dedup();
    Ok(Json(json!({"models": names.into_iter().map(|name| json!({
        "name":format!("models/{name}"),"version":"web","displayName":name,
        "description":"Gemini Web model exposed by gemini-rust-gateway",
        "inputTokenLimit":state.config.max_prompt_chars,"outputTokenLimit":65536,
        "supportedGenerationMethods":["generateContent","streamGenerateContent","countTokens"]
    })).collect::<Vec<_>>() })))
}

pub async fn model_action(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(action): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    Json(body): Json<Value>,
) -> Result<Response> {
    authorize_gemini(
        &state.config,
        &headers,
        query.get("key").map(String::as_str),
    )?;
    let account = requested_account(&headers)?;
    let (model, operation) = action.rsplit_once(':').ok_or_else(|| {
        GatewayError::InvalidRequest(
            "expected models/{model}:generateContent or :streamGenerateContent".into(),
        )
    })?;
    if model.is_empty() {
        return Err(GatewayError::InvalidRequest("model cannot be empty".into()));
    }
    let mut prompt = gemini_contents_to_prompt(&body)?;
    if prompt.chars().count() > state.config.max_prompt_chars {
        return Err(GatewayError::InvalidRequest(format!(
            "contents exceed {} characters",
            state.config.max_prompt_chars
        )));
    }
    let tools = gemini_tools(&body);
    let gemini_choice = if gemini_tool_required(&body) {
        Some(json!("required"))
    } else {
        None
    };
    append_tool_instructions(&mut prompt, &tools, gemini_choice.as_ref());

    match operation {
        "countTokens" => {
            let estimated = prompt.chars().count().div_ceil(4).max(1);
            Ok(Json(json!({"totalTokens":estimated,"promptTokensDetails":[]})).into_response())
        }
        "generateContent" => {
            let mut output = state
                .gemini
                .generate_for(account.as_deref(), &prompt, model, 0)
                .await?;
            let mut calls = extract_tool_calls(&output.text);
            if calls.is_empty() && gemini_tool_required(&body) {
                let correction = format!(
                    "{prompt}\n\nYour previous response was invalid. Return only the required tool_calls JSON object now."
                );
                output = state
                    .gemini
                    .generate_for(account.as_deref(), &correction, model, 0)
                    .await?;
                calls = extract_tool_calls(&output.text);
            }
            if calls.is_empty() && gemini_tool_required(&body) {
                return Err(GatewayError::Provider(
                    "model did not produce the required function call".into(),
                ));
            }
            Ok(Json(gemini_response(&output.text, &calls, true)).into_response())
        }
        "streamGenerateContent" => {
            if !tools.is_empty() {
                let mut output = state
                    .gemini
                    .generate_for(account.as_deref(), &prompt, model, 0)
                    .await?;
                let mut calls = extract_tool_calls(&output.text);
                if calls.is_empty() && gemini_tool_required(&body) {
                    let correction = format!(
                        "{prompt}\n\nYour previous response was invalid. Return only the required tool_calls JSON object now."
                    );
                    output = state
                        .gemini
                        .generate_for(account.as_deref(), &correction, model, 0)
                        .await?;
                    calls = extract_tool_calls(&output.text);
                }
                if calls.is_empty() && gemini_tool_required(&body) {
                    return Err(GatewayError::Provider(
                        "model did not produce the required function call".into(),
                    ));
                }
                let response = gemini_response(&output.text, &calls, true);
                let stream = futures_util::stream::iter(vec![Ok::<Event, Infallible>(
                    Event::default().data(response.to_string()),
                )]);
                return Ok(Sse::new(stream).into_response());
            }
            let mut provider = state
                .gemini
                .generate_stream_for(account.as_deref(), &prompt, model, 0)
                .await?;
            let stream = async_stream::stream! {
                while let Some(update) = provider.next().await {
                    match update {
                        Ok(update) if !update.text_delta.is_empty() => yield Ok::<Event, Infallible>(Event::default().data(gemini_response(&update.text_delta, &[], false).to_string())),
                        Ok(_) => {},
                        Err(error) => {
                            yield Ok(Event::default().data(json!({"error":{"code":502,"message":error.to_string(),"status":"UNAVAILABLE"}}).to_string()));
                            return;
                        }
                    }
                }
                yield Ok(Event::default().data(gemini_response("", &[], true).to_string()));
            };
            Ok(Sse::new(stream)
                .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
                .into_response())
        }
        _ => Err(GatewayError::InvalidRequest(format!(
            "unsupported model operation: {operation}"
        ))),
    }
}

fn gemini_response(text: &str, calls: &[crate::compat::ToolCall], finished: bool) -> Value {
    let parts = if calls.is_empty() {
        if text.is_empty() {
            Vec::new()
        } else {
            vec![json!({"text":text})]
        }
    } else {
        calls
            .iter()
            .map(|call| {
                let args = serde_json::from_str::<Value>(&call.arguments)
                    .unwrap_or_else(|_| json!({"value":call.arguments}));
                json!({"functionCall":{"name":call.name,"args":args}})
            })
            .collect()
    };
    json!({
        "candidates":[{"content":{"parts":parts,"role":"model"},"finishReason":if finished {"STOP"} else {"FINISH_REASON_UNSPECIFIED"},"index":0}],
        "usageMetadata":{"promptTokenCount":0,"candidatesTokenCount":0,"totalTokenCount":0},
        "modelVersion":"gemini-web"
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, routing::post};

    #[test]
    fn emits_native_function_call_part() {
        let call = crate::compat::ToolCall {
            id: "call_1".into(),
            name: "weather".into(),
            arguments: r#"{"city":"Paris"}"#.into(),
        };
        let value = gemini_response("", &[call], true);
        assert_eq!(
            value["candidates"][0]["content"]["parts"][0]["functionCall"]["name"],
            "weather"
        );
    }

    #[test]
    fn axum_accepts_gemini_action_wildcard() {
        let _: Router = Router::new().route("/v1beta/models/{*action}", post(|| async { "ok" }));
    }
}
