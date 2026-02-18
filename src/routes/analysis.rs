use std::sync::Arc;

use askama::Template;
use axum::extract::{Path, State};
use axum::response::Html;

use crate::config::AppConfig;
use crate::models::log_parser;
use crate::models::tips;

/// A file or directory entry in the analysis overview.
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct FileEntry {
    /// Display name.
    name: String,
    /// Relative path from the analysis root (URL-encoded for links).
    path: String,
    /// Whether this is a directory.
    is_dir: bool,
    /// File size in bytes (0 for directories).
    size: u64,
    /// Human-readable file size.
    size_display: String,
    /// Category for grouping in the UI.
    category: FileCategory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FileCategory {
    /// Top-level job log (e.g. `0_hello-world.txt`).
    JobLog,
    /// Step-level log inside a job directory.
    StepLog,
    /// System log (`system.txt`).
    SystemLog,
    /// Runner diagnostic log.
    RunnerDiagnostic,
    /// Other file.
    Other,
}

impl FileCategory {
    fn label(&self) -> &'static str {
        match self {
            FileCategory::JobLog => "Job Log",
            FileCategory::StepLog => "Step Log",
            FileCategory::SystemLog => "System Log",
            FileCategory::RunnerDiagnostic => "Runner Diagnostics",
            FileCategory::Other => "Other",
        }
    }
}

/// A group of files for display in the analysis overview.
struct FileGroup {
    label: String,
    entries: Vec<FileEntry>,
}

/// Template for the analysis overview page.
#[derive(Template)]
#[template(path = "analysis.html")]
struct AnalysisTemplate {
    analysis_id: String,
    groups: Vec<FileGroup>,
}

/// `GET /analysis/:id` — overview of all log files in this analysis.
pub async fn overview(
    State(config): State<Arc<AppConfig>>,
    Path(id): Path<String>,
) -> Result<Html<String>, AnalysisError> {
    // Sanitize ID
    if !id
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Err(AnalysisError::NotFound(format!(
            "Invalid analysis ID: {id}"
        )));
    }

    let analysis_dir = config.upload_dir.join(&id);
    if !analysis_dir.is_dir() {
        return Err(AnalysisError::NotFound(format!(
            "Analysis '{id}' not found. Upload a ZIP first."
        )));
    }

    let entries = scan_analysis_dir(&analysis_dir, &analysis_dir);
    let groups = group_entries(entries);

    let template = AnalysisTemplate {
        analysis_id: id,
        groups,
    };

    template
        .render()
        .map(Html)
        .map_err(|e| AnalysisError::Internal(format!("Template error: {e}")))
}

/// A tip evaluation result adapted for template rendering.
struct TipDisplay {
    id: String,
    name: String,
    emoji: String,
    docs: Option<String>,
    description: Option<String>,
    detail: String,
    has_marked_lines: bool,
    marked_lines: Vec<usize>,
}

/// A parsed action reference adapted for template rendering.
struct ActionSummaryDisplay {
    owner_repo: String,
    path: String,
    reference: String,
    first_line: usize,
    github_url: String,
}

impl TipDisplay {
    fn from_result(r: &tips::TipResult) -> Self {
        Self {
            id: r.tip.id.clone(),
            name: r.tip.name.clone(),
            emoji: r.tip.emoji.clone(),
            docs: r.tip.docs.clone(),
            description: r.tip.description.clone(),
            detail: r.detail.clone(),
            has_marked_lines: !r.marked_lines.is_empty(),
            marked_lines: r.marked_lines.clone(),
        }
    }
}

/// Template for viewing a single log file's timeline.
#[derive(Template)]
#[template(path = "logfile.html")]
struct LogfileTemplate {
    analysis_id: String,
    logfile_name: String,
    entries: Vec<log_parser::LogEntry>,
    total_lines: usize,
    debug_count: usize,
    warning_count: usize,
    error_count: usize,
    notice_count: usize,
    command_count: usize,
    group_count: usize,
    info_count: usize,
    action_refs: Vec<ActionSummaryDisplay>,
    tips: Vec<TipDisplay>,
}

