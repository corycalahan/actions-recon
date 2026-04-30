//! Archive summarization for the compare view.
//!
//! Walks an extracted analysis directory and builds an `ArchiveSummary`
//! containing the structured signals used by the diff:
//!   - runner type classification (GitHub-hosted vs self-hosted vs unknown)
//!   - runner identity (version, name, group, machine, requested labels)
//!   - runner image (image name + version, hosted only)
//!   - Azure region (hosted only)
//!   - referenced GitHub Actions
//!   - per-step durations

use std::fs;
use std::path::Path;

use crate::models::log_parser::{self, ActionReference, LogEntry};

/// Runner image block contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerImage {
    pub image: String,
    pub version: String,
}

/// Coarse classification of how the job's runner was provisioned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerType {
    /// `Hosted Compute Agent` line was present in the Runner Image Provisioner block.
    GitHubHosted,
    /// Self-hosted markers (`Runner name:` / `Machine name:`) present, no Provisioner block.
    SelfHosted,
    /// Could not classify — neither marker found (older log formats or unusual environments).
    Unknown,
}

impl RunnerType {
    pub fn label(self) -> &'static str {
        match self {
            RunnerType::GitHubHosted => "GitHub-hosted",
            RunnerType::SelfHosted => "Self-hosted",
            RunnerType::Unknown => "Unknown",
        }
    }
}

/// Identity fields emitted only by self-hosted runners (top of `1_Set up job.txt`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfHostedIdentity {
    pub runner_name: Option<String>,
    pub runner_group: Option<String>,
    pub machine_name: Option<String>,
}

impl SelfHostedIdentity {
    fn is_empty(&self) -> bool {
        self.runner_name.is_none() && self.runner_group.is_none() && self.machine_name.is_none()
    }
}

/// One step's timing signature, used for duration comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepSummary {
    /// Job folder name (e.g. `hello-world`).
    pub job_folder: String,
    /// Step file name (e.g. `3_Run a one-line script.txt`).
    pub step_file: String,
    /// Normalized step name for fuzzy matching (digit prefix and `.txt` stripped, lowercased).
    pub normalized_name: String,
    /// First timestamped epoch (ms) seen in the step log.
    pub first_epoch_ms: Option<i64>,
    /// Last timestamped epoch (ms) seen in the step log.
    pub last_epoch_ms: Option<i64>,
    /// Computed duration in ms (last minus first), if both timestamps were found.
    pub duration_ms: Option<i64>,
}

/// Summary of one extracted analysis archive.
#[derive(Debug, Clone)]
pub struct ArchiveSummary {
    pub analysis_id: String,
    /// Coarse classification — GitHub-hosted, self-hosted, or unknown.
    pub runner_type: RunnerType,
    /// Runner application version (e.g. `2.334.0`) from `Current runner version: '...'`.
    pub runner_version: Option<String>,
    /// Requested job labels from `system.txt` (e.g. `ubuntu-latest`, `self-hosted`).
    pub requested_labels: Option<String>,
    /// Self-hosted identity fields (only populated when `runner_type == SelfHosted`).
    pub self_hosted_identity: Option<SelfHostedIdentity>,
    pub runner_image: Option<RunnerImage>,
    pub azure_region: Option<String>,
    /// Deduplicated action references aggregated across all step logs.
    pub action_refs: Vec<ActionReference>,
    /// Whether `runner-diagnostic-logs/` is present in this archive.
    pub diagnostic_logs_enabled: bool,
    pub steps: Vec<StepSummary>,
}

