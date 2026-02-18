mod config;
mod extract;
mod models;
mod routes;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use clap::Parser;
use tower_http::services::ServeDir;
use tracing::info;

use config::{AppConfig, Args};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load .env (ignore if missing)
    let _ = dotenvy::dotenv();

    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Parse CLI args and build config
    let args = Args::parse();
    let config = AppConfig::from_env_and_args(&args);

    // Ensure upload directory exists
    std::fs::create_dir_all(&config.upload_dir)?;

    info!(port = config.port, upload_dir = %config.upload_dir.display(), "Starting actions-recon");

    let shared_config = Arc::new(config.clone());

    let app = Router::new()
        .route("/", axum::routing::get(routes::home::index))
        .route("/upload", axum::routing::post(routes::upload::upload))
        .route("/delete-all", axum::routing::post(routes::home::delete_all))
        .route("/settings", axum::routing::get(routes::settings::index))
        .route(
            "/settings/tips",
            axum::routing::post(routes::settings::save_tip),
        )
        .route(
            "/settings/tips/delete",
            axum::routing::post(routes::settings::delete_tip),
        )
        .route(
            "/analysis/{id}",
            axum::routing::get(routes::analysis::overview),
        )
        .route(
            "/analysis/{id}/{*logfile}",
            axum::routing::get(routes::analysis::logfile),
        )
        .nest_service("/static", ServeDir::new("static"))
        .with_state(shared_config);

    let addr = SocketAddr::from(([127, 0, 0, 1], config.port));
    info!(%addr, "Listening");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
