//! Minimal Rust client. Add dependencies:
//! reqwest = { version = "0.12", features = ["json"] }
//! serde = { version = "1", features = ["derive"] }
//! serde_json = "1"
//! tokio = { version = "1", features = ["full"] }

use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

const BASE_URL: &str = "http://127.0.0.1:8000/v1";
const API_KEY: &str = "local-gemini-key";

#[derive(Debug, Deserialize)]
struct Task {
    id: String,
    status: String,
    result: Option<serde_json::Value>,
    error: Option<String>,
}

async fn generate(kind: &str, prompt: &str) -> Result<Task, reqwest::Error> {
    let client = reqwest::Client::new();
    let mut task: Task = client
        .post(format!("{BASE_URL}/generations"))
        .bearer_auth(API_KEY)
        .json(&json!({ "type": kind, "prompt": prompt }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    loop {
        task = client
            .get(format!("{BASE_URL}/tasks/{}", task.id))
            .bearer_auth(API_KEY)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        match task.status.as_str() {
            "completed" | "failed" | "interrupted" | "canceled" => return Ok(task),
            _ => tokio::time::sleep(Duration::from_secs(3)).await,
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let task = generate("canvas", "Build an interactive Pomodoro timer").await?;
    if task.status != "completed" {
        return Err(std::io::Error::other(task.error.unwrap_or(task.status)).into());
    }
    println!("{:#?}", task.result);
    Ok(())
}
