pub mod admin;
pub mod chat;
pub mod gemini_api;
pub mod generations;
pub mod health;
pub mod models;
pub mod responses;
pub mod tasks;
pub mod ui;

use std::{sync::Arc, time::Instant};

use axum::{
    Router,
    extract::{DefaultBodyLimit, MatchedPath, Request, State},
    http::{HeaderValue, Method, header},
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
};
use tower_http::{
    cors::{AllowOrigin, CorsLayer},
    services::ServeDir,
    trace::TraceLayer,
};

use crate::{
    config::Config,
    gemini::GeminiClient,
    observability::{metrics::Metrics, request_id::middleware as request_id_middleware},
    tasks::TaskQueue,
};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub gemini: Arc<GeminiClient>,
    pub tasks: Arc<TaskQueue>,
    pub metrics: Arc<Metrics>,
}

pub fn router(
    config: Arc<Config>,
    gemini: Arc<GeminiClient>,
    tasks: Arc<TaskQueue>,
    metrics: Arc<Metrics>,
) -> Router {
    let files_root = tasks.artifacts_root().to_path_buf();
    let state = AppState {
        config,
        gemini,
        tasks,
        metrics,
    };
    let mut app = Router::new()
        .route("/", get(ui::index))
        .route("/app.css", get(ui::styles))
        .route("/app.js", get(ui::script))
        .route("/admin/cookies/sync", post(admin::sync_cookies))
        .route(
            "/admin/accounts/{account_id}",
            axum::routing::patch(admin::configure_account).delete(admin::delete_account),
        )
        .route("/health", get(health::health))
        .route("/healthz", get(health::healthz))
        .route("/readyz", get(health::readyz))
        .route("/metrics", get(health::metrics))
        .route("/v1/models", get(models::models))
        .route("/v1/capabilities", get(models::capabilities))
        .route("/v1/accounts", get(admin::list_accounts))
        .route("/v1/responses", post(responses::create_response))
        .route("/v1beta/models", get(gemini_api::list_models))
        .route("/v1beta/models/{*action}", post(gemini_api::model_action))
        .route("/v1/generations", post(generations::create_generation))
        .route("/v1/tasks", get(tasks::list_tasks))
        .route(
            "/v1/tasks/{task_id}",
            get(tasks::get_task).delete(tasks::cancel_task),
        )
        .route(
            "/v1/images/generations",
            post(generations::image_generations),
        )
        .route("/v1/chat/completions", post(chat::chat_completions))
        .nest_service("/files", ServeDir::new(files_root))
        .layer(DefaultBodyLimit::max(state.config.max_request_bytes))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            record_metrics,
        ))
        .layer(middleware::from_fn(request_id_middleware))
        .layer(TraceLayer::new_for_http())
        .with_state(state.clone());

    if !state.config.cors_origins.is_empty() {
        let origins: Vec<HeaderValue> = state
            .config
            .cors_origins
            .iter()
            .filter_map(|origin| origin.parse().ok())
            .collect();
        app = app.layer(
            CorsLayer::new()
                .allow_origin(AllowOrigin::list(origins))
                .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::DELETE])
                .allow_headers([
                    header::AUTHORIZATION,
                    header::CONTENT_TYPE,
                    header::HeaderName::from_static("x-goog-api-key"),
                    header::HeaderName::from_static("x-gemini-account"),
                ]),
        );
    }
    app
}

async fn record_metrics(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let started = Instant::now();
    let method = request.method().clone();
    let path = request
        .extensions()
        .get::<MatchedPath>()
        .map(|value| value.as_str().to_owned())
        .unwrap_or_else(|| request.uri().path().to_owned());
    let response = next.run(request).await;
    let status = response.status().as_u16();
    state
        .metrics
        .record_request(method.as_str(), &path, status, started.elapsed());
    response
}