/// `GET /analysis/:id/*logfile` — timeline view of a specific log file.
pub async fn logfile(
    State(config): State<Arc<AppConfig>>,
    Path((id, logfile_path)): Path<(String, String)>,
) -> Result<Html<String>, AnalysisError> {
    // Sanitize ID
    if !id
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Err(AnalysisError::NotFound(format!(
            "Invalid analysis ID: {id}"
        )));
    }

    let analysis_dir = config.upload_dir.join(&id);
    if !analysis_dir.is_dir() {
        return Err(AnalysisError::NotFound(format!(
            "Analysis '{id}' not found."
        )));
    }

    // Decode the logfile path and resolve it
    let decoded_path = urlencoding::decode(&logfile_path)
        .map_err(|e| AnalysisError::NotFound(format!("Invalid path encoding: {e}")))?;
    let file_path = analysis_dir.join(decoded_path.as_ref());

    // Security: ensure the resolved path is inside the analysis directory
    let canonical_analysis = analysis_dir
        .canonicalize()
        .map_err(|e| AnalysisError::Internal(format!("Path error: {e}")))?;
    let canonical_file = file_path
        .canonicalize()
        .map_err(|_| AnalysisError::NotFound(format!("File not found: {decoded_path}")))?;

    if !canonical_file.starts_with(&canonical_analysis) {
        return Err(AnalysisError::NotFound(
            "Path traversal detected.".to_string(),
        ));
    }

    if !canonical_file.is_file() {
        return Err(AnalysisError::NotFound(format!(
            "File not found: {decoded_path}"
        )));
    }

    let content = std::fs::read_to_string(&canonical_file)
        .map_err(|e| AnalysisError::Internal(format!("Failed to read file: {e}")))?;

    // Determine if this is a runner diagnostic log or a workflow log
    let is_runner_log = decoded_path.contains("runner-diagnostic-logs");
    let entries = if is_runner_log {
        log_parser::parse_runner_log(&content)
    } else {
        log_parser::parse_workflow_log(&content)
    };

    let total_lines = entries.len();
    let debug_count = entries
        .iter()
        .filter(|e| e.level == log_parser::LogLevel::Debug)
        .count();
    let warning_count = entries
        .iter()
        .filter(|e| e.level == log_parser::LogLevel::Warning)
        .count();
    let error_count = entries
        .iter()
        .filter(|e| e.level == log_parser::LogLevel::Error)
        .count();
    let notice_count = entries
        .iter()
        .filter(|e| e.level == log_parser::LogLevel::Notice)
        .count();
    let command_count = entries
        .iter()
        .filter(|e| e.level == log_parser::LogLevel::Command)
        .count();
    let group_count = entries
        .iter()
        .filter(|e| e.level == log_parser::LogLevel::Group)
        .count();
    let info_count = entries
        .iter()
        .filter(|e| e.level == log_parser::LogLevel::Info)
        .count();

    let logfile_name = decoded_path.to_string();
    let log_kind = if is_runner_log {
        tips::LogKind::Runner
    } else {
        tips::LogKind::Workflow
    };

    let action_refs: Vec<ActionSummaryDisplay> = log_parser::extract_action_references(&entries)
        .into_iter()
        .map(|action| {
            let github_url = action.github_url();
            ActionSummaryDisplay {
                owner_repo: action.owner_repo(),
                path: action.path.unwrap_or_else(|| "-".to_string()),
                reference: action.reference,
                first_line: action.first_line,
                github_url,
            }
        })
        .collect();

    // Load and evaluate tips
    let all_tips = tips::load_tips(std::path::Path::new("tips"));
    let tip_results = tips::evaluate_tips_for_log(&all_tips, &entries, log_kind);
    let tips_display: Vec<TipDisplay> = tip_results
        .iter()
        .filter(|r| r.triggered)
        .map(TipDisplay::from_result)
        .collect();

    let template = LogfileTemplate {
        analysis_id: id,
        logfile_name,
        entries,
        total_lines,
        debug_count,
        warning_count,
        error_count,
        notice_count,
        command_count,
        group_count,
        info_count,
        action_refs,
        tips: tips_display,
    };

    template
        .render()
        .map(Html)
        .map_err(|e| AnalysisError::Internal(format!("Template error: {e}")))
}

