use std::sync::Arc;

use axum::{extract::State, routing::get, Json, Router};
use tower_http::cors::CorsLayer;
use tracing::info;

use crate::api::core::{DiffRequest, DiffResponse, DiffService};
use crate::options::{DiffOptions, DisplayOptions};

pub fn run_http() {
    let rt = tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime");
    rt.block_on(async {
        let service = Arc::new(DiffService::new());
        start(service).await;
    });
}

pub async fn start(service: Arc<DiffService>) {
    let app = create_router(service);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
        .await
        .expect("Failed to bind to address");
    info!("HTTP server listening on 0.0.0.0:3000");
    axum::serve(listener, app).await.expect("Server error");
}

fn create_router(service: Arc<DiffService>) -> Router {
    Router::new()
        .route("/api/v1/diff", axum::routing::post(diff_handler))
        .route("/health", get(health_handler))
        .layer(CorsLayer::permissive())
        .with_state(service)
}

async fn diff_handler(
    State(service): State<Arc<DiffService>>,
    Json(req): Json<DiffRequest>,
) -> Json<DiffResponse> {
    let diff_options = DiffOptions::default();
    let display_options = DisplayOptions::default();

    let language_override = req
        .language_override
        .and_then(|lang| crate::parse::guess_language::language_override_from_name(&lang))
        .and_then(|lo| match lo {
            crate::parse::guess_language::LanguageOverride::Language(lang) => Some(lang),
            crate::parse::guess_language::LanguageOverride::PlainText => None,
        });

    let diff_result = service.diff(
        &req.lhs_content,
        &req.rhs_content,
        req.display_path.as_deref().unwrap_or("unknown"),
        language_override,
        &[],
        &diff_options,
        &display_options,
    );

    let response = DiffResponse {
        display_path: diff_result.display_path,
        file_format: format!("{}", diff_result.file_format),
        has_syntactic_changes: diff_result.has_syntactic_changes,
        has_byte_changes: diff_result.has_byte_changes.is_some(),
        lhs_byte_len: diff_result.has_byte_changes.map(|(lhs, _)| lhs),
        rhs_byte_len: diff_result.has_byte_changes.map(|(_, rhs)| rhs),
        hunks: vec![],
    };

    Json(response)
}

async fn health_handler() -> &'static str {
    "OK"
}
