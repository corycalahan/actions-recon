//! Troubleshooting tips library — loads tips from TOML files and evaluates
//! them against parsed log entries.

use std::path::Path;

use regex::Regex;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::models::log_parser::{LogEntry, LogLevel};

const CURRENT_TIP_SCHEMA_VERSION: u32 = 1;

// ── TOML schema ──────────────────────────────────────────────────────────────

/// Raw TOML representation of a single tip file.
#[derive(Debug, Deserialize, Serialize)]
struct TipToml {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    schema_version: Option<u32>,
    id: String,
    name: String,
    emoji: String,
    #[serde(default)]
    docs: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    created: Option<String>,
    #[serde(default)]
    updated: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    enabled: Option<bool>,
    check: CheckToml,
}

/// The `[check]` table in a tip TOML file.
#[derive(Debug, Deserialize, Serialize)]
struct CheckToml {
    r#type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    applies_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pattern: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    patterns: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    threshold_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    step: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mark: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    threshold: Option<usize>,
}

// ── Loaded tip ───────────────────────────────────────────────────────────────

/// A fully validated and ready-to-evaluate tip.
#[derive(Debug, Clone)]
pub struct Tip {
    pub id: String,
    pub name: String,
    pub emoji: String,
    pub docs: Option<String>,
    pub description: Option<String>,
    pub enabled: bool,
    pub scope: TipScope,
    pub applies_to: TipAppliesTo,
    pub check: Check,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TipScope {
    All,
    Workflow,
    Runner,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogKind {
    Workflow,
    Runner,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TipAppliesTo {
    All,
    StandardLogs,
    DebugLogsEnabled,
    DiagnosticLogsEnabled,
}

impl TipAppliesTo {
    fn matches(&self, log_kind: LogKind, has_debug_lines: bool) -> bool {
        match self {
            TipAppliesTo::All => true,
            TipAppliesTo::StandardLogs => log_kind == LogKind::Workflow && !has_debug_lines,
            TipAppliesTo::DebugLogsEnabled => log_kind == LogKind::Workflow && has_debug_lines,
            TipAppliesTo::DiagnosticLogsEnabled => log_kind == LogKind::Runner,
        }
    }
}

impl TipScope {
    fn matches(&self, log_kind: LogKind) -> bool {
        match self {
            TipScope::All => true,
            TipScope::Workflow => log_kind == LogKind::Workflow,
            TipScope::Runner => log_kind == LogKind::Runner,
        }
    }
}

/// The check logic for a tip.
#[derive(Debug, Clone)]
pub enum Check {
    /// Flag every line whose *original log line* (message) matches the regex.
    PatternMatch { regex: Regex },
    /// Flag lines matching any of a list of regex patterns.
    ContainsAnyPatterns { regexes: Vec<Regex> },
    /// Flag when total elapsed time exceeds a threshold.
    TimeDelta {
        threshold_ms: i64,
        mark: TimeDeltaMark,
    },
    /// Flag when a single gap between consecutive timestamped lines exceeds a threshold.
    TimeGap {
        threshold_ms: i64,
        mark: TimeDeltaMark,
    },
    /// Flag when the count of a log level exceeds a threshold.
    LevelCount { level: LogLevel, threshold: usize },
    /// Flag when a pattern is NOT found in any log line.
    MissingPattern { regex: Regex },
    /// Flag when one or more expected patterns are absent.
    MissingAnyPattern {
        patterns: Vec<String>,
        regexes: Vec<Regex>,
    },
    /// Flag when a specific step duration exceeds a threshold.
    StepDuration {
        step: String,
        threshold_ms: i64,
        mark: TimeDeltaMark,
    },
}

/// Which lines to mark for a time-delta tip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeDeltaMark {
    /// Mark the first and last timestamped lines.
    FirstLast,
    /// Mark only the last timestamped line.
    Last,
}

// ── Evaluation results ───────────────────────────────────────────────────────

/// Result of evaluating a tip against a log file.
#[derive(Debug, Clone)]
pub struct TipResult {
    pub tip: Tip,
    /// Whether the tip triggered (at least one match or condition met).
    pub triggered: bool,
    /// Detail string (e.g. "Elapsed: 6h 12m", "5 errors found").
    pub detail: String,
    /// Line numbers that should be marked with this tip's emoji.
    pub marked_lines: Vec<usize>,
}

// ── Loading ──────────────────────────────────────────────────────────────────

/// Load all tips from `.toml` files in the given directory.
///
/// Invalid files are logged as warnings and skipped.
pub fn load_tips(dir: &Path) -> Vec<Tip> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        warn!(dir = %dir.display(), "Tips directory not found — no tips loaded");
        return Vec::new();
    };

    let mut tips = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }

        match load_tip_file(&path) {
            Ok(tip) => {
                debug!(id = %tip.id, "Loaded tip");
                tips.push(tip);
            }
            Err(e) => {
                warn!(file = %path.display(), err = %e, "Skipping invalid tip file");
            }
        }
    }

    tips.sort_by(|a, b| a.id.cmp(&b.id));
    tips
}

