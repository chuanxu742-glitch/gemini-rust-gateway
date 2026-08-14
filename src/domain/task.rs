use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::generation::GenerationRequest;

pub const TERMINAL_STATES: &[&str] =
    &["completed", "failed", "interrupted", "rejected", "canceled"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRecord {
    pub id: String,
    pub object: String,
    pub kind: super::generation::GenerationKind,
    pub status: String,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<GenerationRequest>,
    pub progress: Option<Value>,
    pub result: Option<Value>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PublicTaskRecord {
    pub id: String,
    pub object: String,
    pub kind: super::generation::GenerationKind,
    pub status: String,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    pub progress: Option<Value>,
    pub result: Option<Value>,
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl TaskRecord {
    pub fn public(&self, base_url: Option<&str>) -> PublicTaskRecord {
        PublicTaskRecord {
            id: self.id.clone(),
            object: self.object.clone(),
            kind: self.kind.clone(),
            status: self.status.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            account_id: self.account_id.clone(),
            progress: self.progress.clone(),
            result: self.result.clone(),
            error: self.error.clone(),
            url: base_url.map(|base| format!("{base}/v1/tasks/{}", self.id)),
        }
    }

    pub fn terminal(&self) -> bool {
        TERMINAL_STATES.contains(&self.status.as_str())
    }
}
