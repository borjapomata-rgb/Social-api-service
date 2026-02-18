use axum::Router;
use crate::app::AppState;

mod health;
mod likes;

pub fn create_router(state: AppState) -> Router {
    Router::new()
        .merge(health::routes())
        .merge(likes::routes())
        .with_state(state)
}
