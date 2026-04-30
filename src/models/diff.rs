//! Pairing and diffing of two `ArchiveSummary` values.

use std::collections::BTreeMap;

use crate::models::compare::{
    ArchiveSummary, RunnerImage, RunnerType, SelfHostedIdentity, StepSummary,
};
use crate::models::log_parser::ActionReference;

/// A side-by-side comparison of a single optional value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StringComparison {
    pub a: Option<String>,
    pub b: Option<String>,
    pub differs: bool,
}

impl StringComparison {
    fn new(a: Option<String>, b: Option<String>) -> Self {
        let differs = a != b;
        Self { a, b, differs }
    }
}

/// A side-by-side comparison of an optional `RunnerImage`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerImageComparison {
    pub a: Option<RunnerImage>,
    pub b: Option<RunnerImage>,
    pub differs: bool,
}

impl RunnerImageComparison {
    fn new(a: Option<RunnerImage>, b: Option<RunnerImage>) -> Self {
        let differs = a != b;
        Self { a, b, differs }
    }
}

/// A boolean side-by-side comparison (e.g. diagnostic-logs-enabled).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoolComparison {
    pub a: bool,
    pub b: bool,
    pub differs: bool,
}

impl BoolComparison {
    fn new(a: bool, b: bool) -> Self {
        Self {
            a,
            b,
            differs: a != b,
        }
    }
}

/// A side-by-side comparison of `RunnerType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunnerTypeComparison {
    pub a: RunnerType,
    pub b: RunnerType,
    pub differs: bool,
}

impl RunnerTypeComparison {
    fn new(a: RunnerType, b: RunnerType) -> Self {
        Self {
            a,
            b,
            differs: a != b,
        }
    }
}

/// A side-by-side comparison of an optional `SelfHostedIdentity`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityComparison {
    pub a: Option<SelfHostedIdentity>,
    pub b: Option<SelfHostedIdentity>,
    pub differs: bool,
}

impl IdentityComparison {
    fn new(a: Option<SelfHostedIdentity>, b: Option<SelfHostedIdentity>) -> Self {
        let differs = a != b;
        Self { a, b, differs }
    }
}

/// A paired action reference present in both archives.
#[derive(Debug, Clone)]
pub struct PairedActionRef {
    pub owner_repo_path: String,
    pub github_url: String,
    pub a_reference: String,
    pub b_reference: String,
    pub differs: bool,
}

/// Buckets of action-ref differences between two archives.
#[derive(Debug, Clone, Default)]
pub struct ActionRefDiff {
    pub paired: Vec<PairedActionRef>,
    pub only_in_a: Vec<ActionReference>,
    pub only_in_b: Vec<ActionReference>,
}

/// A paired step (same job folder + step file, or fuzzy match).
#[derive(Debug, Clone)]
pub struct PairedStep {
    pub label: String,
    pub a_duration_ms: Option<i64>,
    pub b_duration_ms: Option<i64>,
    pub delta_ms: Option<i64>,
}

/// Buckets of step-pairing results.
#[derive(Debug, Clone, Default)]
pub struct StepDiff {
    /// Steps present in both archives. May span multiple job folders.
    pub paired_by_job: Vec<JobStepGroup>,
    pub only_in_a: Vec<StepSummary>,
    pub only_in_b: Vec<StepSummary>,
}

/// Group of paired steps within one job folder, for table-grouped rendering.
#[derive(Debug, Clone)]
pub struct JobStepGroup {
    pub job_folder: String,
    pub steps: Vec<PairedStep>,
}

/// Top-level comparison result handed to the template.
#[derive(Debug, Clone)]
pub struct ComparisonReport {
    pub a_id: String,
    pub b_id: String,
    pub fuzzy: bool,
    pub runner_type: RunnerTypeComparison,
    pub runner_version: StringComparison,
    pub requested_labels: StringComparison,
    pub self_hosted_identity: IdentityComparison,
    pub runner_image: RunnerImageComparison,
    pub azure_region: StringComparison,
    pub diagnostic_logs: BoolComparison,
    pub action_refs: ActionRefDiff,
    pub steps: StepDiff,
}