/// Format a byte count as a human-readable string.
fn format_file_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// Recursively scan the analysis directory and build a flat list of file entries.
fn scan_analysis_dir(dir: &std::path::Path, root: &std::path::Path) -> Vec<FileEntry> {
    let mut entries = Vec::new();

    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return entries;
    };

    for item in read_dir.flatten() {
        let path = item.path();
        let rel_path = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();

        if path.is_dir() {
            // Recurse into subdirectories
            let sub_entries = scan_analysis_dir(&path, root);
            entries.extend(sub_entries);
        } else {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let size = path.metadata().map(|m| m.len()).unwrap_or(0);
            let category = categorize_file(&rel_path, &name);

            entries.push(FileEntry {
                name,
                path: rel_path,
                is_dir: false,
                size_display: format_file_size(size),
                size,
                category,
            });
        }
    }

    entries
}

/// Determine the category of a file based on its path and name.
fn categorize_file(rel_path: &str, name: &str) -> FileCategory {
    // std::path::MAIN_SEPARATOR is '/' on macOS/Linux and '\' on Windows.
    // Using it here ensures step logs (files inside subdirectories) are correctly
    // distinguished from top-level job logs regardless of the host OS.
    let sep = std::path::MAIN_SEPARATOR;

    if rel_path.contains("runner-diagnostic-logs") {
        FileCategory::RunnerDiagnostic
    } else if name == "system.txt" {
        FileCategory::SystemLog
    } else if !rel_path.contains(sep) && name.ends_with(".txt") {
        // No separator → file is at the top level of the analysis dir → job log
        // e.g. "0_hello-world.txt" on macOS/Linux, same on Windows
        FileCategory::JobLog
    } else if rel_path.contains(sep) && name.ends_with(".txt") {
        // Has a separator → file is inside a subdirectory → step log
        // e.g. "hello-world/1_Set up job.txt" (macOS/Linux: '/')
        //      "hello-world\1_Set up job.txt" (Windows: '\')
        FileCategory::StepLog
    } else {
        FileCategory::Other
    }
}

/// Group file entries by category for display.
fn group_entries(mut entries: Vec<FileEntry>) -> Vec<FileGroup> {
    // Sort by path for consistent ordering
    entries.sort_by(|a, b| a.path.cmp(&b.path));

    let categories = [
        FileCategory::JobLog,
        FileCategory::StepLog,
        FileCategory::SystemLog,
        FileCategory::RunnerDiagnostic,
        FileCategory::Other,
    ];

    let mut groups = Vec::new();
    for cat in &categories {
        let group_entries: Vec<FileEntry> = entries
            .iter()
            .filter(|e| &e.category == cat)
            .cloned()
            .collect();

        if !group_entries.is_empty() {
            groups.push(FileGroup {
                label: cat.label().to_string(),
                entries: group_entries,
            });
        }
    }

    groups
}

/// Errors from analysis routes.
#[derive(Debug, thiserror::Error)]
pub enum AnalysisError {
    #[error("{0}")]
    NotFound(String),

    #[error("{0}")]
    Internal(String),
}

impl axum::response::IntoResponse for AnalysisError {
    fn into_response(self) -> axum::response::Response {
        use askama::Template;
        use axum::http::StatusCode;
        use axum::response::Html;

        let status = match &self {
            AnalysisError::NotFound(_) => StatusCode::NOT_FOUND,
            AnalysisError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };

        let message = self.to_string();

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
