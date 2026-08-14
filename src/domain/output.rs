#[derive(Debug, Clone, Default)]
pub struct GeminiOutput {
    pub text: String,
    pub cid: Option<String>,
    pub rid: Option<String>,
    pub rcid: Option<String>,
    pub generated_images: Vec<String>,
    pub generated_videos: Vec<String>,
    pub generated_media: Vec<String>,
}