/// Parse and validate a single tip TOML file.
fn load_tip_file(path: &Path) -> anyhow::Result<Tip> {
    let content = std::fs::read_to_string(path)?;
    let raw: TipToml = toml::from_str(&content)?;
    parse_tip_schema_version(raw.schema_version)?;
    let scope = parse_tip_scope(raw.check.scope.as_deref())?;
    let applies_to = parse_tip_applies_to(raw.check.applies_to.as_deref())?;
    let enabled = raw.enabled.unwrap_or(true);

    let check =
        match raw.check.r#type.as_str() {
            "pattern_match" => {
                let pat = raw
                    .check
                    .pattern
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("pattern_match requires 'pattern'"))?;
                let regex = Regex::new(pat)?;
                Check::PatternMatch { regex }
            }
            "contains_any_patterns" => {
                let pats =
                    raw.check.patterns.as_ref().ok_or_else(|| {
                        anyhow::anyhow!("contains_any_patterns requires 'patterns'")
                    })?;
                if pats.is_empty() {
                    anyhow::bail!("contains_any_patterns requires at least one pattern");
                }
                let regexes: anyhow::Result<Vec<Regex>> = pats
                    .iter()
                    .map(|p| Regex::new(p).map_err(Into::into))
                    .collect();
                Check::ContainsAnyPatterns { regexes: regexes? }
            }
            "time_delta" => {
                let secs = raw
                    .check
                    .threshold_secs
                    .ok_or_else(|| anyhow::anyhow!("time_delta requires 'threshold_secs'"))?;
                let mark = match raw.check.mark.as_deref() {
                    Some("first_last") => TimeDeltaMark::FirstLast,
                    Some("last") | None => TimeDeltaMark::Last,
                    Some(other) => anyhow::bail!("Unknown mark type: {other}"),
                };
                Check::TimeDelta {
                    threshold_ms: secs as i64 * 1000,
                    mark,
                }
            }
            "time_gap" => {
                let secs = raw
                    .check
                    .threshold_secs
                    .ok_or_else(|| anyhow::anyhow!("time_gap requires 'threshold_secs'"))?;
                let mark = match raw.check.mark.as_deref() {
                    Some("first_last") => TimeDeltaMark::FirstLast,
                    Some("last") | None => TimeDeltaMark::Last,
                    Some(other) => anyhow::bail!("Unknown mark type: {other}"),
                };
                Check::TimeGap {
                    threshold_ms: secs as i64 * 1000,
                    mark,
                }
            }
            "step_duration" => {
                let step = raw
                    .check
                    .step
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("step_duration requires 'step'"))?
                    .trim()
                    .to_string();
                if step.is_empty() {
                    anyhow::bail!("step_duration requires non-empty 'step'");
                }
                let secs = raw
                    .check
                    .threshold_secs
                    .ok_or_else(|| anyhow::anyhow!("step_duration requires 'threshold_secs'"))?;
                let mark = match raw.check.mark.as_deref() {
                    Some("first_last") => TimeDeltaMark::FirstLast,
                    Some("last") | None => TimeDeltaMark::Last,
                    Some(other) => anyhow::bail!("Unknown mark type: {other}"),
                };
                Check::StepDuration {
                    step,
                    threshold_ms: secs as i64 * 1000,
                    mark,
                }
            }
            "level_count" => {
                let level_str = raw
                    .check
                    .level
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("level_count requires 'level'"))?;
                let level = parse_log_level(level_str)?;
                let threshold = raw
                    .check
                    .threshold
                    .ok_or_else(|| anyhow::anyhow!("level_count requires 'threshold'"))?;
                Check::LevelCount { level, threshold }
            }
            "missing_pattern" => {
                let pat = raw
                    .check
                    .pattern
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("missing_pattern requires 'pattern'"))?;
                let regex = Regex::new(pat)?;
                Check::MissingPattern { regex }
            }
            "missing_any_pattern" => {
                let pats =
                    raw.check.patterns.as_ref().ok_or_else(|| {
                        anyhow::anyhow!("missing_any_pattern requires 'patterns'")
                    })?;
                if pats.is_empty() {
                    anyhow::bail!("missing_any_pattern requires at least one pattern");
                }
                let regexes: anyhow::Result<Vec<Regex>> = pats
                    .iter()
                    .map(|p| Regex::new(p).map_err(Into::into))
                    .collect();
                Check::MissingAnyPattern {
                    patterns: pats.clone(),
                    regexes: regexes?,
                }
            }
            other => anyhow::bail!("Unknown check type: {other}"),
        };

    Ok(Tip {
        id: raw.id,
        name: raw.name,
        emoji: raw.emoji,
        docs: raw.docs,
        description: raw.description,
        enabled,
        scope,
        applies_to,
        check,
    })
}

// ── Raw TOML loading for admin ───────────────────────────────────────────────

/// A lightweight representation of a tip for display in the admin UI.
/// Unlike `Tip`, this does not compile regexes — it keeps the raw TOML fields.
#[derive(Debug, Clone)]
pub struct TipSummary {
    /// Filename stem for this tip source file (without .toml extension).
    pub source_id: String,
    pub id: String,
    pub name: String,
    pub emoji: String,
    pub docs: Option<String>,
    pub description: Option<String>,
    pub enabled: bool,
    pub check_type: String,
    pub scope: Option<String>,
    pub applies_to: Option<String>,
    pub pattern: Option<String>,
    pub patterns: Option<String>,
    pub step: Option<String>,
    pub threshold_secs: Option<u64>,
    pub mark: Option<String>,
    pub level: Option<String>,
    pub threshold: Option<usize>,
    /// Date the tip was first created (YYYY-MM-DD).
    pub created: Option<String>,
    /// Date the tip was last updated (YYYY-MM-DD).
    pub updated: Option<String>,
    /// Non-empty if the tip file had a parse/validation error.
    pub error: Option<String>,
}

