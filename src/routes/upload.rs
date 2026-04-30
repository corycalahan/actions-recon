use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::State;
use axum::response::Redirect;

use crate::config::AppConfig;
use crate::extract::zip_extract;

/// Sanitize an analysis ID derived from an uploaded filename.
///
/// Only allows alphanumeric, hyphens, and underscores. Returns the same
/// `BadRequest` style error used by the upload handler so all callers share
/// one rejection message.
pub(crate) fn sanitize_analysis_id(filename: &str, analysis_id: &str) -> Result<(), UploadError> {
    if !analysis_id
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Err(UploadError::BadRequest(format!(
            "Invalid filename '{filename}': only alphanumeric characters, hyphens, and underscores are allowed."
        )));
    }
    Ok(())
}

/// Derive the analysis ID from a filename's stem.
pub(crate) fn analysis_id_from_filename(filename: &str) -> String {
    PathBuf::from(filename)
        .file_stem()
        .and_then(|s| s.to_str().map(String::from))
        .unwrap_or_else(|| "unknown".to_string())
}

/// Extract a single uploaded ZIP into `<upload_dir>/<analysis_id>/`.
///
/// Runs the extraction inside `tokio::task::spawn_blocking` so it does not
/// block the runtime. Auto-extracts nested ZIPs (e.g. runner-diagnostic-logs).
pub(crate) async fn extract_uploaded_zip(
    upload_dir: &std::path::Path,
    analysis_id: &str,
    data: Vec<u8>,
) -> Result<zip_extract::ExtractResult, UploadError> {
    let output_dir = upload_dir.join(analysis_id);
    let bytes = data.len();
    tracing::info!(analysis_id = %analysis_id, bytes, "Processing upload");

    let result = tokio::task::spawn_blocking(move || {
        let result = zip_extract::extract_zip(&data, &output_dir)?;
        zip_extract::extract_nested_zips(&output_dir);
        Ok::<_, zip_extract::ExtractError>(result)
    })
    .await
    .map_err(|e| UploadError::Internal(format!("Task join error: {e}")))?
    .map_err(UploadError::Extraction)?;

    tracing::info!(
        analysis_id = %analysis_id,
        files = result.file_count,
        total_bytes = result.total_bytes,
        "Upload extracted successfully"
    );

    Ok(result)
}

/// `POST /upload` — accept a ZIP file upload, extract it, redirect to the analysis page.
pub async fn upload(
    State(config): State<Arc<AppConfig>>,
    mut multipart: axum_extra::extract::Multipart,
) -> Result<Redirect, UploadError> {
    // Read the first file field named "zipfile"
    let mut zip_bytes: Option<(String, Vec<u8>)> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| UploadError::BadRequest(format!("Failed to read multipart field: {e}")))?
    {
        if field.name() == Some("zipfile") {
            let filename = field.file_name().unwrap_or("upload.zip").to_string();
            let data = field
                .bytes()
                .await
                .map_err(|e| UploadError::BadRequest(format!("Failed to read file data: {e}")))?;
            zip_bytes = Some((filename, data.to_vec()));
            break;
        }
    }

    let (filename, data) = zip_bytes.ok_or_else(|| {
        UploadError::BadRequest("No file field named 'zipfile' found in the upload.".into())
    })?;

    let analysis_id = analysis_id_from_filename(&filename);
    sanitize_analysis_id(&filename, &analysis_id)?;
    extract_uploaded_zip(&config.upload_dir, &analysis_id, data).await?;

    Ok(Redirect::to(&format!("/analysis/{analysis_id}")))
}

/// Errors from the upload handler — rendered as user-facing error pages.
#[derive(Debug, thiserror::Error)]
pub enum UploadError {
    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("Extraction failed: {0}")]
    Extraction(#[from] zip_extract::ExtractError),

    #[error("Internal error: {0}")]
    Internal(String),
}

impl axum::response::IntoResponse for UploadError {
    fn into_response(self) -> axum::response::Response {
        use askama::Template;
        use axum::http::StatusCode;
        use axum::response::Html;

        let status = match &self {
            UploadError::BadRequest(_) => StatusCode::BAD_REQUEST,
            UploadError::Extraction(_) => StatusCode::UNPROCESSABLE_ENTITY,
            UploadError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };

        let message = self.to_string();
        tracing::error!(error = %message, "Upload failed");

        #[derive(Template)]
        #[template(path = "error.html")]
        struct ErrorTemplate {
            title: String,
            message: String,
        }

        let template = ErrorTemplate {
            title: format!("{status}"),
            message,
        };

        match template.render() {
            Ok(html) => (status, Html(html)).into_response(),
            Err(_) => (status, "An error occurred").into_response(),
        }
    }
}
