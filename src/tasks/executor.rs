use std::sync::Arc;

use serde_json::{Value, json};

use crate::{
    artifacts::ArtifactStore,
    config::Config,
    domain::{
        generation::{GenerationKind, GenerationRequest},
        output::GeminiOutput,
    },
    error::{GatewayError, Result},
    gemini::GeminiClient,
    util::time::truncate,
};

pub(super) struct TaskExecutor {
    config: Arc<Config>,
    gemini: Arc<GeminiClient>,
    artifacts: ArtifactStore,
}

impl TaskExecutor {
    pub(super) fn new(
        config: Arc<Config>,
        gemini: Arc<GeminiClient>,
        artifacts: ArtifactStore,
    ) -> Self {
        Self {
            config,
            gemini,
            artifacts,
        }
    }

    pub(super) async fn execute(
        &self,
        task_id: &str,
        request: &GenerationRequest,
    ) -> Result<Value> {
        let model = request.model.as_deref().unwrap_or_else(|| {
            if matches!(
                request.kind,
                GenerationKind::Video
                    | GenerationKind::Audio
                    | GenerationKind::Music
                    | GenerationKind::DeepResearch
            ) {
                &self.config.advanced_model
            } else {
                &self.config.default_model
            }
        });
        let prompt = build_prompt(request);
        let account_id = request.account_id.as_deref().ok_or_else(|| {
            GatewayError::InvalidRequest("generation task has no bound account_id".into())
        })?;
        let output = self
            .gemini
            .generate_for(
                Some(account_id),
                &prompt,
                model,
                request.kind.feature_mode(),
            )
            .await?;
        let files = self.collect_files(task_id, request, &output).await?;
        Ok(json!({
            "model": model,
            "account_id": account_id,
            "text": truncate(&output.text, 4000),
            "files": files,
        }))
    }

    async fn collect_files(
        &self,
        task_id: &str,
        request: &GenerationRequest,
        output: &GeminiOutput,
    ) -> Result<Vec<Value>> {
        let mut files = Vec::new();
        let account_id = request.account_id.as_deref().ok_or_else(|| {
            GatewayError::InvalidRequest("generation task has no bound account_id".into())
        })?;
        let http = self.gemini.http_for(account_id).await?;
        match request.kind {
            GenerationKind::Canvas => {
                let html = crate::artifacts::extract_standalone_html(&output.text);
                files.push(
                    self.artifacts
                        .write_text(task_id, "canvas.html", &html)
                        .await?,
                );
            }
            GenerationKind::DeepResearch => {
                if output.text.is_empty() {
                    return Err(GatewayError::Provider(
                        "Gemini returned no research text".into(),
                    ));
                }
                files.push(
                    self.artifacts
                        .write_text(task_id, "research.md", &output.text)
                        .await?,
                );
            }
            GenerationKind::Image => {
                for (index, url) in output
                    .generated_images
                    .iter()
                    .take(request.count)
                    .enumerate()
                {
                    files.push(
                        self.artifacts
                            .download(
                                &http,
                                task_id,
                                &format!("image_{}", index + 1),
                                url,
                                Some("image".into()),
                            )
                            .await?,
                    );
                }
                if files.is_empty() {
                    return Err(GatewayError::Provider(format!(
                        "Gemini returned no generated image{}",
                        provider_text_suffix(&output.text)
                    )));
                }
            }
            GenerationKind::Video => {
                for (index, url) in output.generated_videos.iter().enumerate() {
                    files.push(
                        self.artifacts
                            .download(
                                &http,
                                task_id,
                                &format!("video_{}", index + 1),
                                url,
                                Some("video".into()),
                            )
                            .await?,
                    );
                }
                if files.is_empty() {
                    return Err(GatewayError::Provider(
                        "Gemini returned no generated video".into(),
                    ));
                }
            }
            GenerationKind::Audio | GenerationKind::Music => {
                for (index, url) in output.generated_media.iter().enumerate() {
                    files.push(
                        self.artifacts
                            .download(
                                &http,
                                task_id,
                                &format!("media_{}", index + 1),
                                url,
                                Some(request.download_type.clone()),
                            )
                            .await?,
                    );
                }
                if files.is_empty() {
                    return Err(GatewayError::Provider(
                        "Gemini returned no generated media".into(),
                    ));
                }
            }
        }
        Ok(files
            .into_iter()
            .map(|artifact| serde_json::to_value(artifact).unwrap_or(Value::Null))
            .collect())
    }
}

fn build_prompt(request: &GenerationRequest) -> String {
    match request.kind {
        GenerationKind::Image => format!(
            "Use the image generation tool. Generate exactly {} original image{}. Do not fetch images from the web.\n\n{}",
            request.count,
            if request.count == 1 { "" } else { "s" },
            request.prompt,
        ),
        GenerationKind::Video => format!(
            "Use the video generation tool. Generate a video for this request:\n\n{}",
            request.prompt
        ),
        GenerationKind::Audio => format!(
            "Use the media generation tool. Generate an audio clip for this request:\n\n{}",
            request.prompt
        ),
        GenerationKind::Music => format!(
            "Use the media generation tool. Generate a music track for this request:\n\n{}",
            request.prompt
        ),
        GenerationKind::DeepResearch => request.prompt.clone(),
        GenerationKind::Canvas => format!(
            "Create a polished, interactive, self-contained HTML canvas artifact for the request below. Return exactly one complete HTML document. Put all CSS and JavaScript inline. Do not use Markdown fences, external packages, remote assets, or network requests. Make it responsive and accessible.\n\nUser request:\n{}",
            request.prompt,
        ),
    }
}

fn provider_text_suffix(value: &str) -> String {
    let summary = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if summary.is_empty() {
        String::new()
    } else {
        format!("; provider text: {}", truncate(&summary, 240))
    }
}
