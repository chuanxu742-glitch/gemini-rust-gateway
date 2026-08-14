use std::{
    convert::Infallible,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

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
    compat::{append_tool_instructions, extract_tool_calls, tool_choice_required},
    domain::chat::{ChatCompletionRequest, Message, MessageContent},
    error::{GatewayError, Result},
    middleware::auth::{authorize, requested_account},
};

use super::AppState;

pub async fn chat_completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ChatCompletionRequest>,
) -> Result<Response> {
    authorize(&state.config, &headers)?;
    let account = requested_account(&headers)?;
    let mut prompt = messages_to_prompt(&state.config, &request.messages)?;
    append_tool_instructions(&mut prompt, &request.tools, request.tool_choice.as_ref());
    let model = request
        .model
        .clone()
        .unwrap_or_else(|| state.config.default_model.clone());
    let request_id = format!("chatcmpl-{}", Uuid::new_v4().simple());
    let created = epoch_seconds();
    let _ignored_openai_options = (request.temperature, request.max_tokens);

    if !request.tools.is_empty() {
        let mut output = state
            .gemini
            .generate_for(account.as_deref(), &prompt, &model, 0)
            .await?;
        let mut calls = extract_tool_calls(&output.text);
        if calls.is_empty() && tool_choice_required(request.tool_choice.as_ref()) {
            let correction = format!(
                "{prompt}\n\nYour previous response was invalid. Return only the required tool_calls JSON object now."
            );
            output = state
                .gemini
                .generate_for(account.as_deref(), &correction, &model, 0)
                .await?;
            calls = extract_tool_calls(&output.text);
        }
        if calls.is_empty() && tool_choice_required(request.tool_choice.as_ref()) {
            return Err(GatewayError::Provider(
                "model did not produce the required function call".into(),
            ));
        }
        if request.stream {
            let event_stream = async_stream::stream! {
                if calls.is_empty() {
                    yield Ok::<Event, Infallible>(Event::default().data(json!({
                        "id": request_id, "object": "chat.completion.chunk", "created": created,
                        "model": model, "choices": [{"index": 0, "delta": {"role":"assistant","content":output.text}, "finish_reason": null}]
                    }).to_string()));
                    yield Ok(Event::default().data(json!({
                        "id": request_id, "object": "chat.completion.chunk", "created": created,
                        "model": model, "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
                    }).to_string()));
                } else {
                    let tool_calls = calls.iter().enumerate().map(|(index, call)| json!({
                        "index": index, "id": call.id, "type": "function",
                        "function": {"name": call.name, "arguments": call.arguments}
                    })).collect::<Vec<_>>();
                    yield Ok::<Event, Infallible>(Event::default().data(json!({
                        "id": request_id, "object": "chat.completion.chunk", "created": created,
                        "model": model, "choices": [{"index": 0, "delta": {"role":"assistant","tool_calls":tool_calls}, "finish_reason": null}]
                    }).to_string()));
                    yield Ok(Event::default().data(json!({
                        "id": request_id, "object": "chat.completion.chunk", "created": created,
                        "model": model, "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]
                    }).to_string()));
                }
                yield Ok(Event::default().data("[DONE]"));
            };
            return Ok(Sse::new(event_stream).into_response());
        }
        if !calls.is_empty() {
            let tool_calls = calls
                .into_iter()
                .map(|call| {
                    json!({
                        "id": call.id, "type": "function",
                        "function": {"name": call.name, "arguments": call.arguments}
                    })
                })
                .collect::<Vec<_>>();
            return Ok(Json(json!({
                "id": request_id, "object": "chat.completion", "created": created, "model": model,
                "choices": [{"index": 0, "message": {"role":"assistant","content":null,"tool_calls":tool_calls}, "finish_reason":"tool_calls"}],
                "usage": {"prompt_tokens":0,"completion_tokens":0,"total_tokens":0}
            })).into_response());
        }
        return Ok(Json(json!({
            "id": request_id, "object": "chat.completion", "created": created, "model": model,
            "choices": [{"index":0,"message":{"role":"assistant","content":output.text},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":0,"completion_tokens":0,"total_tokens":0}
        })).into_response());
    }

    if request.stream {
        let mut provider = state
            .gemini
            .generate_stream_for(account.as_deref(), &prompt, &model, 0)
            .await?;
        let event_stream = async_stream::stream! {
            while let Some(update) = provider.next().await {
                match update {
                    Ok(update) if !update.text_delta.is_empty() => {
                        let payload = json!({
                            "id": request_id,
                            "object": "chat.completion.chunk",
                            "created": created,
                            "model": model,
                            "choices": [{
                                "index": 0,
                                "delta": {"role": "assistant", "content": update.text_delta},
                                "finish_reason": null,
                            }],
                        });
                        yield Ok::<Event, Infallible>(Event::default().data(payload.to_string()));
                    }
                    Ok(_) => {}
                    Err(error) => {
                        yield Ok(Event::default().data(json!({
                            "error": {"message": error.to_string(), "type": "gemini_error"}
                        }).to_string()));
                        yield Ok(Event::default().data("[DONE]"));
                        return;
                    }
                }
            }
            yield Ok(Event::default().data(json!({
                "id": request_id,
                "object": "chat.completion.chunk",
                "created": created,
                "model": model,
                "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
            }).to_string()));
            yield Ok(Event::default().data("[DONE]"));
        };
        return Ok(Sse::new(event_stream)
            .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
            .into_response());
    }

    let output = state
        .gemini
        .generate_for(account.as_deref(), &prompt, &model, 0)
        .await?;
    let mut text = output.text;
    if !output.generated_images.is_empty() {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(
            &output
                .generated_images
                .iter()
                .map(|url| format!("![Generated image]({url})"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
    Ok(Json(json!({
        "id": request_id,
        "object": "chat.completion",
        "created": created,
        "model": model,
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": text},
            "finish_reason": "stop",
        }],
        "usage": {"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0},
        "images": output.generated_images,
    }))
    .into_response())
}

fn messages_to_prompt(config: &crate::config::Config, messages: &[Message]) -> Result<String> {
    if messages.is_empty() || messages.len() > config.max_messages {
        return Err(GatewayError::InvalidRequest(format!(
            "messages must contain 1..={} items",
            config.max_messages
        )));
    }
    let mut sections = Vec::new();
    let mut character_count = 0usize;
    for message in messages {
        let mut text = message
            .content
            .as_ref()
            .map(content_to_text)
            .unwrap_or_default();
        if !message.tool_calls.is_empty() {
            text.push_str("\nFunction calls requested: ");
            text.push_str(&serde_json::to_string(&message.tool_calls)?);
        }
        let text = text.trim().to_owned();
        if text.is_empty() {
            continue;
        }
        let label = match message.role.to_ascii_lowercase().as_str() {
            "system" => "System instructions",
            "assistant" => "Assistant",
            "tool" | "function" => "Tool result",
            _ => "User",
        };
        let identifier = message
            .tool_call_id
            .as_deref()
            .or(message.name.as_deref())
            .map(|value| format!(" ({value})"))
            .unwrap_or_default();
        let section = format!("{label}{identifier}:\n{text}");
        character_count += section.chars().count() + 2;
        if character_count > config.max_prompt_chars {
            return Err(GatewayError::InvalidRequest(format!(
                "combined message content exceeds {} characters",
                config.max_prompt_chars
            )));
        }
        sections.push(section);
    }
    if sections.is_empty() {
        return Err(GatewayError::InvalidRequest(
            "messages cannot be empty".into(),
        ));
    }
    Ok(format!("{}\n\nAssistant:", sections.join("\n\n")))
}

fn content_to_text(content: &MessageContent) -> String {
    match content {
        MessageContent::Text(value) => value.clone(),
        MessageContent::Parts(parts) => parts
            .iter()
            .filter_map(|part| match part.get("type").and_then(Value::as_str) {
                Some("text") => part.get("text").and_then(Value::as_str).map(str::to_owned),
                Some("image_url") => {
                    let image = part.get("image_url")?;
                    let url = image
                        .as_str()
                        .or_else(|| image.get("url").and_then(Value::as_str))?;
                    Some(format!("[User image: {url}]"))
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_config() -> crate::config::Config {
        crate::config::Config {
            project_root: std::path::PathBuf::from("."),
            host: "127.0.0.1".into(),
            port: 8000,
            api_key: "test-key-1234".into(),
            admin_token: "admin-key-1234".into(),
            secure_1psid: "psid".into(),
            secure_1psidts: None,
            auth_user: 0,
            accounts_file: std::path::PathBuf::from(".gemini.accounts.local.json"),
            account_cooldown: std::time::Duration::from_secs(60),
            account_probe_interval: std::time::Duration::from_secs(60),
            proxy: None,
            provider_origin: "https://gemini.google.com".into(),
            retry_attempts: 3,
            retry_delay: std::time::Duration::from_millis(10),
            cookie_sync_enabled: false,
            remote_admin_enabled: false,
            default_model: "gemini-3-flash".into(),
            advanced_model: "gemini-3-pro".into(),
            enabled_generations: vec![],
            data_root: std::path::PathBuf::from("."),
            public_base_url: "http://127.0.0.1:8000".into(),
            max_concurrency: 1,
            max_account_concurrency: 1,
            task_workers: 1,
            max_task_queue: 20,
            task_retention: std::time::Duration::from_secs(7 * 24 * 3600),
            queue_timeout: std::time::Duration::from_secs(30),
            provider_timeout: std::time::Duration::from_secs(90),
            max_messages: 50,
            max_prompt_chars: 200000,
            max_request_bytes: 1048576,
            cors_origins: vec![],
        }
    }

    #[test]
    fn multipart_content_is_flattened() {
        let content = MessageContent::Parts(vec![
            json!({"type": "text", "text": "hello"}),
            json!({"type": "image_url", "image_url": {"url": "https://example/x.png"}}),
        ]);
        assert_eq!(
            content_to_text(&content),
            "hello\n[User image: https://example/x.png]"
        );
    }

    #[test]
    fn messages_to_prompt_formats_roles() {
        let config = base_config();
        let messages = vec![
            Message {
                role: "system".into(),
                content: Some(MessageContent::Text("be helpful".into())),
                name: None,
                tool_call_id: None,
                tool_calls: vec![],
            },
            Message {
                role: "user".into(),
                content: Some(MessageContent::Text("hi".into())),
                name: None,
                tool_call_id: None,
                tool_calls: vec![],
            },
        ];
        let prompt = messages_to_prompt(&config, &messages).unwrap();
        assert!(prompt.contains("System instructions:\nbe helpful"));
        assert!(prompt.contains("User:\nhi"));
        assert!(prompt.ends_with("Assistant:"));
    }

    #[test]
    fn messages_to_prompt_rejects_empty() {
        let config = base_config();
        let messages: Vec<Message> = vec![];
        assert!(messages_to_prompt(&config, &messages).is_err());
    }

    #[test]
    fn messages_to_prompt_rejects_too_many() {
        let mut config = base_config();
        config.max_messages = 1;
        let messages = vec![
            Message {
                role: "user".into(),
                content: Some(MessageContent::Text("a".into())),
                name: None,
                tool_call_id: None,
                tool_calls: vec![],
            },
            Message {
                role: "user".into(),
                content: Some(MessageContent::Text("b".into())),
                name: None,
                tool_call_id: None,
                tool_calls: vec![],
            },
        ];
        assert!(messages_to_prompt(&config, &messages).is_err());
    }

    #[test]
    fn messages_to_prompt_rejects_oversized() {
        let mut config = base_config();
        config.max_prompt_chars = 5;
        let messages = vec![Message {
            role: "user".into(),
            content: Some(MessageContent::Text("hello world".into())),
            name: None,
            tool_call_id: None,
            tool_calls: vec![],
        }];
        assert!(messages_to_prompt(&config, &messages).is_err());
    }
}