/// Load raw tip summaries for the admin UI (does not validate regexes strictly).
pub fn load_tip_summaries(dir: &Path) -> Vec<TipSummary> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut summaries = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }

        let source_id = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                summaries.push(TipSummary {
                    source_id: source_id.clone(),
                    id: source_id,
                    name: String::new(),
                    emoji: String::new(),
                    docs: None,
                    description: None,
                    enabled: true,
                    check_type: String::new(),
                    scope: None,
                    applies_to: None,
                    pattern: None,
                    patterns: None,
                    step: None,
                    threshold_secs: None,
                    mark: None,
                    level: None,
                    threshold: None,
                    created: None,
                    updated: None,
                    error: Some(format!("Read error: {e}")),
                });
                continue;
            }
        };

        match toml::from_str::<TipToml>(&content) {
            Ok(raw) => {
                summaries.push(TipSummary {
                    source_id,
                    id: raw.id,
                    name: raw.name,
                    emoji: raw.emoji,
                    docs: raw.docs,
                    description: raw.description,
                    enabled: raw.enabled.unwrap_or(true),
                    check_type: raw.check.r#type,
                    scope: raw.check.scope,
                    applies_to: raw.check.applies_to,
                    pattern: raw.check.pattern,
                    patterns: raw.check.patterns.map(|patterns| patterns.join("\n")),
                    step: raw.check.step,
                    threshold_secs: raw.check.threshold_secs,
                    mark: raw.check.mark,
                    level: raw.check.level,
                    threshold: raw.check.threshold,
                    created: raw.created,
                    updated: raw.updated,
                    error: None,
                });
            }
            Err(e) => {
                summaries.push(TipSummary {
                    source_id: source_id.clone(),
                    id: source_id,
                    name: String::new(),
                    emoji: String::new(),
                    docs: None,
                    description: None,
                    enabled: true,
                    check_type: String::new(),
                    scope: None,
                    applies_to: None,
                    pattern: None,
                    patterns: None,
                    step: None,
                    threshold_secs: None,
                    mark: None,
                    level: None,
                    threshold: None,
                    created: None,
                    updated: None,
                    error: Some(format!("Parse error: {e}")),
                });
            }
        }
    }

    summaries.sort_by(|a, b| a.id.cmp(&b.id));
    summaries
}

/// Input data for creating or updating a tip.
pub struct SaveTipInput<'a> {
    pub dir: &'a Path,
    pub id: &'a str,
    pub source_id: Option<&'a str>,
    pub name: &'a str,
    pub emoji: &'a str,
    pub enabled: bool,
    pub docs: Option<&'a str>,
    pub description: Option<&'a str>,
    pub check_type: &'a str,
    pub scope: Option<&'a str>,
    pub applies_to: Option<&'a str>,
    pub pattern: Option<&'a str>,
    pub patterns: Option<Vec<String>>,
    pub step: Option<&'a str>,
    pub threshold_secs: Option<u64>,
    pub mark: Option<&'a str>,
    pub level: Option<&'a str>,
    pub threshold: Option<usize>,
}

/// Save a tip to a TOML file. The filename is derived from the tip ID.
/// Returns the validated `Tip` on success.
pub fn save_tip(input: SaveTipInput<'_>) -> anyhow::Result<()> {
    let id = input.id;
    // Sanitize ID for filename safety
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        anyhow::bail!(
            "Tip ID must be non-empty and contain only alphanumeric, dash, or underscore characters"
        );
    }

    let source_id = input
        .source_id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    if let Some(source) = &source_id
        && !source
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        anyhow::bail!("Invalid source tip ID");
    }

    let canonical_path = input.dir.join(format!("{id}.toml"));
    let source_path = source_id
        .as_deref()
        .map(|source| input.dir.join(format!("{source}.toml")));
    let write_path = match source_path.as_ref() {
        Some(path) if path.exists() => path.clone(),
        _ => canonical_path.clone(),
    };

    // Determine created/updated dates
    let today = today_date_string();
    let created = if write_path.exists() {
        // Preserve the original created date if the file already exists
        std::fs::read_to_string(&write_path)
            .ok()
            .and_then(|c| toml::from_str::<TipToml>(&c).ok())
            .and_then(|t| t.created)
            .unwrap_or_else(|| today.clone())
    } else {
        today.clone()
    };

    let raw = TipToml {
        schema_version: Some(CURRENT_TIP_SCHEMA_VERSION),
        id: id.to_string(),
        name: input.name.to_string(),
        emoji: input.emoji.to_string(),
        enabled: Some(input.enabled),
        docs: input.docs.filter(|s| !s.is_empty()).map(|s| s.to_string()),
        description: input
            .description
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
        created: Some(created),
        updated: Some(today),
        check: CheckToml {
            r#type: input.check_type.to_string(),
            scope: input.scope.filter(|s| !s.is_empty()).map(|s| s.to_string()),
            applies_to: input
                .applies_to
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string()),
            pattern: input
                .pattern
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string()),
            patterns: input.patterns.filter(|p| !p.is_empty()),
            threshold_secs: input.threshold_secs,
            step: input.step.filter(|s| !s.is_empty()).map(|s| s.to_string()),
            mark: input.mark.filter(|s| !s.is_empty()).map(|s| s.to_string()),
            level: input.level.filter(|s| !s.is_empty()).map(|s| s.to_string()),
            threshold: input.threshold,
        },
    };

    // Validate by trying to load it (compiles regex, checks required fields)
    let toml_str = toml::to_string_pretty(&raw)?;
    let _: TipToml = toml::from_str(&toml_str)?;

    // Also validate the check logic (e.g. compile regex)
    let dir = input.dir;
    let temp_path = dir.join("__validate_temp__.toml");
    std::fs::write(&temp_path, &toml_str)?;
    let result = load_tip_file(&temp_path);
    let _ = std::fs::remove_file(&temp_path);
    result?;

    // Write the actual file
    std::fs::write(&write_path, toml_str)?;

    Ok(())
}

