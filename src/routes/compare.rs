//! Compare-view routes — pick two analyses (existing or newly uploaded)
//! and render a structured diff between them.

use std::sync::Arc;

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::response::{Html, Redirect};
use serde::Deserialize;

use crate::config::AppConfig;
use crate::models::compare::{self, RunnerImage};
use crate::models::diff::{self, ComparisonReport, JobStepGroup, PairedActionRef, PairedStep};
use crate::models::log_parser::ActionReference;
use crate::routes::home::list_analyses;
use crate::routes::upload::{
    UploadError, analysis_id_from_filename, extract_uploaded_zip, sanitize_analysis_id,
};

// ── Form / query ─────────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
pub struct CompareQuery {
    #[serde(default)]
    pub fuzzy: Option<String>,
}

impl CompareQuery {
    fn is_fuzzy(&self) -> bool {
        matches!(
            self.fuzzy.as_deref(),
            Some("1") | Some("true") | Some("on") | Some("yes")
        )
    }
}

// ── Template view models ─────────────────────────────────────────────────────

/// Display row for a paired action reference.
#[allow(dead_code)]
struct ActionPairRow {
    owner_repo_path: String,
    github_url: String,
    a_reference: String,
    b_reference: String,
    differs: bool,
}

/// Display row for an action reference present on only one side.
#[allow(dead_code)]
struct ActionOnlyRow {
    owner_repo_path: String,
    github_url: String,
    reference: String,
}

/// Display row for a paired step.
#[allow(dead_code)]
struct StepRow {
    label: String,
    a_duration_display: String,
    b_duration_display: String,
    delta_display: String,
    delta_class: &'static str,
}

/// Display group of paired steps (one job folder).
#[allow(dead_code)]
struct StepGroupView {
    job_folder: String,
    steps: Vec<StepRow>,
}

/// Unmatched step row.
#[allow(dead_code)]
struct UnmatchedStepRow {
    job_folder: String,
    step_file: String,
    duration_display: String,
}

#[derive(Template)]
#[template(path = "compare.html")]
struct CompareFormTemplate {
    analyses: Vec<String>,
}

#[derive(Template)]
#[template(path = "compare_result.html")]
#[allow(dead_code)]
struct CompareResultTemplate {
    a_id: String,
    b_id: String,
    fuzzy: bool,
    // Runner type (classified)
    runner_type_a: String,
    runner_type_b: String,
    runner_type_differs: bool,
    // Runner identity (always render)
    runner_version_a: String,
    runner_version_b: String,
    runner_version_differs: bool,
    requested_labels_a: String,
    requested_labels_b: String,
    requested_labels_differs: bool,
    // Self-hosted identity (only render when at least one side is self-hosted)
    self_hosted_present: bool,
    self_hosted_runner_name_a: String,
    self_hosted_runner_name_b: String,
    self_hosted_runner_group_a: String,
    self_hosted_runner_group_b: String,
    self_hosted_machine_name_a: String,
    self_hosted_machine_name_b: String,
    self_hosted_identity_differs: bool,
    // Runner image (hosted-only signal; rendered when at least one side reports it)
    runner_image_present: bool,
    runner_image_a_image: String,
    runner_image_a_version: String,
    runner_image_b_image: String,
    runner_image_b_version: String,
    runner_image_differs: bool,
    runner_image_a_reported: bool,
    runner_image_b_reported: bool,
    // Azure region
    azure_region_a: String,
    azure_region_b: String,
    azure_region_differs: bool,
    azure_region_present: bool,
    // Diagnostic logs
    diagnostic_logs_a: bool,
    diagnostic_logs_b: bool,
    diagnostic_logs_differs: bool,
    // Action refs
    action_pairs: Vec<ActionPairRow>,
    action_only_in_a: Vec<ActionOnlyRow>,
    action_only_in_b: Vec<ActionOnlyRow>,
    // Steps
    step_groups: Vec<StepGroupView>,
    steps_only_in_a: Vec<UnmatchedStepRow>,
    steps_only_in_b: Vec<UnmatchedStepRow>,
}

// ── Errors ───────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum CompareError {
    #[error("{0}")]
    NotFound(String),

    #[error("{0}")]
    BadRequest(String),

    #[error("{0}")]
    Internal(String),

    #[error(transparent)]
    Upload(#[from] UploadError),
}

