use std::{convert::Infallible, time::Duration};

use axum::{
    Json,
    extract::State,
    http::HeaderMap,
    response::{
        IntoResponse, Response, Sse,
        sse::{Event, KeepAlive},
    },
};
use futures_util::StreamExt;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    compat::{
        ToolCall, append_tool_instructions, extract_tool_calls, responses_input_to_prompt,
        tool_choice_required,
    },
    error::{GatewayError, Result},
    middleware::auth::{authorize, requested_account},
    util::time::epoch_seconds,
};

use super::AppState;

pub async fn create_response(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> Result<Response> {
    authorize(&state.config, &headers)?;
    let account = requested_account(&headers)?;
    let input = request.get("input").ok_or_else(|| {
        GatewayError::InvalidRequest(
            "input is required; previous_response_id storage is not supported".into(),
        )
    })?;
    let instructions = request.get("instructions").and_then(Value::as_str);
    let mut prompt = responses_input_to_prompt(input, instructions)?;
    if prompt.chars().count() > state.config.max_prompt_chars {
        return Err(GatewayError::InvalidRequest(format!(
            "input exceeds {} characters",
            state.config.max_prompt_chars
        )));
    }
    let tools = request
        .get("tools")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    append_tool_instructions(&mut prompt, &tools, request.get("tool_choice"));
    let model = request
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or(&state.config.default_model)
        .to_owned();
    let stream = request
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let response_id = format!("resp_{}", Uuid::new_v4().simple());
    let created_at = epoch_seconds();

    if !tools.is_empty() {
        let mut output = state
            .gemini
            .generate_for(account.as_deref(), &prompt, &model, 0)
            .await?;
        let mut calls = extract_tool_calls(&output.text);
        if calls.is_empty() && tool_choice_required(request.get("tool_choice")) {
            let correction = format!(
                "{prompt}\n\nYour previous response was invalid. Return only the required tool_calls JSON object now."
            );
            output = state
                .gemini
                .generate_for(account.as_deref(), &correction, &model, 0)
                .await?;
            calls = extract_tool_calls(&output.text);
        }
        if calls.is_empty() && tool_choice_required(request.get("tool_choice")) {
            return Err(GatewayError::Provider(
                "model did not produce the required function call".into(),
            ));
        }
        let response = response_object(&response_id, created_at, &model, &output.text, &calls);
        return if stream {
            Ok(stream_completed_response(response).into_response())
        } else {
            Ok(Json(response).into_response())
        };
    }

    if stream {
        let mut provider = state
            .gemini
            .generate_stream_for(account.as_deref(), &prompt, &model, 0)
            .await?;
        let event_stream = async_stream::stream! {
            let message_id = format!("msg_{}", Uuid::new_v4().simple());
            let initial = response_shell(&response_id, created_at, &model, "in_progress", vec![]);
            yield event("response.created", json!({"type":"response.created","sequence_number":0,"response":initial}));
            let item = json!({"id":message_id,"type":"message","status":"in_progress","role":"assistant","content":[]});
            yield event("response.output_item.added", json!({"type":"response.output_item.added","sequence_number":1,"output_index":0,"item":item}));
            let part = json!({"type":"output_text","text":"","annotations":[],"logprobs":[]});
            yield event("response.content_part.added", json!({"type":"response.content_part.added","sequence_number":2,"item_id":message_id,"output_index":0,"content_index":0,"part":part}));
            let mut text = String::new();
            let mut sequence = 3_u64;
            while let Some(update) = provider.next().await {
                match update {
                    Ok(update) if !update.text_delta.is_empty() => {
                        text.push_str(&update.text_delta);
                        yield event("response.output_text.delta", json!({
                            "type":"response.output_text.delta","sequence_number":sequence,"item_id":message_id,
                            "output_index":0,"content_index":0,"delta":update.text_delta,"logprobs":[]
                        }));
                        sequence += 1;
                    }
                    Ok(_) => {}
                    Err(error) => {
                        yield event("error", json!({"type":"error","sequence_number":sequence,"error":{"type":"provider_error","message":error.to_string()}}));
                        return;
                    }
                }
            }
            yield event("response.output_text.done", json!({"type":"response.output_text.done","sequence_number":sequence,"item_id":message_id,"output_index":0,"content_index":0,"text":text,"logprobs":[]})); sequence += 1;
            let done_part = json!({"type":"output_text","text":text,"annotations":[],"logprobs":[]});
            yield event("response.content_part.done", json!({"type":"response.content_part.done","sequence_number":sequence,"item_id":message_id,"output_index":0,"content_index":0,"part":done_part})); sequence += 1;
            let done_item = json!({"id":message_id,"type":"message","status":"completed","role":"assistant","content":[done_part]});
            yield event("response.output_item.done", json!({"type":"response.output_item.done","sequence_number":sequence,"output_index":0,"item":done_item})); sequence += 1;
            let completed = response_shell(&response_id, created_at, &model, "completed", vec![done_item]);
            yield event("response.completed", json!({"type":"response.completed","sequence_number":sequence,"response":completed}));
        };
        return Ok(Sse::new(event_stream)
            .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
            .into_response());
    }

    let output = state
        .gemini
        .generate_for(account.as_deref(), &prompt, &model, 0)
        .await?;
    Ok(Json(response_object(
        &response_id,
        created_at,
        &model,
        &output.text,
        &[],
    ))
    .into_response())
}

fn response_object(
    id: &str,
    created_at: u64,
    model: &str,
    text: &str,
    calls: &[ToolCall],
) -> Value {
    let output = if calls.is_empty() {
        vec![json!({
            "id":format!("msg_{}", Uuid::new_v4().simple()),"type":"message","status":"completed","role":"assistant",
            "content":[{"type":"output_text","text":text,"annotations":[],"logprobs":[]}]
        })]
    } else {
        calls.iter().map(|call| json!({
            "id":format!("fc_{}", Uuid::new_v4().simple()),"type":"function_call","status":"completed",
            "call_id":call.id,"name":call.name,"arguments":call.arguments
        })).collect()
    };
    response_shell(id, created_at, model, "completed", output)
}

fn response_shell(
    id: &str,
    created_at: u64,
    model: &str,
    status: &str,
    output: Vec<Value>,
) -> Value {
    json!({
        "id":id,"object":"response","created_at":created_at,"status":status,"background":false,
        "error":null,"incomplete_details":null,"instructions":null,"max_output_tokens":null,"model":model,
        "output":output,"parallel_tool_calls":true,"previous_response_id":null,"reasoning":{"effort":null,"summary":null},
        "store":false,"temperature":null,"text":{"format":{"type":"text"}},"tool_choice":"auto","tools":[],
        "top_p":null,"truncation":"disabled","usage":{"input_tokens":0,"input_tokens_details":{"cached_tokens":0},"output_tokens":0,"output_tokens_details":{"reasoning_tokens":0},"total_tokens":0},
        "user":null,"metadata":{}
    })
}

fn stream_completed_response(
    response: Value,
) -> Sse<impl futures_util::Stream<Item = std::result::Result<Event, Infallible>>> {
    let output = response
        .get("output")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut events = vec![event(
        "response.created",
        json!({"type":"response.created","sequence_number":0,"response":response_shell(
            response.get("id").and_then(Value::as_str).unwrap_or("resp_unknown"),
            response.get("created_at").and_then(Value::as_u64).unwrap_or(0),
            response.get("model").and_then(Value::as_str).unwrap_or("gemini"), "in_progress", vec![]
        )}),
    )];
    let mut sequence = 1_u64;
    for (index, item) in output.iter().enumerate() {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
        let item_id = item
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("item_unknown");
        let mut initial_item = item.clone();
        if item_type == "function_call" {
            initial_item["arguments"] = json!("");
        } else if item_type == "message" {
            initial_item["content"] = json!([]);
            initial_item["status"] = json!("in_progress");
        }
        events.push(event("response.output_item.added", json!({"type":"response.output_item.added","sequence_number":sequence,"output_index":index,"item":initial_item})));
        sequence += 1;
        if item_type == "function_call" {
            let arguments = item.get("arguments").and_then(Value::as_str).unwrap_or("");
            events.push(event("response.function_call_arguments.delta", json!({"type":"response.function_call_arguments.delta","sequence_number":sequence,"item_id":item_id,"output_index":index,"delta":arguments})));
            sequence += 1;
            events.push(event("response.function_call_arguments.done", json!({"type":"response.function_call_arguments.done","sequence_number":sequence,"item_id":item_id,"output_index":index,"arguments":arguments})));
            sequence += 1;
        } else if item_type == "message"
            && let Some(part) = item
                .get("content")
                .and_then(Value::as_array)
                .and_then(|parts| parts.first())
        {
            let text = part.get("text").and_then(Value::as_str).unwrap_or("");
            events.push(event("response.content_part.added", json!({"type":"response.content_part.added","sequence_number":sequence,"item_id":item_id,"output_index":index,"content_index":0,"part":{"type":"output_text","text":"","annotations":[],"logprobs":[]}})));
            sequence += 1;
            events.push(event("response.output_text.delta", json!({"type":"response.output_text.delta","sequence_number":sequence,"item_id":item_id,"output_index":index,"content_index":0,"delta":text,"logprobs":[]})));
            sequence += 1;
            events.push(event("response.output_text.done", json!({"type":"response.output_text.done","sequence_number":sequence,"item_id":item_id,"output_index":index,"content_index":0,"text":text,"logprobs":[]})));
            sequence += 1;
            events.push(event("response.content_part.done", json!({"type":"response.content_part.done","sequence_number":sequence,"item_id":item_id,"output_index":index,"content_index":0,"part":part})));
            sequence += 1;
        }
        events.push(event("response.output_item.done", json!({"type":"response.output_item.done","sequence_number":sequence,"output_index":index,"item":item})));
        sequence += 1;
    }
    events.push(event(
        "response.completed",
        json!({"type":"response.completed","sequence_number":sequence,"response":response}),
    ));
    Sse::new(futures_util::stream::iter(events))
}

fn event(name: &'static str, value: Value) -> std::result::Result<Event, Infallible> {
    Ok(Event::default().event(name).data(value.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_contains_function_call_contract() {
        let value = response_object(
            "resp_1",
            1,
            "gemini",
            "",
            &[ToolCall {
                id: "call_1".into(),
                name: "weather".into(),
                arguments: "{}".into(),
            }],
        );
        assert_eq!(value["output"][0]["type"], "function_call");
        assert_eq!(value["output"][0]["call_id"], "call_1");
    }
}
