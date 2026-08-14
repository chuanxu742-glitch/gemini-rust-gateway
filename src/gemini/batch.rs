use serde_json::json;

use crate::error::GatewayError;

use super::session::{SessionMetadata, app_path, app_url, batch_url};

pub(super) async fn batch_execute(
    http: &reqwest::Client,
    rpcid: &str,
    payload: &str,
    metadata: &SessionMetadata,
    request_id: u64,
    auth_user: usize,
    origin: &str,
) -> Result<String, GatewayError> {
    let serialized = json!([[[rpcid, payload, null, "generic"]]]).to_string();
    let mut query = vec![
        ("rpcids", rpcid.to_owned()),
        ("hl", metadata.language.clone()),
        ("_reqid", request_id.to_string()),
        ("rt", "c".into()),
        ("source-path", app_path(auth_user)),
    ];
    if let Some(value) = &metadata.build_label {
        query.push(("bl", value.clone()));
    }
    if let Some(value) = &metadata.session_id {
        query.push(("f.sid", value.clone()));
    }
    let response = http
        .post(batch_url(origin, auth_user))
        .query(&query)
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded;charset=utf-8",
        )
        .header(reqwest::header::ORIGIN, origin)
        .header(reqwest::header::REFERER, app_url(origin, auth_user))
        .header("x-same-domain", "1")
        .header(
            "x-goog-ext-525001261-jspb",
            "[1,null,null,null,null,null,null,null,[4]]",
        )
        .header("x-goog-ext-73010989-jspb", "[0]")
        .form(&[
            ("at", metadata.access_token.clone().unwrap_or_default()),
            ("f.req", serialized),
        ])
        .send()
        .await?
        .error_for_status()?;
    Ok(response.text().await?)
}
