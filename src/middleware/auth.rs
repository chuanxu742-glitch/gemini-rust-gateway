use axum::http::{HeaderMap, header};

use crate::{config::Config, error::GatewayError, error::Result};

pub fn authorize(config: &Config, headers: &HeaderMap) -> Result<()> {
    let expected = format!("Bearer {}", config.api_key);
    let actual = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if !constant_time_eq(actual.as_bytes(), expected.as_bytes()) {
        return Err(GatewayError::Auth("invalid API key".into()));
    }
    Ok(())
}

pub fn authorize_admin(config: &Config, headers: &HeaderMap) -> Result<()> {
    authorize_secret(&config.admin_token, headers, "invalid admin token")
}

pub fn requested_account(headers: &HeaderMap) -> Result<Option<String>> {
    let Some(value) = headers.get("x-gemini-account") else {
        return Ok(None);
    };
    let value = value
        .to_str()
        .map_err(|_| GatewayError::InvalidRequest("invalid X-Gemini-Account header".into()))?
        .trim();
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(GatewayError::InvalidRequest(
            "X-Gemini-Account must contain 1..64 letters, digits, '.', '_' or '-'".into(),
        ));
    }
    Ok(Some(value.to_owned()))
}

fn authorize_secret(expected: &str, headers: &HeaderMap, message: &str) -> Result<()> {
    let actual = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or("");
    if !constant_time_eq(actual.as_bytes(), expected.as_bytes()) {
        return Err(GatewayError::Auth(message.into()));
    }
    Ok(())
}

pub fn authorize_gemini(
    config: &Config,
    headers: &HeaderMap,
    query_key: Option<&str>,
) -> Result<()> {
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let google_key = headers
        .get("x-goog-api-key")
        .and_then(|value| value.to_str().ok());
    let actual = bearer.or(google_key).or(query_key).unwrap_or("");
    if !constant_time_eq(actual.as_bytes(), config.api_key.as_bytes()) {
        return Err(GatewayError::Auth("invalid API key".into()));
    }
    Ok(())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let max = left.len().max(right.len());
    let mut difference = left.len() ^ right.len();
    for index in 0..max {
        let a = left.get(index).copied().unwrap_or(0);
        let b = right.get(index).copied().unwrap_or(0);
        difference |= usize::from(a ^ b);
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_comparison_handles_lengths() {
        assert!(constant_time_eq(b"same", b"same"));
        assert!(!constant_time_eq(b"same", b"different"));
    }
}
