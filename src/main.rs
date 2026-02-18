use std::net::SocketAddr;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod config;
mod app;
mod routes;
mod db;
mod cache;

use config::Config;
use app::AppState;

#[tokio::main]
async fn main() {
    // Initialize logging
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Load config
    let config = Config::from_env();

    // Initialize database pool
    let db = db::postgres::init_pool(&config.database_url).await;

    // Initialize redis client
    let redis = cache::redis::init_client(&config.redis_url);

    // Create shared app state
    let state = AppState { db, redis };

    // Build router
    let app = routes::create_router(state);

    // Start server
    let addr = SocketAddr::from(([0, 0, 0, 0], config.http_port));
    tracing::info!("Server running on {}", addr);

    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}
