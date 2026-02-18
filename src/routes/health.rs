use axum::{routing::get, Router};
use axum::response::IntoResponse;
use crate::app::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
}

async fn live() -> impl IntoResponse {
    "OK"
}

async fn ready() -> impl IntoResponse {
    "READY"
}