impl axum::response::IntoResponse for CompareError {
    fn into_response(self) -> axum::response::Response {
        use axum::http::StatusCode;

        // UploadError already implements IntoResponse — delegate to it.
        if let CompareError::Upload(e) = self {
            return e.into_response();
        }

        let status = match &self {
            CompareError::NotFound(_) => StatusCode::NOT_FOUND,
            CompareError::BadRequest(_) => StatusCode::BAD_REQUEST,
            CompareError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            CompareError::Upload(_) => unreachable!(),
        };

        let message = self.to_string();
        tracing::error!(error = %message, "Compare error");

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

// ── Handlers ─────────────────────────────────────────────────────────────────

/// `GET /compare` — render the form (dropdowns + dual-upload).
pub async fn compare_form(
    State(config): State<Arc<AppConfig>>,
) -> Result<Html<String>, CompareError> {
    let analyses = list_analyses(&config.upload_dir);
    let template = CompareFormTemplate { analyses };
    template
        .render()
        .map(Html)
        .map_err(|e| CompareError::Internal(format!("Template error: {e}")))
}

/// Form payload for `POST /compare/upload`.
struct ComparePairInputs {
    a_id: String,
    b_id: String,
}

/// `POST /compare/upload` — accept any combination of existing analysis IDs
/// and freshly-uploaded ZIPs (one per side), extract uploads, then redirect
/// to `/compare/{a}/{b}`.
pub async fn compare_upload(
    State(config): State<Arc<AppConfig>>,
    mut multipart: axum_extra::extract::Multipart,
) -> Result<Redirect, CompareError> {
    let mut existing_a: Option<String> = None;
    let mut existing_b: Option<String> = None;
    let mut zip_a: Option<(String, Vec<u8>)> = None;
    let mut zip_b: Option<(String, Vec<u8>)> = None;
    let mut fuzzy = false;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| CompareError::BadRequest(format!("Failed to read multipart field: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "existing_a" => {
                let v = field.text().await.unwrap_or_default();
                if !v.is_empty() {
                    existing_a = Some(v);
                }
            }
            "existing_b" => {
                let v = field.text().await.unwrap_or_default();
                if !v.is_empty() {
                    existing_b = Some(v);
                }
            }
            "zipfile_a" => {
                let filename = field.file_name().unwrap_or("upload.zip").to_string();
                let data = field.bytes().await.map_err(|e| {
                    CompareError::BadRequest(format!("Failed to read upload A: {e}"))
                })?;
                if !data.is_empty() {
                    zip_a = Some((filename, data.to_vec()));
                }
            }
            "zipfile_b" => {
                let filename = field.file_name().unwrap_or("upload.zip").to_string();
                let data = field.bytes().await.map_err(|e| {
                    CompareError::BadRequest(format!("Failed to read upload B: {e}"))
                })?;
                if !data.is_empty() {
                    zip_b = Some((filename, data.to_vec()));
                }
            }
            "fuzzy" => {
                let v = field.text().await.unwrap_or_default();
                if matches!(v.as_str(), "1" | "true" | "on" | "yes") {
                    fuzzy = true;
                }
            }
            _ => {
                // Ignore unknown fields.
                let _ = field.bytes().await;
            }
        }
    }

    let inputs = resolve_pair_inputs(&config, existing_a, existing_b, zip_a, zip_b).await?;

    let suffix = if fuzzy { "?fuzzy=1" } else { "" };
    Ok(Redirect::to(&format!(
        "/compare/{}/{}{}",
        inputs.a_id, inputs.b_id, suffix
    )))
}

async fn resolve_pair_inputs(
    config: &AppConfig,
    existing_a: Option<String>,
    existing_b: Option<String>,
    zip_a: Option<(String, Vec<u8>)>,
    zip_b: Option<(String, Vec<u8>)>,
) -> Result<ComparePairInputs, CompareError> {
    let a_id = resolve_side(config, "A", existing_a, zip_a).await?;
    let b_id = resolve_side(config, "B", existing_b, zip_b).await?;

    if a_id == b_id {
        return Err(CompareError::BadRequest(
            "Pick two different analyses to compare.".to_string(),
        ));
    }
    Ok(ComparePairInputs { a_id, b_id })
}

async fn resolve_side(
    config: &AppConfig,
    label: &str,
    existing: Option<String>,
    zip: Option<(String, Vec<u8>)>,
) -> Result<String, CompareError> {
    match (existing, zip) {
        (Some(_), Some(_)) => Err(CompareError::BadRequest(format!(
            "Side {label}: pick an existing analysis OR upload a ZIP, not both."
        ))),
        (Some(id), None) => {
            // Validate the existing ID and that the directory exists.
            if !is_valid_id(&id) {
                return Err(CompareError::BadRequest(format!(
                    "Side {label}: invalid analysis ID '{id}'."
                )));
            }
            if !config.upload_dir.join(&id).is_dir() {
                return Err(CompareError::NotFound(format!(
                    "Side {label}: analysis '{id}' not found."
                )));
            }
            Ok(id)
        }
        (None, Some((filename, data))) => {
            let id = analysis_id_from_filename(&filename);
            sanitize_analysis_id(&filename, &id).map_err(CompareError::Upload)?;
            extract_uploaded_zip(&config.upload_dir, &id, data).await?;
            Ok(id)
        }
        (None, None) => Err(CompareError::BadRequest(format!(
            "Side {label}: choose an existing analysis or upload a ZIP."
        ))),
    }
}

/// `GET /compare/{a}/{b}` — render the side-by-side comparison.
pub async fn compare_view(
    State(config): State<Arc<AppConfig>>,
    Path((a_id, b_id)): Path<(String, String)>,
    Query(query): Query<CompareQuery>,
) -> Result<Html<String>, CompareError> {
    if !is_valid_id(&a_id) || !is_valid_id(&b_id) {
        return Err(CompareError::NotFound(
            "Invalid analysis ID in compare URL.".to_string(),
        ));
    }
    let a_dir = config.upload_dir.join(&a_id);
    let b_dir = config.upload_dir.join(&b_id);
    if !a_dir.is_dir() {
        return Err(CompareError::NotFound(format!(
            "Analysis '{a_id}' not found."
        )));
    }
    if !b_dir.is_dir() {
        return Err(CompareError::NotFound(format!(
            "Analysis '{b_id}' not found."
        )));
    }

    let fuzzy = query.is_fuzzy();

    let (a_id_for_task, b_id_for_task) = (a_id.clone(), b_id.clone());
    let (summary_a, summary_b) = tokio::task::spawn_blocking(move || {
        let a = compare::summarize_archive(&a_id_for_task, &a_dir);
        let b = compare::summarize_archive(&b_id_for_task, &b_dir);
        (a, b)
    })
    .await
    .map_err(|e| CompareError::Internal(format!("Task join error: {e}")))?;

    let report = diff::compare(&summary_a, &summary_b, fuzzy);
    let template = build_result_template(&report);

    template
        .render()
        .map(Html)
        .map_err(|e| CompareError::Internal(format!("Template error: {e}")))
}

fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
}