/// Delete a tip TOML file by ID. Returns an error if not found.
pub fn delete_tip(dir: &Path, id: &str) -> anyhow::Result<()> {
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        anyhow::bail!("Invalid tip ID");
    }
    let path = dir.join(format!("{id}.toml"));
    if !path.exists() {
        anyhow::bail!("Tip '{id}' not found");
    }
    std::fs::remove_file(&path)?;
    Ok(())
}

/// Map a string log level name to a `LogLevel`.
fn parse_log_level(s: &str) -> anyhow::Result<LogLevel> {
    match s.to_lowercase().as_str() {
        "info" => Ok(LogLevel::Info),
        "debug" => Ok(LogLevel::Debug),
        "warning" | "warn" => Ok(LogLevel::Warning),
        "error" | "err" => Ok(LogLevel::Error),
        "notice" => Ok(LogLevel::Notice),
        "group" => Ok(LogLevel::Group),
        "command" => Ok(LogLevel::Command),
        _ => Err(anyhow::anyhow!("Unknown log level: {s}")),
    }
}

fn parse_tip_scope(scope: Option<&str>) -> anyhow::Result<TipScope> {
    match scope.unwrap_or("all").to_lowercase().as_str() {
        "all" => Ok(TipScope::All),
        "workflow" => Ok(TipScope::Workflow),
        "runner" => Ok(TipScope::Runner),
        other => Err(anyhow::anyhow!("Unknown tip scope: {other}")),
    }
}

fn parse_tip_applies_to(applies_to: Option<&str>) -> anyhow::Result<TipAppliesTo> {
    match applies_to.unwrap_or("all").to_lowercase().as_str() {
        "all" => Ok(TipAppliesTo::All),
        "standard_logs" => Ok(TipAppliesTo::StandardLogs),
        "debug_logs_enabled" => Ok(TipAppliesTo::DebugLogsEnabled),
        "diagnostic_logs_enabled" => Ok(TipAppliesTo::DiagnosticLogsEnabled),
        other => Err(anyhow::anyhow!("Unknown tip applies_to: {other}")),
    }
}

fn parse_tip_schema_version(raw_version: Option<u32>) -> anyhow::Result<u32> {
    let version = raw_version.unwrap_or(CURRENT_TIP_SCHEMA_VERSION);
    if version != CURRENT_TIP_SCHEMA_VERSION {
        anyhow::bail!(
            "Unsupported tip schema_version: {version} (supported: {CURRENT_TIP_SCHEMA_VERSION})"
        );
    }
    Ok(version)
}

// ── Evaluation ───────────────────────────────────────────────────────────────

/// Evaluate all tips against a set of log entries.
pub fn evaluate_tips_for_log(
    tips: &[Tip],
    entries: &[LogEntry],
    log_kind: LogKind,
) -> Vec<TipResult> {
    let has_debug_lines = entries.iter().any(|e| e.level == LogLevel::Debug);

    tips.iter()
        .filter(|tip| tip.enabled)
        .filter(|tip| tip.scope.matches(log_kind))
        .filter(|tip| tip.applies_to.matches(log_kind, has_debug_lines))
        .map(|tip| evaluate_tip(tip, entries))
        .collect()
}

/// Evaluate a single tip against the log entries.
fn evaluate_tip(tip: &Tip, entries: &[LogEntry]) -> TipResult {
    match &tip.check {
        Check::PatternMatch { regex } => eval_pattern_match(tip, entries, regex),
        Check::ContainsAnyPatterns { regexes } => eval_contains_any_patterns(tip, entries, regexes),
        Check::TimeDelta { threshold_ms, mark } => {
            eval_time_delta(tip, entries, *threshold_ms, *mark)
        }
        Check::TimeGap { threshold_ms, mark } => eval_time_gap(tip, entries, *threshold_ms, *mark),
        Check::LevelCount { level, threshold } => eval_level_count(tip, entries, level, *threshold),
        Check::MissingPattern { regex } => eval_missing_pattern(tip, entries, regex),
        Check::MissingAnyPattern { patterns, regexes } => {
            eval_missing_any_pattern(tip, entries, patterns, regexes)
        }
        Check::StepDuration {
            step,
            threshold_ms,
            mark,
        } => eval_step_duration(tip, entries, step, *threshold_ms, *mark),
    }
}

fn eval_pattern_match(tip: &Tip, entries: &[LogEntry], regex: &Regex) -> TipResult {
    let marked_lines: Vec<usize> = entries
        .iter()
        .filter(|e| regex.is_match(&e.message))
        .map(|e| e.line_number)
        .collect();

    let triggered = !marked_lines.is_empty();
    let detail = if triggered {
        format!("{} matching line(s)", marked_lines.len())
    } else {
        String::new()
    };

    TipResult {
        tip: tip.clone(),
        triggered,
        detail,
        marked_lines,
    }
}

