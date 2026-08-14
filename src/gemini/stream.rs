use std::{pin::Pin, sync::Arc};

use futures_util::{Stream, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{RwLock, Semaphore};
use uuid::Uuid;

use crate::{config::Config, error::GatewayError};

use super::{
    framing::{PathPart, nested, parse_frames},
    protocol::{model_headers, update_output},
    session::{SessionMetadata, app_url, generate_url},
};

pub type DeltaStream =
    Pin<Box<dyn Stream<Item = Result<super::GeminiDelta, GatewayError>> + Send + 'static>>;

pub(super) struct StreamContext {
    pub metadata: SessionMetadata,
    pub request_uuid: String,
    pub outer: String,
    pub query: Vec<(&'static str, String)>,
    pub auth_user: usize,
    pub origin: String,
}

pub(super) fn build_stream_context(
    prompt: &str,
    model: &str,
    feature_mode: i64,
    metadata: &SessionMetadata,
    request_id: u64,
    auth_user: usize,
    origin: &str,
) -> Result<StreamContext, GatewayError> {
    let request_uuid = Uuid::new_v4().to_string().to_uppercase();

    let mut inner = vec![Value::Null; 69];
    inner[0] = json!([prompt, 0, null, [], null, null, 0]);
    inner[1] = json!([metadata.language]);
    inner[2] = json!([null, null, null, null, null, null, null, null, null, ""]);
    inner[6] = json!([1]);
    inner[7] = json!(1);
    inner[10] = json!(1);
    inner[11] = json!(0);
    inner[17] = json!([[0]]);
    inner[18] = json!(0);
    inner[27] = json!(1);
    inner[30] = json!([4]);
    inner[41] = json!([1]);
    // Normal chat and Canvas can be temporary. Media tools must create a
    // history item so their delayed assets remain retrievable.
    if feature_mode == 0 || feature_mode == 2 {
        inner[45] = json!(1);
    }
    if feature_mode != 0 {
        inner[49] = json!(feature_mode);
    }
    inner[53] = json!(0);
    if feature_mode == 1 {
        inner[54] = json!([[[[[1]]]]]);
        inner[55] = json!([[1]]);
    }
    inner[59] = json!(request_uuid.clone());
    inner[61] = json!([]);
    inner[68] = json!(2);

    let inner_string = serde_json::to_string(&inner)?;
    let outer = serde_json::to_string(&json!([null, inner_string]))?;
    let mut query = vec![
        ("hl", metadata.language.clone()),
        ("_reqid", request_id.to_string()),
        ("rt", "c".into()),
    ];
    if let Some(value) = &metadata.build_label {
        query.push(("bl", value.clone()));
    }
    if let Some(value) = &metadata.session_id {
        query.push(("f.sid", value.clone()));
    }

    let _ = model; // model_headers applied at request build time
    Ok(StreamContext {
        metadata: metadata.clone(),
        request_uuid,
        outer,
        query,
        auth_user,
        origin: origin.to_owned(),
    })
}

pub(super) fn build_request(
    http: &reqwest::Client,
    ctx: &StreamContext,
    model: &str,
) -> reqwest::RequestBuilder {
    let mut request = http
        .post(generate_url(&ctx.origin, ctx.auth_user))
        .query(&ctx.query)
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded;charset=utf-8",
        )
        .header(reqwest::header::ORIGIN, &ctx.origin)
        .header(
            reqwest::header::REFERER,
            app_url(&ctx.origin, ctx.auth_user),
        )
        .header("x-same-domain", "1")
        .header(
            "x-goog-ext-525005358-jspb",
            format!(r#"["{}",1]"#, ctx.request_uuid),
        );
    for (name, value) in model_headers(model) {
        request = request.header(name, value);
    }
    request.form(&[
        ("at", ctx.metadata.access_token.clone().unwrap_or_default()),
        ("f.req", ctx.outer.clone()),
    ])
}

pub(super) fn parse_response_stream<G>(
    body: impl Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
    timeout: std::time::Duration,
    guards: G,
) -> DeltaStream
where
    G: Send + 'static,
{
    let output_stream = async_stream::try_stream! {
        let _guards = guards;
        let mut buffer = String::new();
        let mut previous_text = String::new();
        let mut latest = super::super::domain::output::GeminiOutput::default();
        let mut body = Box::pin(body);
        while let Some(chunk) = tokio::time::timeout(timeout, body.next())
            .await
            .map_err(|_| GatewayError::Timeout)?
        {
            let chunk = chunk.map_err(|error| {
                GatewayError::Provider(format!("Gemini response stream failed: {error}"))
            })?;
            match std::str::from_utf8(&chunk) {
                Ok(text) => buffer.push_str(text),
                Err(_) => buffer.push_str(&String::from_utf8_lossy(&chunk)),
            }
            if buffer.starts_with(")]}'") {
                buffer.drain(..4);
                let leading = buffer.len() - buffer.trim_start().len();
                if leading > 0 {
                    buffer.drain(..leading);
                }
            }
            let (parts, remainder) = parse_frames(&buffer);
            buffer = remainder;
            for part in parts {
                if let Some(error_code) = nested(
                    &part,
                    &[PathPart::Index(5), PathPart::Index(2), PathPart::Index(0), PathPart::Index(1), PathPart::Index(0)],
                ).and_then(Value::as_i64) {
                    Err(GatewayError::Provider(format!("Gemini error code {error_code}")))?;
                }
                let Some(inner_json) = nested(&part, &[PathPart::Index(2)]).and_then(Value::as_str) else {
                    continue;
                };
                let Ok(parsed) = serde_json::from_str::<Value>(inner_json) else {
                    continue;
                };
                update_output(&mut latest, &parsed);
                if latest.text.is_empty() && latest.generated_images.is_empty()
                    && latest.generated_videos.is_empty() && latest.generated_media.is_empty()
                {
                    continue;
                }
                let delta = latest.text.strip_prefix(&previous_text)
                    .unwrap_or(&latest.text)
                    .to_owned();
                previous_text = latest.text.clone();
                yield super::GeminiDelta {
                    output: latest.clone(),
                    text_delta: delta,
                    account_id: String::new(),
                };
            }
        }
    };
    Box::pin(output_stream)
}

#[allow(dead_code)]
pub(super) fn _typecheck(_: &Arc<Config>, _: &RwLock<SessionMetadata>, _: Arc<Semaphore>) {}