/// Compute the comparison report between two archive summaries.
pub fn compare(a: &ArchiveSummary, b: &ArchiveSummary, fuzzy: bool) -> ComparisonReport {
    ComparisonReport {
        a_id: a.analysis_id.clone(),
        b_id: b.analysis_id.clone(),
        fuzzy,
        runner_type: RunnerTypeComparison::new(a.runner_type, b.runner_type),
        runner_version: StringComparison::new(a.runner_version.clone(), b.runner_version.clone()),
        requested_labels: StringComparison::new(
            a.requested_labels.clone(),
            b.requested_labels.clone(),
        ),
        self_hosted_identity: IdentityComparison::new(
            a.self_hosted_identity.clone(),
            b.self_hosted_identity.clone(),
        ),
        runner_image: RunnerImageComparison::new(a.runner_image.clone(), b.runner_image.clone()),
        azure_region: StringComparison::new(a.azure_region.clone(), b.azure_region.clone()),
        diagnostic_logs: BoolComparison::new(a.diagnostic_logs_enabled, b.diagnostic_logs_enabled),
        action_refs: diff_action_refs(&a.action_refs, &b.action_refs),
        steps: diff_steps(&a.steps, &b.steps, fuzzy),
    }
}

fn key_owner_repo_path(r: &ActionReference) -> String {
    match &r.path {
        Some(p) => format!("{}/{}{}", r.owner, r.repo, p),
        None => format!("{}/{}", r.owner, r.repo),
    }
}

fn diff_action_refs(a: &[ActionReference], b: &[ActionReference]) -> ActionRefDiff {
    // Bucket each side by (owner, repo, path). If multiple refs share the same
    // key on one side, keep the first one (earliest line).
    let mut a_by_key: BTreeMap<String, ActionReference> = BTreeMap::new();
    for r in a {
        a_by_key
            .entry(key_owner_repo_path(r))
            .or_insert_with(|| r.clone());
    }
    let mut b_by_key: BTreeMap<String, ActionReference> = BTreeMap::new();
    for r in b {
        b_by_key
            .entry(key_owner_repo_path(r))
            .or_insert_with(|| r.clone());
    }

    let mut paired: Vec<PairedActionRef> = Vec::new();
    let mut only_in_a: Vec<ActionReference> = Vec::new();
    let mut only_in_b: Vec<ActionReference> = Vec::new();

    let mut all_keys: Vec<&String> = a_by_key.keys().chain(b_by_key.keys()).collect();
    all_keys.sort();
    all_keys.dedup();

    for key in all_keys {
        match (a_by_key.get(key), b_by_key.get(key)) {
            (Some(ra), Some(rb)) => {
                let differs = ra.reference != rb.reference;
                paired.push(PairedActionRef {
                    owner_repo_path: key.clone(),
                    github_url: ra.github_url(),
                    a_reference: ra.reference.clone(),
                    b_reference: rb.reference.clone(),
                    differs,
                });
            }
            (Some(ra), None) => only_in_a.push(ra.clone()),
            (None, Some(rb)) => only_in_b.push(rb.clone()),
            (None, None) => {}
        }
    }

    ActionRefDiff {
        paired,
        only_in_a,
        only_in_b,
    }
}