fn eval_contains_any_patterns(tip: &Tip, entries: &[LogEntry], regexes: &[Regex]) -> TipResult {
    let marked_lines: Vec<usize> = entries
        .iter()
        .filter(|e| regexes.iter().any(|regex| regex.is_match(&e.message)))
        .map(|e| e.line_number)
        .collect();

    let triggered = !marked_lines.is_empty();
    let detail = if triggered {
        format!(
            "{} matching line(s) across {} pattern(s)",
            marked_lines.len(),
            regexes.len()
        )
    } else {
        String::new()
    };

    TipResult {
        tip: tip.clone(),
        triggered,
        detail,
        marked_lines,
    }
}

fn eval_time_delta(
    tip: &Tip,
    entries: &[LogEntry],
    threshold_ms: i64,
    mark: TimeDeltaMark,
) -> TipResult {
    let timestamped: Vec<&LogEntry> = entries
        .iter()
        .filter(|e| e.epoch_millis.is_some())
        .collect();

    if timestamped.len() < 2 {
        return TipResult {
            tip: tip.clone(),
            triggered: false,
            detail: String::new(),
            marked_lines: Vec::new(),
        };
    }

    let first = timestamped.first().unwrap();
    let last = timestamped.last().unwrap();
    let first_epoch = first.epoch_millis.unwrap();
    let last_epoch = last.epoch_millis.unwrap();
    let delta_ms = last_epoch - first_epoch;

    let triggered = delta_ms > threshold_ms;
    let crossing_line = first_line_exceeding_threshold(&timestamped, first_epoch, threshold_ms)
        .unwrap_or(last.line_number);

    let marked_lines = if triggered {
        match mark {
            TimeDeltaMark::FirstLast => vec![first.line_number, last.line_number],
            TimeDeltaMark::Last => vec![crossing_line],
        }
    } else {
        Vec::new()
    };

    let detail = if triggered {
        format_duration_detail(delta_ms)
    } else {
        String::new()
    };

    TipResult {
        tip: tip.clone(),
        triggered,
        detail,
        marked_lines,
    }
}

fn first_line_exceeding_threshold(
    timestamped: &[&LogEntry],
    first_epoch: i64,
    threshold_ms: i64,
) -> Option<usize> {
    timestamped.iter().find_map(|entry| {
        let epoch = entry.epoch_millis?;
        if epoch - first_epoch > threshold_ms {
            Some(entry.line_number)
        } else {
            None
        }
    })
}

fn eval_level_count(
    tip: &Tip,
    entries: &[LogEntry],
    level: &LogLevel,
    threshold: usize,
) -> TipResult {
    let count = entries.iter().filter(|e| &e.level == level).count();
    let triggered = count > threshold;

    let detail = if triggered {
        format!("{count} {level} line(s) (threshold: {threshold})")
    } else {
        String::new()
    };

    // For level_count, mark all lines of that level
    let marked_lines = if triggered {
        entries
            .iter()
            .filter(|e| &e.level == level)
            .map(|e| e.line_number)
            .collect()
    } else {
        Vec::new()
    };

    TipResult {
        tip: tip.clone(),
        triggered,
        detail,
        marked_lines,
    }
}

fn eval_time_gap(
    tip: &Tip,
    entries: &[LogEntry],
    threshold_ms: i64,
    mark: TimeDeltaMark,
) -> TipResult {
    let timestamped: Vec<&LogEntry> = entries
        .iter()
        .filter(|e| e.epoch_millis.is_some())
        .collect();

    if timestamped.len() < 2 {
        return TipResult {
            tip: tip.clone(),
            triggered: false,
            detail: String::new(),
            marked_lines: Vec::new(),
        };
    }

    let mut max_gap_ms = i64::MIN;
    let mut gap_start_line = 0usize;
    let mut gap_end_line = 0usize;

    for pair in timestamped.windows(2) {
        let start = pair[0];
        let end = pair[1];
        let start_epoch = start.epoch_millis.unwrap();
        let end_epoch = end.epoch_millis.unwrap();
        let gap_ms = end_epoch - start_epoch;
        if gap_ms > max_gap_ms {
            max_gap_ms = gap_ms;
            gap_start_line = start.line_number;
            gap_end_line = end.line_number;
        }
    }

    let triggered = max_gap_ms > threshold_ms;

    let marked_lines = if triggered {
        match mark {
            TimeDeltaMark::FirstLast => vec![gap_start_line, gap_end_line],
            TimeDeltaMark::Last => vec![gap_end_line],
        }
    } else {
        Vec::new()
    };

    let detail = if triggered {
        format!(
            "Largest gap: {} (between lines {} and {})",
            format_duration_detail(max_gap_ms),
            gap_start_line,
            gap_end_line
        )
    } else {
        String::new()
    };

    TipResult {
        tip: tip.clone(),
        triggered,
        detail,
        marked_lines,
    }
}

fn eval_missing_pattern(tip: &Tip, entries: &[LogEntry], regex: &Regex) -> TipResult {
    let found = entries.iter().any(|e| regex.is_match(&e.message));
    let triggered = !found;

    let detail = if triggered {
        "Expected pattern not found in log".to_string()
    } else {
        String::new()
    };

    // No specific lines to mark for a missing pattern
    TipResult {
        tip: tip.clone(),
        triggered,
        detail,
        marked_lines: Vec::new(),
    }
}

