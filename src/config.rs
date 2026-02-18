use std::path::PathBuf;

use clap::Parser;

/// Default port the server listens on.
const DEFAULT_PORT: u16 = 8080;

/// Default directory for extracted uploads.
const DEFAULT_UPLOAD_DIR: &str = "./data/uploads";

/// CLI arguments for actions-recon.
#[derive(Parser, Debug)]
#[command(name = "actions-recon", about = "GitHub Actions log analysis tool")]
pub struct Args {
    /// Port to listen on (overrides PORT env var)
    #[arg(long, default_value_t = DEFAULT_PORT)]
    pub port: u16,
}

/// Application configuration resolved from CLI args + environment.
#[derive(Debug, Clone)]
pub struct AppConfig {
    pub port: u16,
    pub upload_dir: PathBuf,
}

impl AppConfig {
    /// Build config from CLI args and environment variables.
    /// CLI flags take precedence over env vars; env vars over defaults.
    pub fn from_env_and_args(args: &Args) -> Self {
        // PORT: CLI arg wins if explicitly set (non-default), else check env.
        let port = if args.port != DEFAULT_PORT {
            args.port
        } else {
            std::env::var("PORT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(args.port)
        };

        let upload_dir = std::env::var("UPLOAD_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(DEFAULT_UPLOAD_DIR));

        Self { port, upload_dir }
    }
}
