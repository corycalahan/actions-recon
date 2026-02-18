use std::sync::Arc;

use askama::Template;
use axum::extract::State;
use axum::response::{Html, Redirect};

use crate::config::AppConfig;

/// Template context for the home page.
#[derive(Template)]
#[template(path = "home.html")]
struct HomeTemplate {
    analyses: Vec<String>,
    upload_path: String,
}

/// `GET /` — home page: upload form + list of previous analyses + delete-all button.
pub async fn index(State(config): State<Arc<AppConfig>>) -> Result<Html<String>, String> {
    let analyses = list_analyses(&config.upload_dir);
    let upload_path = config
        .upload_dir
        .canonicalize()
        .unwrap_or_else(|_| config.upload_dir.clone())
        .display()
        .to_string();
    let template = HomeTemplate {
        analyses,
        upload_path,
    };
    template
        .render()
        .map(Html)
        .map_err(|e| format!("Template error: {e}"))
}

/// `POST /delete-all` — remove all extracted files and redirect to home.
pub async fn delete_all(State(config): State<Arc<AppConfig>>) -> Result<Redirect, String> {
    let upload_dir = &config.upload_dir;

    if upload_dir.is_dir() {
        for entry in std::fs::read_dir(upload_dir)
            .map_err(|e| format!("Failed to read upload directory: {e}"))?
        {
            let entry = entry.map_err(|e| format!("Failed to read entry: {e}"))?;
            let path = entry.path();
            if path.is_dir() {
                std::fs::remove_dir_all(&path)
                    .map_err(|e| format!("Failed to remove {}: {e}", path.display()))?;
            } else {
                std::fs::remove_file(&path)
                    .map_err(|e| format!("Failed to remove {}: {e}", path.display()))?;
            }
        }
        tracing::info!(dir = %upload_dir.display(), "Deleted all extracted files");
    }

    Ok(Redirect::to("/"))
}

/// Scan the upload directory for existing analysis folders.
fn list_analyses(upload_dir: &std::path::Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(upload_dir) else {
        return Vec::new();
    };

    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();

    names.sort();
    names
}
