use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GenerationKind {
    Image,
    Video,
    Audio,
    Music,
    DeepResearch,
    Canvas,
}

impl GenerationKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Video => "video",
            Self::Audio => "audio",
            Self::Music => "music",
            Self::DeepResearch => "deep_research",
            Self::Canvas => "canvas",
        }
    }

    pub fn feature_mode(&self) -> i64 {
        match self {
            Self::DeepResearch => 1,
            Self::Canvas => 2,
            Self::Video => 11,
            Self::Image => 14,
            Self::Audio | Self::Music => 21,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GenerationRequest {
    #[serde(rename = "type")]
    pub kind: GenerationKind,
    pub prompt: String,
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default = "default_count")]
    pub count: usize,
    #[serde(default = "default_download_type")]
    pub download_type: String,
    #[serde(default = "default_poll_interval")]
    pub poll_interval: f64,
    #[serde(default = "default_timeout")]
    pub timeout: f64,
}

fn default_count() -> usize {
    1
}
fn default_download_type() -> String {
    "audio".into()
}
fn default_poll_interval() -> f64 {
    10.0
}
fn default_timeout() -> f64 {
    900.0
}

#[derive(Debug, Clone, Deserialize)]
pub struct ImageGenerationRequest {
    pub prompt: String,
    pub model: Option<String>,
    #[serde(default = "default_count", rename = "n")]
    pub count: usize,
    #[serde(default)]
    pub response_format: Option<String>,
}