/// Walk `analysis_dir` and build a summary.
///
/// `analysis_dir` should be the canonicalized directory for one extracted
/// upload (e.g. `data/uploads/<id>`). Errors reading individual files are
/// silently skipped — partial summaries are better than failing the whole
/// compare view.
pub fn summarize_archive(analysis_id: &str, analysis_dir: &Path) -> ArchiveSummary {
    let mut runner_type_indicator: Option<String> = None;
    let mut runner_image: Option<RunnerImage> = None;
    let mut azure_region: Option<String> = None;
    let mut runner_version: Option<String> = None;
    let mut self_hosted_identity_raw: Option<SelfHostedIdentity> = None;
    let mut requested_labels: Option<String> = None;
    let mut all_action_refs: Vec<ActionReference> = Vec::new();
    let mut steps: Vec<StepSummary> = Vec::new();
    let mut diagnostic_logs_enabled = false;

    // Iterate top-level entries; each subdirectory is treated as a job folder.
    let Ok(entries) = fs::read_dir(analysis_dir) else {
        return ArchiveSummary {
            analysis_id: analysis_id.to_string(),
            runner_type: RunnerType::Unknown,
            runner_version,
            requested_labels,
            self_hosted_identity: None,
            runner_image,
            azure_region,
            action_refs: all_action_refs,
            diagnostic_logs_enabled,
            steps,
        };
    };

    for top in entries.flatten() {
        let top_path = top.path();
        let top_name = top_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        if top_path.is_dir() {
            if top_name == "runner-diagnostic-logs" {
                diagnostic_logs_enabled = true;
                continue;
            }

            // Job folder — iterate step files inside it.
            let Ok(inner) = fs::read_dir(&top_path) else {
                continue;
            };

            for step_entry in inner.flatten() {
                let step_path = step_entry.path();
                if !step_path.is_file() {
                    continue;
                }
                let step_file = step_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();
                if !step_file.ends_with(".txt") {
                    continue;
                }

                let Ok(content) = fs::read_to_string(&step_path) else {
                    continue;
                };
                let parsed = log_parser::parse_workflow_log(&content);

                // Aggregate signals from the "Set up job" step file.
                // The runner image / provisioner blocks live there.
                if runner_type_indicator.is_none() {
                    runner_type_indicator = extract_runner_type(&parsed);
                }
                if runner_image.is_none() {
                    runner_image = extract_runner_image(&parsed);
                }
                if azure_region.is_none() {
                    azure_region = extract_azure_region(&parsed);
                }
                if runner_version.is_none() {
                    runner_version = extract_runner_version(&parsed);
                }
                if self_hosted_identity_raw.is_none() {
                    self_hosted_identity_raw = extract_self_hosted_identity(&parsed);
                }
                if requested_labels.is_none() && step_file == "system.txt" {
                    requested_labels = extract_requested_labels(&parsed);
                }

                // Extract action refs from this step log and dedupe later.
                let refs = log_parser::extract_action_references(&parsed);
                all_action_refs.extend(refs);

                // Compute step duration from first/last timestamp.
                let first_epoch_ms = parsed.iter().find_map(|e| e.epoch_millis);
                let last_epoch_ms = parsed.iter().rev().find_map(|e| e.epoch_millis);
                let duration_ms = match (first_epoch_ms, last_epoch_ms) {
                    (Some(a), Some(b)) if b >= a => Some(b - a),
                    _ => None,
                };

                steps.push(StepSummary {
                    job_folder: top_name.clone(),
                    step_file: step_file.clone(),
                    normalized_name: normalize_step_name(&step_file),
                    first_epoch_ms,
                    last_epoch_ms,
                    duration_ms,
                });
            }
        }
    }

    // Classify runner type.
    let runner_type = if runner_type_indicator.is_some() {
        RunnerType::GitHubHosted
    } else if self_hosted_identity_raw
        .as_ref()
        .is_some_and(|i| !i.is_empty())
    {
        RunnerType::SelfHosted
    } else {
        RunnerType::Unknown
    };
    // Only surface the self-hosted identity when we classified as self-hosted.
    let self_hosted_identity = match runner_type {
        RunnerType::SelfHosted => self_hosted_identity_raw,
        _ => None,
    };

    // Dedupe action refs by (owner, repo, path, reference), keeping earliest line.
    all_action_refs.sort_by(|a, b| {
        (
            a.owner.clone(),
            a.repo.clone(),
            a.path.clone(),
            a.reference.clone(),
            a.first_line,
        )
            .cmp(&(
                b.owner.clone(),
                b.repo.clone(),
                b.path.clone(),
                b.reference.clone(),
                b.first_line,
            ))
    });
    all_action_refs.dedup_by(|a, b| {
        a.owner == b.owner && a.repo == b.repo && a.path == b.path && a.reference == b.reference
    });

    // Stable ordering for steps: job folder, then step file.
    steps.sort_by(|a, b| {
        a.job_folder
            .cmp(&b.job_folder)
            .then_with(|| a.step_file.cmp(&b.step_file))
    });

    ArchiveSummary {
        analysis_id: analysis_id.to_string(),
        runner_type,
        runner_version,
        requested_labels,
        self_hosted_identity,
        runner_image,
        azure_region,
        action_refs: all_action_refs,
        diagnostic_logs_enabled,
        steps,
    }
}