fn diff_steps(a: &[StepSummary], b: &[StepSummary], fuzzy: bool) -> StepDiff {
    // Pair by exact (job_folder, step_file) first.
    let mut a_used = vec![false; a.len()];
    let mut b_used = vec![false; b.len()];
    let mut paired_pairs: Vec<(usize, usize)> = Vec::new();

    for (i, sa) in a.iter().enumerate() {
        for (j, sb) in b.iter().enumerate() {
            if !b_used[j] && sa.job_folder == sb.job_folder && sa.step_file == sb.step_file {
                paired_pairs.push((i, j));
                a_used[i] = true;
                b_used[j] = true;
                break;
            }
        }
    }

    if fuzzy {
        // Pair leftovers by normalized name across job folders.
        for i in 0..a.len() {
            if a_used[i] {
                continue;
            }
            for j in 0..b.len() {
                if b_used[j] {
                    continue;
                }
                if a[i].normalized_name == b[j].normalized_name {
                    paired_pairs.push((i, j));
                    a_used[i] = true;
                    b_used[j] = true;
                    break;
                }
            }
        }
    }

    // Group paired steps by the A-side job folder.
    let mut by_job: BTreeMap<String, Vec<PairedStep>> = BTreeMap::new();
    for (i, j) in &paired_pairs {
        let sa = &a[*i];
        let sb = &b[*j];
        let label = if sa.step_file == sb.step_file {
            sa.step_file.clone()
        } else {
            format!("{} ⇄ {}", sa.step_file, sb.step_file)
        };
        let delta_ms = match (sa.duration_ms, sb.duration_ms) {
            (Some(da), Some(db)) => Some(db - da),
            _ => None,
        };
        by_job
            .entry(sa.job_folder.clone())
            .or_default()
            .push(PairedStep {
                label,
                a_duration_ms: sa.duration_ms,
                b_duration_ms: sb.duration_ms,
                delta_ms,
            });
    }

    let paired_by_job: Vec<JobStepGroup> = by_job
        .into_iter()
        .map(|(job_folder, steps)| JobStepGroup { job_folder, steps })
        .collect();

    let only_in_a: Vec<StepSummary> = a
        .iter()
        .enumerate()
        .filter(|(i, _)| !a_used[*i])
        .map(|(_, s)| s.clone())
        .collect();
    let only_in_b: Vec<StepSummary> = b
        .iter()
        .enumerate()
        .filter(|(j, _)| !b_used[*j])
        .map(|(_, s)| s.clone())
        .collect();

    StepDiff {
        paired_by_job,
        only_in_a,
        only_in_b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aref(owner: &str, repo: &str, reference: &str) -> ActionReference {
        ActionReference {
            owner: owner.to_string(),
            repo: repo.to_string(),
            path: None,
            reference: reference.to_string(),
            first_line: 1,
        }
    }

    fn step(job: &str, file: &str, dur: Option<i64>) -> StepSummary {
        StepSummary {
            job_folder: job.to_string(),
            step_file: file.to_string(),
            normalized_name: crate::models::compare::normalize_step_name(file),
            first_epoch_ms: dur.map(|_| 0),
            last_epoch_ms: dur,
            duration_ms: dur,
        }
    }

    fn empty_summary(id: &str) -> ArchiveSummary {
        ArchiveSummary {
            analysis_id: id.to_string(),
            runner_type: RunnerType::Unknown,
            runner_version: None,
            requested_labels: None,
            self_hosted_identity: None,
            runner_image: None,
            azure_region: None,
            action_refs: Vec::new(),
            diagnostic_logs_enabled: false,
            steps: Vec::new(),
        }
    }

    #[test]
    fn flags_azure_region_difference() {
        let mut a = empty_summary("a");
        let mut b = empty_summary("b");
        a.azure_region = Some("westcentralus".to_string());
        b.azure_region = Some("eastus".to_string());
        let report = compare(&a, &b, false);
        assert!(report.azure_region.differs);
        assert_eq!(report.azure_region.a.as_deref(), Some("westcentralus"));
        assert_eq!(report.azure_region.b.as_deref(), Some("eastus"));
    }

    #[test]
    fn flags_azure_region_same() {
        let mut a = empty_summary("a");
        let mut b = empty_summary("b");
        a.azure_region = Some("eastus".to_string());
        b.azure_region = Some("eastus".to_string());
        let report = compare(&a, &b, false);
        assert!(!report.azure_region.differs);
    }

    #[test]
    fn flags_runner_image_difference() {
        let mut a = empty_summary("a");
        let mut b = empty_summary("b");
        a.runner_image = Some(RunnerImage {
            image: "ubuntu-24.04".into(),
            version: "20260101.1".into(),
        });
        b.runner_image = Some(RunnerImage {
            image: "ubuntu-24.04".into(),
            version: "20260413.86.1".into(),
        });
        let report = compare(&a, &b, false);
        assert!(report.runner_image.differs);
    }

    #[test]
    fn flags_runner_type_same() {
        let mut a = empty_summary("a");
        let mut b = empty_summary("b");
        a.runner_type = RunnerType::GitHubHosted;
        b.runner_type = RunnerType::GitHubHosted;
        let report = compare(&a, &b, false);
        assert!(!report.runner_type.differs);
    }

    #[test]
    fn flags_runner_type_difference_hosted_vs_self_hosted() {
        let mut a = empty_summary("a");
        let mut b = empty_summary("b");
        a.runner_type = RunnerType::GitHubHosted;
        b.runner_type = RunnerType::SelfHosted;
        let report = compare(&a, &b, false);
        assert!(report.runner_type.differs);
        assert_eq!(report.runner_type.a, RunnerType::GitHubHosted);
        assert_eq!(report.runner_type.b, RunnerType::SelfHosted);
    }

    #[test]
    fn buckets_action_refs_correctly() {
        let mut a = empty_summary("a");
        let mut b = empty_summary("b");
        // Same on both sides
        a.action_refs.push(aref("actions", "checkout", "v4"));
        b.action_refs.push(aref("actions", "checkout", "v4"));
        // Changed reference
        a.action_refs.push(aref("actions", "setup-node", "v3"));
        b.action_refs.push(aref("actions", "setup-node", "v4"));
        // Only in A
        a.action_refs.push(aref("actions", "cache", "v3"));
        // Only in B
        b.action_refs.push(aref("actions", "upload-artifact", "v4"));

        let report = compare(&a, &b, false);
        let paired = &report.action_refs.paired;
        assert_eq!(paired.len(), 2);

        let same = paired
            .iter()
            .find(|p| p.owner_repo_path == "actions/checkout")
            .expect("same pair");
        assert!(!same.differs);

        let changed = paired
            .iter()
            .find(|p| p.owner_repo_path == "actions/setup-node")
            .expect("changed pair");
        assert!(changed.differs);
        assert_eq!(changed.a_reference, "v3");
        assert_eq!(changed.b_reference, "v4");

        assert_eq!(report.action_refs.only_in_a.len(), 1);
        assert_eq!(report.action_refs.only_in_a[0].repo, "cache");
        assert_eq!(report.action_refs.only_in_b.len(), 1);
        assert_eq!(report.action_refs.only_in_b[0].repo, "upload-artifact");
    }

    #[test]
    fn pairs_steps_exact() {
        let mut a = empty_summary("a");
        let mut b = empty_summary("b");
        a.steps
            .push(step("hello-world", "1_Set up job.txt", Some(1000)));
        a.steps.push(step(
            "hello-world",
            "3_Run a one-line script.txt",
            Some(2000),
        ));
        b.steps
            .push(step("hello-world", "1_Set up job.txt", Some(1500)));
        // Different prefix → does not pair under exact mode
        b.steps.push(step(
            "hello-world",
            "5_Run a one-line script.txt",
            Some(2500),
        ));

        let report = compare(&a, &b, false);
        assert_eq!(report.steps.paired_by_job.len(), 1);
        let group = &report.steps.paired_by_job[0];
        assert_eq!(group.job_folder, "hello-world");
        assert_eq!(group.steps.len(), 1);
        assert_eq!(group.steps[0].label, "1_Set up job.txt");
        assert_eq!(group.steps[0].delta_ms, Some(500));
        assert_eq!(report.steps.only_in_a.len(), 1);
        assert_eq!(report.steps.only_in_b.len(), 1);
    }

    #[test]
    fn pairs_steps_fuzzy() {
        let mut a = empty_summary("a");
        let mut b = empty_summary("b");
        a.steps.push(step(
            "hello-world",
            "3_Run a one-line script.txt",
            Some(2000),
        ));
        // Same normalized name but different prefix → only pairs with fuzzy on
        b.steps.push(step(
            "hello-world",
            "5_Run a one-line script.txt",
            Some(2500),
        ));

        let exact = compare(&a, &b, false);
        assert!(exact.steps.paired_by_job.is_empty());
        assert_eq!(exact.steps.only_in_a.len(), 1);
        assert_eq!(exact.steps.only_in_b.len(), 1);

        let fuzzy = compare(&a, &b, true);
        assert_eq!(fuzzy.steps.paired_by_job.len(), 1);
        let group = &fuzzy.steps.paired_by_job[0];
        assert_eq!(group.steps.len(), 1);
        assert_eq!(group.steps[0].delta_ms, Some(500));
        assert!(fuzzy.steps.only_in_a.is_empty());
        assert!(fuzzy.steps.only_in_b.is_empty());
    }
}
