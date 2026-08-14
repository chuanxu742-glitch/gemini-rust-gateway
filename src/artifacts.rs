use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use regex::Regex;
use tokio::{fs, io::AsyncWriteExt};

use crate::{
    domain::artifact::Artifact,
    error::{GatewayError, Result},
};

#[derive(Debug, Clone)]
pub struct ArtifactStore {
    root: PathBuf,
    public_base_url: String,
}

impl ArtifactStore {
    pub async fn new(root: PathBuf, public_base_url: String) -> Result<Self> {
        fs::create_dir_all(&root).await?;
        Ok(Self {
            root: root.canonicalize()?,
            public_base_url: public_base_url.trim_end_matches('/').to_owned(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub async fn task_dir(&self, task_id: &str) -> Result<PathBuf> {
        validate_task_id(task_id)?;
        let path = self.root.join(task_id);
        fs::create_dir_all(&path).await?;
        Ok(path)
    }

    pub async fn remove_task(&self, task_id: &str) -> Result<()> {
        validate_task_id(task_id)?;
        let path = self.root.join(task_id);
        if !path.exists() {
            return Ok(());
        }
        let canonical = path.canonicalize()?;
        if canonical.parent() != Some(self.root.as_path()) {
            return Err(GatewayError::InvalidRequest(
                "artifact task path is outside the configured root".into(),
            ));
        }
        fs::remove_dir_all(canonical).await?;
        Ok(())
    }

    pub async fn write_text(
        &self,
        task_id: &str,
        filename: &str,
        content: &str,
    ) -> Result<Artifact> {
        validate_filename(filename)?;
        let path = self.task_dir(task_id).await?.join(filename);
        fs::write(&path, content.as_bytes()).await?;
        self.describe(&path, None).await
    }

    pub async fn download(
        &self,
        http: &reqwest::Client,
        task_id: &str,
        filename_stem: &str,
        url: &str,
        role: Option<String>,
    ) -> Result<Artifact> {
        validate_filename(filename_stem)?;
        let response = http.get(url).send().await?.error_for_status()?;
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .unwrap_or("application/octet-stream")
            .to_owned();
        let extension = mime_guess::get_mime_extensions_str(&content_type)
            .and_then(|items| items.first().copied())
            .unwrap_or("bin");
        let path = self
            .task_dir(task_id)
            .await?
            .join(format!("{filename_stem}.{extension}"));
        let temporary = path.with_extension(format!("{extension}.part"));
        let mut file = fs::File::create(&temporary).await?;
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            file.write_all(&chunk?).await?;
        }
        file.flush().await?;
        drop(file);
        fs::rename(&temporary, &path).await?;
        self.describe(&path, role).await
    }

    async fn describe(&self, path: &Path, role: Option<String>) -> Result<Artifact> {
        let canonical = path.canonicalize()?;
        if !canonical.starts_with(&self.root) {
            return Err(GatewayError::InvalidRequest(
                "artifact path is outside the configured data directory".into(),
            ));
        }
        let relative = canonical
            .strip_prefix(&self.root)
            .map_err(|_| GatewayError::InvalidRequest("invalid artifact path".into()))?
            .to_string_lossy()
            .replace('\\', "/");
        let relative_url = format!("/files/{relative}");
        let metadata = fs::metadata(&canonical).await?;
        Ok(Artifact {
            name: canonical
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("artifact")
                .to_owned(),
            url: format!("{}{}", self.public_base_url, relative_url),
            relative_url,
            content_type: mime_guess::from_path(&canonical)
                .first_or_octet_stream()
                .essence_str()
                .to_owned(),
            size: metadata.len(),
            role,
        })
    }
}

fn validate_filename(value: &str) -> Result<()> {
    if value.is_empty()
        || value.contains('/')
        || value.contains('\\')
        || value == "."
        || value == ".."
    {
        return Err(GatewayError::InvalidRequest(
            "invalid artifact filename".into(),
        ));
    }
    Ok(())
}

fn validate_task_id(task_id: &str) -> Result<()> {
    if task_id.len() != 32 || !task_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(GatewayError::InvalidRequest(
            "invalid artifact task id".into(),
        ));
    }
    Ok(())
}

pub fn extract_standalone_html(text: &str) -> String {
    let fenced = Regex::new(r"(?is)```(?:html)?\s*(.*?)```").expect("HTML fence regex");
    let candidate = fenced
        .captures(text)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str().trim())
        .unwrap_or_else(|| text.trim());
    let html_start = Regex::new(r"(?i)(<!doctype\s+html|<html\b)")
        .expect("HTML start regex")
        .find(candidate)
        .map(|value| value.start());
    if let Some(start) = html_start {
        return candidate[start..].to_owned();
    }
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Gemini Canvas</title></head><body><main><pre style=\"white-space:pre-wrap\">{}</pre></main></body></html>",
        escape_html(candidate)
    )
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_fenced_html() {
        let output = extract_standalone_html("before\n```html\n<html>ok</html>\n```");
        assert_eq!(output, "<html>ok</html>");
    }

    #[test]
    fn escapes_plain_text() {
        let output = extract_standalone_html("<unsafe & text");
        assert!(output.contains("&lt;unsafe &amp; text"));
    }
}