/// Search for the `Hosted Compute Agent` line inside the
/// `##[group]Runner Image Provisioner` block. Returns the indicator string
/// verbatim (currently always "Hosted Compute Agent" if found).
pub fn extract_runner_type(entries: &[LogEntry]) -> Option<String> {
    let mut in_provisioner = false;
    for e in entries {
        if e.group_start && e.message.trim() == "Runner Image Provisioner" {
            in_provisioner = true;
            continue;
        }
        if e.group_end {
            in_provisioner = false;
            continue;
        }
        if in_provisioner && e.message.trim() == "Hosted Compute Agent" {
            return Some("Hosted Compute Agent".to_string());
        }
    }
    None
}

/// Parse the `##[group]Runner Image` block for `Image:` and `Version:` lines.
pub fn extract_runner_image(entries: &[LogEntry]) -> Option<RunnerImage> {
    let mut in_block = false;
    let mut image: Option<String> = None;
    let mut version: Option<String> = None;

    for e in entries {
        if e.group_start && e.message.trim() == "Runner Image" {
            in_block = true;
            continue;
        }
        if e.group_end {
            if in_block {
                break;
            }
            continue;
        }
        if !in_block {
            continue;
        }
        if let Some(v) = e.message.strip_prefix("Image:") {
            image = Some(v.trim().to_string());
        } else if let Some(v) = e.message.strip_prefix("Version:") {
            version = Some(v.trim().to_string());
        }
    }

    match (image, version) {
        (Some(image), Some(version)) => Some(RunnerImage { image, version }),
        _ => None,
    }
}

/// Parse the `##[group]Runner Image Provisioner` block for `Azure Region:`.
pub fn extract_azure_region(entries: &[LogEntry]) -> Option<String> {
    let mut in_provisioner = false;
    for e in entries {
        if e.group_start && e.message.trim() == "Runner Image Provisioner" {
            in_provisioner = true;
            continue;
        }
        if e.group_end {
            if in_provisioner {
                break;
            }
            continue;
        }
        if !in_provisioner {
            continue;
        }
        if let Some(v) = e.message.strip_prefix("Azure Region:") {
            let region = v.trim();
            if !region.is_empty() {
                return Some(region.to_string());
            }
        }
    }
    None
}

/// Extract the runner application version from `Current runner version: '...'`.
/// Present in both hosted and self-hosted setup logs.
pub fn extract_runner_version(entries: &[LogEntry]) -> Option<String> {
    for e in entries {
        if let Some(rest) = e.message.strip_prefix("Current runner version:") {
            // Strip surrounding whitespace and the wrapping single quotes.
            let trimmed = rest.trim();
            let unquoted = trimmed
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
                .unwrap_or(trimmed);
            if !unquoted.is_empty() {
                return Some(unquoted.to_string());
            }
        }
    }
    None
}