fn eval_missing_any_pattern(
    tip: &Tip,
    entries: &[LogEntry],
    patterns: &[String],
    regexes: &[Regex],
) -> TipResult {
    let missing: Vec<&str> = patterns
        .iter()
        .zip(regexes.iter())
        .filter_map(|(pattern, regex)| {
            let found = entries.iter().any(|e| regex.is_match(&e.message));
            if found { None } else { Some(pattern.as_str()) }
        })
        .collect();

    let triggered = !missing.is_empty();
    let detail = if triggered {
        format!("Missing expected patterns: {}", missing.join(", "))
    } else {
        String::new()
    };

    TipResult {
        tip: tip.clone(),
        triggered,
        detail,
        marked_lines: Vec::new(),
    }
}

fn eval_step_duration(
    tip: &Tip,
    entries: &[LogEntry],
    step: &str,
    threshold_ms: i64,
    mark: TimeDeltaMark,
) -> TipResult {
    let start_marker = format!("Starting: {step}");
    let finish_marker = format!("Finishing: {step}");

    let start = entries
        .iter()
        .find(|e| e.epoch_millis.is_some() && e.message.contains(&start_marker));
    let Some(start) = start else {
        return TipResult {
            tip: tip.clone(),
            triggered: false,
            detail: String::new(),
            marked_lines: Vec::new(),
        };
    };

    let finish = entries.iter().find(|e| {
        e.epoch_millis.is_some()
            && e.line_number > start.line_number
            && e.message.contains(&finish_marker)
    });
    let Some(finish) = finish else {
        return TipResult {
            tip: tip.clone(),
            triggered: false,
            detail: String::new(),
            marked_lines: Vec::new(),
        };
    };

    let delta_ms = finish.epoch_millis.unwrap() - start.epoch_millis.unwrap();
    let triggered = delta_ms > threshold_ms;

    let marked_lines = if triggered {
        match mark {
            TimeDeltaMark::FirstLast => vec![start.line_number, finish.line_number],
            TimeDeltaMark::Last => vec![finish.line_number],
        }
    } else {
        Vec::new()
    };

    let detail = if triggered {
        format!(
            "Step '{step}' duration: {}",
            format_duration_detail(delta_ms)
        )
    } else {
        String::new()
    };

    TipResult {
        tip: tip.clone(),
        triggered,
        detail,
        marked_lines,
    }
}

/// Format a millisecond duration as "Xh Ym Zs".
fn format_duration_detail(ms: i64) -> String {
    let total_secs = ms / 1000;
    let hours = total_secs / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;

    if hours > 0 {
        format!("Elapsed: {hours}h {mins}m {secs}s")
    } else if mins > 0 {
        format!("Elapsed: {mins}m {secs}s")
    } else {
        format!("Elapsed: {secs}s")
    }
}