fn build_result_template(report: &ComparisonReport) -> CompareResultTemplate {
    let runner_image_a_reported = report.runner_image.a.is_some();
    let runner_image_b_reported = report.runner_image.b.is_some();
    let runner_image_present = runner_image_a_reported || runner_image_b_reported;

    let runner_image_a = report.runner_image.a.clone().unwrap_or(RunnerImage {
        image: "—".into(),
        version: "—".into(),
    });
    let runner_image_b = report.runner_image.b.clone().unwrap_or(RunnerImage {
        image: "—".into(),
        version: "—".into(),
    });

    let azure_region_present = report.azure_region.a.is_some() || report.azure_region.b.is_some();

    // Self-hosted identity fields render when at least one side is self-hosted.
    let self_hosted_present =
        report.self_hosted_identity.a.is_some() || report.self_hosted_identity.b.is_some();
    let id_a = report.self_hosted_identity.a.as_ref();
    let id_b = report.self_hosted_identity.b.as_ref();

    let action_pairs: Vec<ActionPairRow> = report
        .action_refs
        .paired
        .iter()
        .map(paired_action_row)
        .collect();
    let action_only_in_a: Vec<ActionOnlyRow> = report
        .action_refs
        .only_in_a
        .iter()
        .map(action_only_row)
        .collect();
    let action_only_in_b: Vec<ActionOnlyRow> = report
        .action_refs
        .only_in_b
        .iter()
        .map(action_only_row)
        .collect();

    let step_groups: Vec<StepGroupView> = report
        .steps
        .paired_by_job
        .iter()
        .map(job_step_group_view)
        .collect();
    let steps_only_in_a: Vec<UnmatchedStepRow> = report
        .steps
        .only_in_a
        .iter()
        .map(unmatched_step_row)
        .collect();
    let steps_only_in_b: Vec<UnmatchedStepRow> = report
        .steps
        .only_in_b
        .iter()
        .map(unmatched_step_row)
        .collect();

    CompareResultTemplate {
        a_id: report.a_id.clone(),
        b_id: report.b_id.clone(),
        fuzzy: report.fuzzy,
        runner_type_a: report.runner_type.a.label().to_string(),
        runner_type_b: report.runner_type.b.label().to_string(),
        runner_type_differs: report.runner_type.differs,
        runner_version_a: opt_or_dash(&report.runner_version.a),
        runner_version_b: opt_or_dash(&report.runner_version.b),
        runner_version_differs: report.runner_version.differs,
        requested_labels_a: opt_or_dash(&report.requested_labels.a),
        requested_labels_b: opt_or_dash(&report.requested_labels.b),
        requested_labels_differs: report.requested_labels.differs,
        self_hosted_present,
        self_hosted_runner_name_a: identity_field(id_a, |i| &i.runner_name),
        self_hosted_runner_name_b: identity_field(id_b, |i| &i.runner_name),
        self_hosted_runner_group_a: identity_field(id_a, |i| &i.runner_group),
        self_hosted_runner_group_b: identity_field(id_b, |i| &i.runner_group),
        self_hosted_machine_name_a: identity_field(id_a, |i| &i.machine_name),
        self_hosted_machine_name_b: identity_field(id_b, |i| &i.machine_name),
        self_hosted_identity_differs: report.self_hosted_identity.differs,
        runner_image_present,
        runner_image_a_image: runner_image_a.image,
        runner_image_a_version: runner_image_a.version,
        runner_image_b_image: runner_image_b.image,
        runner_image_b_version: runner_image_b.version,
        runner_image_differs: report.runner_image.differs,
        runner_image_a_reported,
        runner_image_b_reported,
        azure_region_a: report.azure_region.a.clone().unwrap_or_else(|| "—".into()),
        azure_region_b: report.azure_region.b.clone().unwrap_or_else(|| "—".into()),
        azure_region_differs: report.azure_region.differs,
        azure_region_present,
        diagnostic_logs_a: report.diagnostic_logs.a,
        diagnostic_logs_b: report.diagnostic_logs.b,
        diagnostic_logs_differs: report.diagnostic_logs.differs,
        action_pairs,
        action_only_in_a,
        action_only_in_b,
        step_groups,
        steps_only_in_a,
        steps_only_in_b,
    }
}

