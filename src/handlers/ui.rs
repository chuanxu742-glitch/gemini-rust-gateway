use axum::{
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};

const INDEX: &str = include_str!("../../web/index.html");
const STYLES: &str = include_str!("../../web/app.css");
const SCRIPT: &str = include_str!("../../web/app.js");

pub async fn index() -> Response {
    asset("text/html; charset=utf-8", INDEX, "no-cache")
}

pub async fn styles() -> Response {
    asset("text/css; charset=utf-8", STYLES, "public, max-age=3600")
}

pub async fn script() -> Response {
    asset(
        "text/javascript; charset=utf-8",
        SCRIPT,
        "public, max-age=3600",
    )
}

fn asset(content_type: &'static str, body: &'static str, cache_control: &'static str) -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, cache_control),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'self'; img-src 'self' data: blob:; style-src 'self'; script-src 'self'; connect-src 'self' http: https:",
            ),
        ],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_frontend_has_expected_controls() {
        assert!(INDEX.contains("id=\"chat-form\""));
        assert!(INDEX.contains("id=\"image-form\""));
        assert!(INDEX.contains("id=\"task-form\""));
        assert!(STYLES.contains(":root"));
        assert!(SCRIPT.contains("/v1/chat/completions"));
        assert!(SCRIPT.contains("/v1/generations"));
    }
}