/// Return today's date as a "YYYY-MM-DD" string (UTC).
fn today_date_string() -> String {
    // Seconds since Unix epoch → calendar date (UTC)
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = secs / 86400;
    // Algorithm: civil_from_days (Howard Hinnant)
    let z = days as i64 + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::log_parser;

    fn sample_entries() -> Vec<LogEntry> {
        log_parser::parse_workflow_log(
            "2025-04-30T00:00:00Z ##[debug]Starting\n\
             2025-04-30T00:00:01Z Normal line\n\
             2025-04-30T00:00:02Z ##[error]Something broke\n\
             2025-04-30T00:00:03Z ##[warning]Watch out\n\
             2025-04-30T00:00:04Z Finishing: job",
        )
    }

    #[test]
    fn test_pattern_match() {
        let tip = Tip {
            id: "test".into(),
            name: "Test".into(),
            emoji: "🔍".into(),
            docs: None,
            description: None,
            scope: TipScope::All,
            applies_to: TipAppliesTo::All,
            enabled: true,
            check: Check::PatternMatch {
                regex: Regex::new("Something broke").unwrap(),
            },
        };
        let result = evaluate_tip(&tip, &sample_entries());
        assert!(result.triggered);
        assert_eq!(result.marked_lines, vec![3]);
    }

    #[test]
    fn test_contains_any_patterns() {
        let tip = Tip {
            id: "multi".into(),
            name: "Multi".into(),
            emoji: "🔎".into(),
            docs: None,
            description: None,
            scope: TipScope::All,
            applies_to: TipAppliesTo::All,
            enabled: true,
            check: Check::ContainsAnyPatterns {
                regexes: vec![
                    Regex::new("Something broke").unwrap(),
                    Regex::new("Watch out").unwrap(),
                ],
            },
        };
        let result = evaluate_tip(&tip, &sample_entries());
        assert!(result.triggered);
        assert_eq!(result.marked_lines, vec![3, 4]);
    }

    #[test]
    fn test_time_delta_not_triggered() {
        let tip = Tip {
            id: "timeout".into(),
            name: "Timeout".into(),
            emoji: "⏱️".into(),
            docs: None,
            description: None,
            scope: TipScope::All,
            applies_to: TipAppliesTo::All,
            enabled: true,
            check: Check::TimeDelta {
                threshold_ms: 21_600_000, // 6 hours
                mark: TimeDeltaMark::Last,
            },
        };
        let result = evaluate_tip(&tip, &sample_entries());
        assert!(!result.triggered);
    }

    #[test]
    fn test_time_delta_triggered() {
        let tip = Tip {
            id: "timeout".into(),
            name: "Timeout".into(),
            emoji: "⏱️".into(),
            docs: None,
            description: None,
            scope: TipScope::All,
            applies_to: TipAppliesTo::All,
            enabled: true,
            check: Check::TimeDelta {
                threshold_ms: 2_000, // 2 seconds
                mark: TimeDeltaMark::FirstLast,
            },
        };
        let result = evaluate_tip(&tip, &sample_entries());
        assert!(result.triggered);
        assert_eq!(result.marked_lines, vec![1, 5]);
        assert!(result.detail.contains("Elapsed:"));
    }

    #[test]
    fn test_time_delta_last_marks_threshold_crossing_line() {
        let tip = Tip {
            id: "timeout".into(),
            name: "Timeout".into(),
            emoji: "⏱️".into(),
            docs: None,
            description: None,
            scope: TipScope::All,
            applies_to: TipAppliesTo::All,
            enabled: true,
            check: Check::TimeDelta {
                threshold_ms: 2_000, // 2 seconds
                mark: TimeDeltaMark::Last,
            },
        };

        let result = evaluate_tip(&tip, &sample_entries());
        assert!(result.triggered);
        assert_eq!(result.marked_lines, vec![4]);
    }

    #[test]
    fn test_time_gap_triggered_marks_gap_end_line() {
        let tip = Tip {
            id: "gap".into(),
            name: "Gap".into(),
            emoji: "⏳".into(),
            docs: None,
            description: None,
            scope: TipScope::All,
            applies_to: TipAppliesTo::All,
            enabled: true,
            check: Check::TimeGap {
                threshold_ms: 500,
                mark: TimeDeltaMark::Last,
            },
        };

        let result = evaluate_tip(&tip, &sample_entries());
        assert!(result.triggered);
        assert_eq!(result.marked_lines, vec![2]);
        assert!(result.detail.contains("Largest gap:"));
    }

    #[test]
    fn test_time_gap_not_triggered() {
        let tip = Tip {
            id: "gap".into(),
            name: "Gap".into(),
            emoji: "⏳".into(),
            docs: None,
            description: None,
            scope: TipScope::All,
            applies_to: TipAppliesTo::All,
            enabled: true,
            check: Check::TimeGap {
                threshold_ms: 5_000,
                mark: TimeDeltaMark::Last,
            },
        };

        let result = evaluate_tip(&tip, &sample_entries());
        assert!(!result.triggered);
    }

    #[test]
    fn test_level_count_not_triggered() {
        let tip = Tip {
            id: "errs".into(),
            name: "Errors".into(),
            emoji: "🚨".into(),
            docs: None,
            description: None,
            scope: TipScope::All,
            applies_to: TipAppliesTo::All,
            enabled: true,
            check: Check::LevelCount {
                level: LogLevel::Error,
                threshold: 5,
            },
        };
        let result = evaluate_tip(&tip, &sample_entries());
        assert!(!result.triggered);
    }

    #[test]
    fn test_level_count_triggered() {
        let tip = Tip {
            id: "debugs".into(),
            name: "Debug".into(),
            emoji: "🐛".into(),
            docs: None,
            description: None,
            scope: TipScope::All,
            applies_to: TipAppliesTo::All,
            enabled: true,
            check: Check::LevelCount {
                level: LogLevel::Debug,
                threshold: 0,
            },
        };
        let result = evaluate_tip(&tip, &sample_entries());
        assert!(result.triggered);
        assert_eq!(result.marked_lines, vec![1]);
    }

    #[test]
    fn test_missing_pattern_triggered() {
        let tip = Tip {
            id: "missing".into(),
            name: "Missing".into(),
            emoji: "❓".into(),
            docs: None,
            description: None,
            scope: TipScope::All,
            applies_to: TipAppliesTo::All,
            enabled: true,
            check: Check::MissingPattern {
                regex: Regex::new("Success!").unwrap(),
            },
        };
        let result = evaluate_tip(&tip, &sample_entries());
        assert!(result.triggered);
    }

    #[test]
    fn test_missing_pattern_not_triggered() {
        let tip = Tip {
            id: "missing".into(),
            name: "Missing".into(),
            emoji: "❓".into(),
            docs: None,
            description: None,
            scope: TipScope::All,
            applies_to: TipAppliesTo::All,
            enabled: true,
            check: Check::MissingPattern {
                regex: Regex::new("Finishing:").unwrap(),
            },
        };
        let result = evaluate_tip(&tip, &sample_entries());
        assert!(!result.triggered);
    }

    #[test]
    fn test_missing_any_pattern_triggered() {
        let tip = Tip {
            id: "missing-any".into(),
            name: "Missing Any".into(),
            emoji: "❓".into(),
            docs: None,
            description: None,
            scope: TipScope::All,
            applies_to: TipAppliesTo::All,
            enabled: true,
            check: Check::MissingAnyPattern {
                patterns: vec!["Finishing:".into(), "Success!".into()],
                regexes: vec![
                    Regex::new("Finishing:").unwrap(),
                    Regex::new("Success!").unwrap(),
                ],
            },
        };
        let result = evaluate_tip(&tip, &sample_entries());
        assert!(result.triggered);
        assert!(result.detail.contains("Success!"));
    }

    #[test]
    fn test_step_duration_triggered() {
        let entries = log_parser::parse_workflow_log(
            "2025-04-30T00:00:00Z ##[debug]Starting: Deploy\n\
             2025-04-30T00:00:05Z Midpoint\n\
             2025-04-30T00:00:10Z ##[debug]Finishing: Deploy",
        );

        let tip = Tip {
            id: "step".into(),
            name: "Step".into(),
            emoji: "⏱️".into(),
            docs: None,
            description: None,
            scope: TipScope::All,
            applies_to: TipAppliesTo::All,
            enabled: true,
            check: Check::StepDuration {
                step: "Deploy".into(),
                threshold_ms: 4_000,
                mark: TimeDeltaMark::Last,
            },
        };

        let result = evaluate_tip(&tip, &entries);
        assert!(result.triggered);
        assert_eq!(result.marked_lines, vec![3]);
    }

    #[test]
    fn test_scope_filters_out_workflow_tip_on_runner_log() {
        let tip = Tip {
            id: "missing".into(),
            name: "Missing".into(),
            emoji: "❓".into(),
            docs: None,
            description: None,
            scope: TipScope::Workflow,
            applies_to: TipAppliesTo::All,
            enabled: true,
            check: Check::MissingPattern {
                regex: Regex::new("Finishing:").unwrap(),
            },
        };

        let results = evaluate_tips_for_log(&[tip], &sample_entries(), LogKind::Runner);
        assert!(results.is_empty());
    }

    #[test]
    fn test_load_tip_toml() {
        let toml_content = r#"
id = "test-tip"
name = "Test Tip"
emoji = "🔍"
docs = "https://example.com"
description = "A test tip."

[check]
type = "pattern_match"
pattern = "error"
"#;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.toml");
        std::fs::write(&path, toml_content).unwrap();

        let tip = load_tip_file(&path).unwrap();
        assert_eq!(tip.id, "test-tip");
        assert_eq!(tip.emoji, "🔍");
    }

    #[test]
    fn test_load_tip_toml_with_schema_version() {
        let toml_content = r#"
schema_version = 1
id = "test-tip"
name = "Test Tip"
emoji = "🔍"

[check]
type = "pattern_match"
pattern = "error"
"#;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_schema.toml");
        std::fs::write(&path, toml_content).unwrap();

        let tip = load_tip_file(&path).unwrap();
        assert_eq!(tip.id, "test-tip");
    }

    #[test]
    fn test_applies_to_debug_logs_enabled_filters_standard_logs() {
        let tip = Tip {
            id: "debug-only".into(),
            name: "Debug Only".into(),
            emoji: "🐛".into(),
            docs: None,
            description: None,
            scope: TipScope::Workflow,
            applies_to: TipAppliesTo::DebugLogsEnabled,
            enabled: true,
            check: Check::MissingPattern {
                regex: Regex::new("never-here").unwrap(),
            },
        };

        let standard_entries = log_parser::parse_workflow_log(
            "2025-04-30T00:00:00Z Starting\n2025-04-30T00:00:01Z Finishing",
        );

        let results = evaluate_tips_for_log(&[tip], &standard_entries, LogKind::Workflow);
        assert!(results.is_empty());
    }

    #[test]
    fn test_applies_to_diagnostic_logs_enabled_filters_workflow_logs() {
        let tip = Tip {
            id: "diag-only".into(),
            name: "Diag Only".into(),
            emoji: "🩺".into(),
            docs: None,
            description: None,
            scope: TipScope::All,
            applies_to: TipAppliesTo::DiagnosticLogsEnabled,
            enabled: true,
            check: Check::MissingPattern {
                regex: Regex::new("never-here").unwrap(),
            },
        };

        let results = evaluate_tips_for_log(&[tip], &sample_entries(), LogKind::Workflow);
        assert!(results.is_empty());
    }

    #[test]
    fn test_reject_unknown_applies_to() {
        let toml_content = r#"
id = "test-tip"
name = "Test Tip"
emoji = "🔍"

[check]
type = "pattern_match"
applies_to = "unknown"
pattern = "error"
"#;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad_applies_to.toml");
        std::fs::write(&path, toml_content).unwrap();

        let err = load_tip_file(&path).unwrap_err();
        assert!(err.to_string().contains("Unknown tip applies_to"));
    }

    #[test]
    fn test_disabled_tip_is_filtered_out() {
        let tip = Tip {
            id: "disabled".into(),
            name: "Disabled".into(),
            emoji: "⏸️".into(),
            docs: None,
            description: None,
            enabled: false,
            scope: TipScope::All,
            applies_to: TipAppliesTo::All,
            check: Check::MissingPattern {
                regex: Regex::new("never-here").unwrap(),
            },
        };

        let results = evaluate_tips_for_log(&[tip], &sample_entries(), LogKind::Workflow);
        assert!(results.is_empty());
    }

    #[test]
    fn test_reject_unsupported_tip_schema_version() {
        let toml_content = r#"
schema_version = 2
id = "test-tip"
name = "Test Tip"
emoji = "🔍"

[check]
type = "pattern_match"
pattern = "error"
"#;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad_schema.toml");
        std::fs::write(&path, toml_content).unwrap();

        let err = load_tip_file(&path).unwrap_err();
        assert!(err.to_string().contains("Unsupported tip schema_version"));
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration_detail(3_661_000), "Elapsed: 1h 1m 1s");
        assert_eq!(format_duration_detail(65_000), "Elapsed: 1m 5s");
        assert_eq!(format_duration_detail(4_000), "Elapsed: 4s");
    }
}