fn opt_or_dash(value: &Option<String>) -> String {
    value.clone().unwrap_or_else(|| "—".into())
}

fn identity_field<F>(identity: Option<&compare::SelfHostedIdentity>, f: F) -> String
where
    F: FnOnce(&compare::SelfHostedIdentity) -> &Option<String>,
{
    match identity {
        Some(i) => f(i).clone().unwrap_or_else(|| "—".into()),
        None => "—".into(),
    }
}

fn paired_action_row(p: &PairedActionRef) -> ActionPairRow {
    ActionPairRow {
        owner_repo_path: p.owner_repo_path.clone(),
        github_url: p.github_url.clone(),
        a_reference: p.a_reference.clone(),
        b_reference: p.b_reference.clone(),
        differs: p.differs,
    }
}

fn action_only_row(a: &ActionReference) -> ActionOnlyRow {
    let key = match &a.path {
        Some(p) => format!("{}/{}{}", a.owner, a.repo, p),
        None => format!("{}/{}", a.owner, a.repo),
    };
    ActionOnlyRow {
        owner_repo_path: key,
        github_url: a.github_url(),
        reference: a.reference.clone(),
    }
}

fn job_step_group_view(g: &JobStepGroup) -> StepGroupView {
    StepGroupView {
        job_folder: g.job_folder.clone(),
        steps: g.steps.iter().map(paired_step_row).collect(),
    }
}

fn paired_step_row(s: &PairedStep) -> StepRow {
    let (delta_display, delta_class) = match s.delta_ms {
        None => ("—".to_string(), "delta-na"),
        Some(d) if d > 0 => (format!("+{}", format_duration_ms(d)), "delta-slower"),
        Some(d) if d < 0 => (format!("-{}", format_duration_ms(-d)), "delta-faster"),
        Some(_) => ("0s".to_string(), "delta-same"),
    };
    StepRow {
        label: s.label.clone(),
        a_duration_display: format_duration_opt(s.a_duration_ms),
        b_duration_display: format_duration_opt(s.b_duration_ms),
        delta_display,
        delta_class,
    }
}

fn unmatched_step_row(s: &compare::StepSummary) -> UnmatchedStepRow {
    UnmatchedStepRow {
        job_folder: s.job_folder.clone(),
        step_file: s.step_file.clone(),
        duration_display: format_duration_opt(s.duration_ms),
    }
}

fn format_duration_opt(ms: Option<i64>) -> String {
    match ms {
        Some(ms) => format_duration_ms(ms),
        None => "—".into(),
    }
}

fn format_duration_ms(ms: i64) -> String {
    if ms < 1000 {
        return format!("{ms}ms");
    }
    let total_secs = ms / 1000;
    let secs = total_secs % 60;
    let mins = (total_secs / 60) % 60;
    let hours = total_secs / 3600;
    if hours > 0 {
        format!("{hours}h {mins}m {secs}s")
    } else if mins > 0 {
        format!("{mins}m {secs}s")
    } else {
        format!("{secs}s")
    }
}