/// Extract self-hosted identity fields from the top of `1_Set up job.txt`.
///
/// Self-hosted runners emit:
///   Runner name: 'W6L71QWDTQ'
///   Runner group name: 'Default'
///   Machine name: 'W6L71QWDTQ'
///
/// Returns `None` when none of those lines are present (typical for hosted runners).
/// Otherwise returns `Some(SelfHostedIdentity { ... })` with whichever fields were found.
pub fn extract_self_hosted_identity(entries: &[LogEntry]) -> Option<SelfHostedIdentity> {
    let mut runner_name: Option<String> = None;
    let mut runner_group: Option<String> = None;
    let mut machine_name: Option<String> = None;

    for e in entries {
        // These lines all appear before any `##[group]` header in the setup log,
        // but checking the whole entry list is cheap and tolerates variations.
        if let Some(v) = e.message.strip_prefix("Runner name:")
            && runner_name.is_none()
        {
            runner_name = Some(unquote(v.trim()));
        } else if let Some(v) = e.message.strip_prefix("Runner group name:")
            && runner_group.is_none()
        {
            runner_group = Some(unquote(v.trim()));
        } else if let Some(v) = e.message.strip_prefix("Machine name:")
            && machine_name.is_none()
        {
            machine_name = Some(unquote(v.trim()));
        }
    }

    let identity = SelfHostedIdentity {
        runner_name,
        runner_group,
        machine_name,
    };
    if identity.is_empty() {
        None
    } else {
        Some(identity)
    }
}

