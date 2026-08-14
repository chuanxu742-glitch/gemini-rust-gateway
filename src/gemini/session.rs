use std::sync::Arc;

use regex::Regex;
use reqwest::{Client, Proxy, cookie::Jar, header};
use wreq_util::Emulation;

use crate::{config::Config, error::GatewayError};

use super::protocol;

const GEMINI_ORIGIN: &str = "https://gemini.google.com";

pub(super) fn app_path(auth_user: usize) -> String {
    format!("/u/{auth_user}/app")
}

pub(super) fn app_url(origin: &str, auth_user: usize) -> String {
    format!("{origin}{}", app_path(auth_user))
}

pub(super) fn generate_url(origin: &str, auth_user: usize) -> String {
    format!(
        "{origin}/u/{auth_user}/_/BardChatUi/data/assistant.lamda.BardFrontendService/StreamGenerate"
    )
}

pub(super) fn batch_url(origin: &str, auth_user: usize) -> String {
    format!("{origin}/u/{auth_user}/_/BardChatUi/data/batchexecute")
}

#[derive(Debug, Clone)]
pub(super) struct SessionMetadata {
    pub access_token: Option<String>,
    pub build_label: Option<String>,
    pub session_id: Option<String>,
    pub language: String,
}

pub(super) fn build_http_client(config: &Config) -> Result<Client, GatewayError> {
    let jar = Arc::new(Jar::default());
    let google = "https://google.com";
    let psid_cookie = format!(
        "__Secure-1PSID={}; Domain=.google.com; Path=/; Secure",
        config.secure_1psid
    );
    jar.add(psid_cookie.as_str(), google);
    if let Some(psidts) = &config.secure_1psidts {
        let psidts_cookie =
            format!("__Secure-1PSIDTS={psidts}; Domain=.google.com; Path=/; Secure");
        jar.add(psidts_cookie.as_str(), google);
    }

    let mut builder = Client::builder()
        .emulation(Emulation::Chrome140)
        .cookie_provider(jar)
        .redirect(reqwest::redirect::Policy::limited(10))
        .timeout(config.provider_timeout);
    if let Some(proxy) = &config.proxy {
        builder = builder.proxy(Proxy::all(proxy).map_err(|error| {
            GatewayError::InvalidRequest(format!("invalid GEMINI_PROXY: {error}"))
        })?);
    } else {
        builder = builder.no_proxy();
    }
    Ok(builder.build()?)
}

pub(super) async fn fetch_session(
    http: &Client,
    config: &Config,
) -> Result<SessionMetadata, GatewayError> {
    let mut auth_cookie = format!("__Secure-1PSID={}", config.secure_1psid);
    if let Some(psidts) = &config.secure_1psidts {
        auth_cookie.push_str("; __Secure-1PSIDTS=");
        auth_cookie.push_str(psidts);
    }

    // Match the browser preflight used by the Python client. Besides
    // validating connectivity this lets the cookie jar receive Google's
    // normal session cookies before the Gemini application request.
    let preflight = if config.provider_origin == GEMINI_ORIGIN {
        "https://www.google.com"
    } else {
        &config.provider_origin
    };
    http.get(preflight).send().await?.error_for_status()?;
    let response = http
        .get(app_url(&config.provider_origin, config.auth_user))
        .header(header::ORIGIN, &config.provider_origin)
        .header(
            header::REFERER,
            app_url(&config.provider_origin, config.auth_user),
        )
        .header(header::COOKIE, auth_cookie)
        .send()
        .await?;
    let response_url = response.uri().clone();
    let response_status = response.status();
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err(GatewayError::Auth(
            "Gemini rejected the configured cookies".into(),
        ));
    }
    let response = response.error_for_status()?.text().await?;
    let access_token = capture(&response, r#""SNlM0e":\s*"(.*?)""#);
    let build_label = capture(&response, r#""cfb2h":\s*"(.*?)""#);
    let session_id = capture(&response, r#""FdrFJe":\s*"(.*?)""#);
    if access_token.is_none() && build_label.is_none() && session_id.is_none() {
        return Err(GatewayError::Auth(format!(
            "Gemini session metadata was not present in /app (status {response_status}, final URL {response_url}, {} bytes)",
            response.len(),
        )));
    }
    Ok(SessionMetadata {
        access_token,
        build_label,
        session_id,
        language: capture(&response, r#""TuX5cc":\s*"(.*?)""#).unwrap_or_else(|| "en".into()),
    })
}

fn capture(input: &str, pattern: &str) -> Option<String> {
    Regex::new(pattern)
        .ok()?
        .captures(input)?
        .get(1)
        .map(|value| value.as_str().to_owned())
}

#[allow(dead_code)]
pub(super) fn placeholder() {
    // Force-link protocol helpers so the module surface stays explicit.
    let _ = protocol::model_headers;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_urls_include_selected_google_profile() {
        assert_eq!(app_path(2), "/u/2/app");
        assert!(generate_url(GEMINI_ORIGIN, 1).contains("/u/1/_/BardChatUi/"));
        assert!(batch_url(GEMINI_ORIGIN, 7).contains("/u/7/_/BardChatUi/"));
    }
}
