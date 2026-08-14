use std::time::Duration;

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};

use crate::{
    domain::generation::{GenerationKind, GenerationRequest, ImageGenerationRequest},
    error::{GatewayError, Result},
    middleware::auth::{authorize, requested_account},
    util::time::epoch_seconds,
};

use super::AppState;

pub async fn create_generation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut request): Json<GenerationRequest>,
) -> Result<(StatusCode, Json<Value>)> {
    authorize(&state.config, &headers)?;
    if let Some(header_account) = requested_account(&headers)? {
        request.account_id = Some(header_account);
    }
    request.account_id = Some(
        state
            .gemini
            .resolve_account(request.account_id.as_deref())
            .await?,
    );
    validate_generation(&state.config, &request)?;
    if request.model.is_none() {
        request.model = Some(default_generation_model(&state.config, &request.kind));
    }
    let task = state.tasks.submit(request).await?;
    Ok((StatusCode::ACCEPTED, Json(serde_json::to_value(task)?)))
}

pub async fn image_generations(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ImageGenerationRequest>,
) -> Result<Response> {
    authorize(&state.config, &headers)?;
    let account_id = state
        .gemini
        .resolve_account(requested_account(&headers)?.as_deref())
        .await?;
    if request.response_format.as_deref().unwrap_or("url") != "url" {
        return Err(GatewayError::InvalidRequest(
            "only response_format=url is supported".into(),
        ));
    }
    let generation = GenerationRequest {
        kind: GenerationKind::Image,
        prompt: request.prompt,
        model: Some(
            request
                .model
                .unwrap_or_else(|| state.config.default_model.clone()),
        ),
        account_id: Some(account_id),
        count: request.count,
        download_type: "audio".into(),
        poll_interval: 10.0,
        timeout: 900.0,
    };
    validate_generation(&state.config, &generation)?;
    let submitted = state.tasks.submit(generation).await?;
    let task = state
        .tasks
        .wait(&submitted.id, Duration::from_secs(960))
        .await?;
    if task.status != "completed" {
        return Err(GatewayError::Provider(
            task.error.unwrap_or_else(|| "image task failed".into()),
        ));
    }
    let data = task
        .result
        .as_ref()
        .and_then(|value| value.get("files"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| {
            item.get("content_type")
                .and_then(Value::as_str)
                .is_some_and(|value| value.starts_with("image/"))
        })
        .filter_map(|item| item.get("url").and_then(Value::as_str))
        .map(|url| json!({"url": url}))
        .collect::<Vec<_>>();
    Ok(Json(json!({"created": epoch_seconds(), "data": data})).into_response())
}

pub(super) fn validate_generation(
    config: &crate::config::Config,
    request: &GenerationRequest,
) -> Result<()> {
    let kind = request.kind.as_str();
    if !config.enabled_generations.iter().any(|item| item == kind) {
        return Err(GatewayError::FeatureDisabled(kind.into()));
    }
    let prompt_chars = request.prompt.chars().count();
    if prompt_chars == 0 || prompt_chars > config.max_prompt_chars {
        return Err(GatewayError::InvalidRequest(format!(
            "prompt must contain 1..={} characters",
            config.max_prompt_chars
        )));
    }
    if !(1..=4).contains(&request.count) {
        return Err(GatewayError::InvalidRequest(
            "count must be between 1 and 4".into(),
        ));
    }
    if !(2.0..=60.0).contains(&request.poll_interval) {
        return Err(GatewayError::InvalidRequest(
            "poll_interval must be between 2 and 60".into(),
        ));
    }
    if !(30.0..=3600.0).contains(&request.timeout) {
        return Err(GatewayError::InvalidRequest(
            "timeout must be between 30 and 3600".into(),
        ));
    }
    Ok(())
}

pub(super) fn default_generation_model(
    config: &crate::config::Config,
    kind: &GenerationKind,
) -> String {
    if matches!(
        kind,
        GenerationKind::Video
            | GenerationKind::Audio
            | GenerationKind::Music
            | GenerationKind::DeepResearch
    ) {
        config.advanced_model.clone()
    } else {
        config.default_model.clone()
    }
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
            enabled_generations: vec!["image".into(), "video".into()],
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
    fn validate_rejects_disabled_kind() {
        let config = base_config();
        let request = GenerationRequest {
            kind: GenerationKind::Canvas,
            prompt: "hello".into(),
            model: None,
            account_id: None,
            count: 1,
            download_type: "audio".into(),
            poll_interval: 10.0,
            timeout: 900.0,
        };
        assert!(validate_generation(&config, &request).is_err());
    }

    #[test]
    fn validate_rejects_empty_prompt() {
        let config = base_config();
        let request = GenerationRequest {
            kind: GenerationKind::Image,
            prompt: String::new(),
            model: None,
            account_id: None,
            count: 1,
            download_type: "audio".into(),
            poll_interval: 10.0,
            timeout: 900.0,
        };
        assert!(validate_generation(&config, &request).is_err());
    }

    #[test]
    fn validate_rejects_count_out_of_range() {
        let config = base_config();
        let request = GenerationRequest {
            kind: GenerationKind::Image,
            prompt: "hello".into(),
            model: None,
            account_id: None,
            count: 5,
            download_type: "audio".into(),
            poll_interval: 10.0,
            timeout: 900.0,
        };
        assert!(validate_generation(&config, &request).is_err());
    }

    #[test]
    fn validate_rejects_timeout_out_of_range() {
        let config = base_config();
        let request = GenerationRequest {
            kind: GenerationKind::Image,
            prompt: "hello".into(),
            model: None,
            account_id: None,
            count: 1,
            download_type: "audio".into(),
            poll_interval: 10.0,
            timeout: 10.0,
        };
        assert!(validate_generation(&config, &request).is_err());
    }

    #[test]
    fn default_model_picks_advanced_for_media() {
        let config = base_config();
        assert_eq!(
            default_generation_model(&config, &GenerationKind::Video),
            "gemini-3-pro"
        );
        assert_eq!(
            default_generation_model(&config, &GenerationKind::Image),
            "gemini-3-flash"
        );
    }
}