/// Extract the `Requested labels:` value from `system.txt`.
///
/// Examples: `ubuntu-latest`, `self-hosted`, `self-hosted, macOS`.
pub fn extract_requested_labels(entries: &[LogEntry]) -> Option<String> {
    for e in entries {
        if let Some(rest) = e.message.strip_prefix("Requested labels:") {
            let trimmed = rest.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn unquote(s: &str) -> String {
    s.strip_prefix('\'')
        .and_then(|t| t.strip_suffix('\''))
        .unwrap_or(s)
        .to_string()
}

/// Normalize a step file name for fuzzy matching across reruns.
/// Strips a leading `\d+_` prefix, strips a trailing `.txt`, lowercases.
pub fn normalize_step_name(name: &str) -> String {
    let stem = name.strip_suffix(".txt").unwrap_or(name);
    // Strip leading digits + underscore
    let bytes = stem.as_bytes();
    let mut idx = 0;
    while idx < bytes.len() && bytes[idx].is_ascii_digit() {
        idx += 1;
    }
    let stripped = if idx > 0 && idx < bytes.len() && bytes[idx] == b'_' {
        &stem[idx + 1..]
    } else {
        stem
    };
    stripped.to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(content: &str) -> Vec<LogEntry> {
        log_parser::parse_workflow_log(content)
    }

    #[test]
    fn extracts_runner_image_from_synthetic_log() {
        let content = "\
2026-04-29T23:55:39.8686412Z ##[group]Runner Image
2026-04-29T23:55:39.8686888Z Image: ubuntu-24.04
2026-04-29T23:55:39.8687343Z Version: 20260413.86.1
2026-04-29T23:55:39.8688249Z Included Software: https://example.com
2026-04-29T23:55:39.8690476Z ##[endgroup]
";
        let entries = parse(content);
        let image = extract_runner_image(&entries).expect("should detect runner image");
        assert_eq!(image.image, "ubuntu-24.04");
        assert_eq!(image.version, "20260413.86.1");
    }

    #[test]
    fn extracts_runner_image_returns_none_when_block_absent() {
        let entries = parse("2026-04-29T23:55:39.8686412Z hello\n");
        assert!(extract_runner_image(&entries).is_none());
    }

    #[test]
    fn detects_hosted_compute_agent() {
        let content = "\
2026-04-29T23:55:39.8678146Z ##[group]Runner Image Provisioner
2026-04-29T23:55:39.8678959Z Hosted Compute Agent
2026-04-29T23:55:39.8679464Z Version: 20260213.493
2026-04-29T23:55:39.8682228Z Azure Region: westcentralus
2026-04-29T23:55:39.8682766Z ##[endgroup]
";
        let entries = parse(content);
        assert_eq!(
            extract_runner_type(&entries).as_deref(),
            Some("Hosted Compute Agent")
        );
    }

    #[test]
    fn detects_hosted_compute_agent_negative() {
        // The string appears outside the provisioner block; should not match.
        let content = "2026-04-29T23:55:39.0Z Hosted Compute Agent\n";
        let entries = parse(content);
        assert!(extract_runner_type(&entries).is_none());
    }

    #[test]
    fn extracts_azure_region_positive() {
        let content = "\
2026-04-29T23:55:39.8678146Z ##[group]Runner Image Provisioner
2026-04-29T23:55:39.8678959Z Hosted Compute Agent
2026-04-29T23:55:39.8682228Z Azure Region: westcentralus
2026-04-29T23:55:39.8682766Z ##[endgroup]
";
        let entries = parse(content);
        assert_eq!(
            extract_azure_region(&entries).as_deref(),
            Some("westcentralus")
        );
    }

    #[test]
    fn extracts_azure_region_returns_none_when_block_absent() {
        // No provisioner block at all (e.g. self-hosted runner).
        let content = "2026-04-29T23:55:39.0Z Current runner version: '2.334.0'\n";
        let entries = parse(content);
        assert!(extract_azure_region(&entries).is_none());
    }

    #[test]
    fn extracts_azure_region_returns_none_when_line_absent() {
        // Provisioner block exists but no Azure Region line.
        let content = "\
2026-04-29T23:55:39.0Z ##[group]Runner Image Provisioner
2026-04-29T23:55:39.0Z Hosted Compute Agent
2026-04-29T23:55:39.0Z ##[endgroup]
";
        let entries = parse(content);
        assert!(extract_azure_region(&entries).is_none());
    }

    #[test]
    fn computes_step_duration_from_first_last_timestamps() {
        // Build a tiny archive on disk and summarize it.
        let tmp = tempfile::tempdir().expect("tempdir");
        let analysis_dir = tmp.path().join("xyz");
        let job_dir = analysis_dir.join("hello-world");
        fs::create_dir_all(&job_dir).expect("mkdir");
        // Two-line step file: 1 second apart.
        let step = "\
2026-04-29T23:55:41.0000000Z First line\n\
2026-04-29T23:55:42.0000000Z Last line\n";
        fs::write(job_dir.join("3_Run a one-line script.txt"), step).expect("write");

        let summary = summarize_archive("xyz", &analysis_dir);
        assert_eq!(summary.steps.len(), 1);
        let s = &summary.steps[0];
        assert_eq!(s.job_folder, "hello-world");
        assert_eq!(s.step_file, "3_Run a one-line script.txt");
        assert_eq!(s.duration_ms, Some(1000));
    }

    #[test]
    fn summarize_detects_diagnostic_logs() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let analysis_dir = tmp.path().join("with-diag");
        fs::create_dir_all(analysis_dir.join("runner-diagnostic-logs")).expect("mkdir");
        let summary = summarize_archive("with-diag", &analysis_dir);
        assert!(summary.diagnostic_logs_enabled);
    }

    #[test]
    fn normalize_step_name_strips_prefix_and_suffix() {
        assert_eq!(
            normalize_step_name("3_Run a one-line script.txt"),
            "run a one-line script"
        );
        assert_eq!(normalize_step_name("12_Set up job.txt"), "set up job");
        // No digit prefix
        assert_eq!(normalize_step_name("system.txt"), "system");
        // Underscore not preceded by digits stays
        assert_eq!(normalize_step_name("my_step.txt"), "my_step");
    }

    #[test]
    fn extracts_runner_version() {
        let content = "2026-04-29T23:55:39.0Z Current runner version: '2.334.0'\n";
        let entries = parse(content);
        assert_eq!(extract_runner_version(&entries).as_deref(), Some("2.334.0"));
    }

    #[test]
    fn extracts_runner_version_returns_none_when_absent() {
        let entries = parse("2026-04-29T23:55:39.0Z hello\n");
        assert!(extract_runner_version(&entries).is_none());
    }

    #[test]
    fn extracts_self_hosted_identity_from_synthetic_log() {
        let content = "\
2026-04-30T00:42:29.0472190Z Current runner version: '2.334.0'
2026-04-30T00:42:29.0481180Z Runner name: 'W6L71QWDTQ'
2026-04-30T00:42:29.0481650Z Runner group name: 'Default'
2026-04-30T00:42:29.0482290Z Machine name: 'W6L71QWDTQ'
";
        let entries = parse(content);
        let identity = extract_self_hosted_identity(&entries).expect("identity should be detected");
        assert_eq!(identity.runner_name.as_deref(), Some("W6L71QWDTQ"));
        assert_eq!(identity.runner_group.as_deref(), Some("Default"));
        assert_eq!(identity.machine_name.as_deref(), Some("W6L71QWDTQ"));
    }

    #[test]
    fn extracts_self_hosted_identity_returns_none_for_hosted_log() {
        // A hosted setup log has no `Runner name:` or `Machine name:` lines.
        let content = "\
2026-04-29T23:55:39.0Z Current runner version: '2.334.0'
2026-04-29T23:55:39.0Z ##[group]Runner Image Provisioner
2026-04-29T23:55:39.0Z Hosted Compute Agent
2026-04-29T23:55:39.0Z ##[endgroup]
";
        let entries = parse(content);
        assert!(extract_self_hosted_identity(&entries).is_none());
    }

    #[test]
    fn extracts_requested_labels() {
        let content = "2026-04-29T23:55:37.4950000Z Requested labels: ubuntu-latest\n";
        let entries = parse(content);
        assert_eq!(
            extract_requested_labels(&entries).as_deref(),
            Some("ubuntu-latest")
        );
    }

    #[test]
    fn summarize_classifies_self_hosted_runner() {
        // Build a tiny archive that mirrors the macOS self-hosted sample.
        let tmp = tempfile::tempdir().expect("tempdir");
        let analysis_dir = tmp.path().join("sh");
        let job_dir = analysis_dir.join("hello-world");
        fs::create_dir_all(&job_dir).expect("mkdir");
        let setup = "\
2026-04-30T00:42:29.0472190Z Current runner version: '2.334.0'
2026-04-30T00:42:29.0481180Z Runner name: 'W6L71QWDTQ'
2026-04-30T00:42:29.0481650Z Runner group name: 'Default'
2026-04-30T00:42:29.0482290Z Machine name: 'W6L71QWDTQ'
";
        fs::write(job_dir.join("1_Set up job.txt"), setup).expect("write");
        let system = "2026-04-30T00:42:26.3560000Z Requested labels: self-hosted\n";
        fs::write(job_dir.join("system.txt"), system).expect("write");

        let summary = summarize_archive("sh", &analysis_dir);
        assert_eq!(summary.runner_type, RunnerType::SelfHosted);
        assert_eq!(summary.runner_version.as_deref(), Some("2.334.0"));
        assert_eq!(summary.requested_labels.as_deref(), Some("self-hosted"));
        let identity = summary
            .self_hosted_identity
            .expect("self-hosted identity present");
        assert_eq!(identity.runner_name.as_deref(), Some("W6L71QWDTQ"));
        assert_eq!(identity.machine_name.as_deref(), Some("W6L71QWDTQ"));
        assert!(summary.runner_image.is_none());
        assert!(summary.azure_region.is_none());
    }

    #[test]
    fn summarize_classifies_github_hosted_runner() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let analysis_dir = tmp.path().join("gh");
        let job_dir = analysis_dir.join("hello-world");
        fs::create_dir_all(&job_dir).expect("mkdir");
        let setup = "\
2026-04-29T23:55:39.0Z Current runner version: '2.334.0'
2026-04-29T23:55:39.0Z ##[group]Runner Image Provisioner
2026-04-29T23:55:39.0Z Hosted Compute Agent
2026-04-29T23:55:39.0Z Azure Region: westcentralus
2026-04-29T23:55:39.0Z ##[endgroup]
2026-04-29T23:55:39.0Z ##[group]Runner Image
2026-04-29T23:55:39.0Z Image: ubuntu-24.04
2026-04-29T23:55:39.0Z Version: 20260413.86.1
2026-04-29T23:55:39.0Z ##[endgroup]
";
        fs::write(job_dir.join("1_Set up job.txt"), setup).expect("write");

        let summary = summarize_archive("gh", &analysis_dir);
        assert_eq!(summary.runner_type, RunnerType::GitHubHosted);
        assert!(summary.self_hosted_identity.is_none());
        assert_eq!(summary.azure_region.as_deref(), Some("westcentralus"));
        assert!(summary.runner_image.is_some());
    }
}
