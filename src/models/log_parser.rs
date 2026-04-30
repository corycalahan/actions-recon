use std::collections::HashSet;
use std::fmt;

use regex::Regex;

/// A single parsed line from a GitHub Actions log file.
#[derive(Debug, Clone)]
pub struct LogEntry {
    /// Line number in the original file (1-based).
    pub line_number: usize,
    /// ISO 8601 timestamp (if present).
    pub timestamp: Option<String>,
    /// Unix epoch in milliseconds (if timestamp is present).
    pub epoch_millis: Option<i64>,
    /// Log level / annotation type.
    pub level: LogLevel,
    /// The message content (without timestamp and annotation prefix).
    pub message: String,
    /// Whether this line opens a group (`##[group]`).
    pub group_start: bool,
    /// Whether this line closes a group (`##[endgroup]`).
    pub group_end: bool,
}

/// A GitHub Action reference extracted from log content.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ActionReference {
    pub owner: String,
    pub repo: String,
    pub path: Option<String>,
    pub reference: String,
    pub first_line: usize,
}

impl ActionReference {
    pub fn owner_repo(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }

    pub fn github_url(&self) -> String {
        format!("https://github.com/{}/{}", self.owner, self.repo)
    }
}

/// Log level extracted from line annotations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogLevel {
    /// Normal output (no annotation).
    Info,
    /// `##[debug]` or runner `DEBUG` level.
    Debug,
    /// `##[warning]` or runner `WARN` level.
    Warning,
    /// `##[error]` or runner `ERR` level.
    Error,
    /// `##[notice]` annotation.
    Notice,
    /// `##[group]` header line.
    Group,
    /// `##[command]` annotation.
    Command,
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LogLevel::Info => write!(f, "info"),
            LogLevel::Debug => write!(f, "debug"),
            LogLevel::Warning => write!(f, "warning"),
            LogLevel::Error => write!(f, "error"),
            LogLevel::Notice => write!(f, "notice"),
            LogLevel::Group => write!(f, "group"),
            LogLevel::Command => write!(f, "command"),
        }
    }
}

impl LogLevel {
    /// CSS class name for styling this level in the timeline.
    pub fn css_class(&self) -> &'static str {
        match self {
            LogLevel::Info => "level-info",
            LogLevel::Debug => "level-debug",
            LogLevel::Warning => "level-warning",
            LogLevel::Error => "level-error",
            LogLevel::Notice => "level-notice",
            LogLevel::Group => "level-group",
            LogLevel::Command => "level-command",
        }
    }
}

/// Parse all lines from a GitHub Actions workflow log file.
///
/// Handles the format: `2025-04-30T00:53:42.8434646Z ##[debug]message`
pub fn parse_workflow_log(content: &str) -> Vec<LogEntry> {
    // GitHub Actions log files often start with a UTF-8 BOM (EF BB BF). If
    // present on line 1, it shifts the timestamp position and prevents the
    // timestamp + message parser from recognizing the line. Strip it once at
    // the start of the content so all downstream parsing works uniformly.
    let content = content.strip_prefix('\u{FEFF}').unwrap_or(content);
    content
        .lines()
        .enumerate()
        .map(|(i, line)| parse_workflow_line(i + 1, line))
        .collect()
}

/// Parse all lines from a runner diagnostic log file.
///
/// Handles the format: `[2025-04-30 00:53:40Z INFO HostContext] message`
pub fn parse_runner_log(content: &str) -> Vec<LogEntry> {
    let content = content.strip_prefix('\u{FEFF}').unwrap_or(content);
    content
        .lines()
        .enumerate()
        .map(|(i, line)| parse_runner_line(i + 1, line))
        .collect()
}

/// Extract referenced GitHub Actions from parsed log entries.
///
/// Supports references like:
/// - `actions/checkout@v4`
/// - `github/codeql-action/init@v3`
/// - `actions/checkout@08c6903...`
/// - `octo-org/shared/.github/workflows/build.yml@v1` (reusable workflow)
///
/// Skips matches inside `Job defined at:` lines, which point to the workflow
/// file that defines the current job (metadata about the run itself, not a
/// dependency the workflow uses). Also skips warning/error/notice entries,
/// where action refs typically appear inside prose (e.g. deprecation
/// warnings) and produce false positives like `v4.` from sentence-ending
/// punctuation. Trailing punctuation on captured refs is trimmed as a
/// secondary safeguard.
pub fn extract_action_references(entries: &[LogEntry]) -> Vec<ActionReference> {
    let regex = Regex::new(
        r"(?P<owner>[A-Za-z0-9_.-]+)/(?P<repo>[A-Za-z0-9_.-]+)(?P<path>/[A-Za-z0-9_.\-/]+)?@(?P<reference>[A-Za-z0-9_.\-/]+)",
    )
    .expect("action reference regex should compile");

    let mut seen: HashSet<(String, String, Option<String>, String)> = HashSet::new();
    let mut references = Vec::new();

    for entry in entries {
        // Skip "Job defined at: <owner/repo/.../workflow.yml@ref>" — that's the
        // location of the workflow file itself, not an action it calls.
        if entry.message.starts_with("Job defined at:") {
            continue;
        }
        // Skip warnings/errors/notices: these carry prose that frequently
        // mentions action refs in a sentence (e.g. "...actions/checkout@v4.
        // Actions will be forced..."), which leads to bogus captures like
        // "v4." with trailing punctuation. Real action references appear in
        // info-level lines such as "Run actions/checkout@v4" and
        // "Download action repository '...@<ref>'".
        if matches!(
            entry.level,
            LogLevel::Warning | LogLevel::Error | LogLevel::Notice
        ) {
            continue;
        }
        for captures in regex.captures_iter(&entry.message) {
            let owner = captures["owner"].to_string();
            let repo = captures["repo"].to_string();
            let path = captures.name("path").map(|m| m.as_str().to_string());
            // Strip trailing punctuation that can leak in when the regex
            // matches inside prose (period, comma, semicolon, colon, closing
            // brackets/quotes). Real refs never end in these characters.
            let reference = captures["reference"]
                .trim_end_matches(|c: char| {
                    matches!(c, '.' | ',' | ';' | ':' | ')' | ']' | '"' | '\'')
                })
                .to_string();
            if reference.is_empty() {
                continue;
            }

            let key = (owner.clone(), repo.clone(), path.clone(), reference.clone());
            if !seen.insert(key) {
                continue;
            }

            references.push(ActionReference {
                owner,
                repo,
                path,
                reference,
                first_line: entry.line_number,
            });
        }
    }

    references.sort_by_key(|r| r.first_line);
    references
}

/// Parse a single workflow log line.
fn parse_workflow_line(line_number: usize, line: &str) -> LogEntry {
    let (timestamp, rest) = extract_workflow_timestamp(line);

    // Check for annotations: ##[type]
    let (level, message, group_start, group_end) = parse_annotation(rest);

    let epoch_millis = timestamp.as_deref().and_then(iso8601_to_epoch_millis);

    LogEntry {
        line_number,
        timestamp,
        epoch_millis,
        level,
        message,
        group_start,
        group_end,
    }
}

/// Extract the ISO 8601 timestamp from a workflow log line.
///
/// Format: `2025-04-30T00:53:42.8434646Z ` (with trailing space)
fn extract_workflow_timestamp(line: &str) -> (Option<String>, &str) {
    // Timestamp is at least 20 chars: "2025-04-30T00:53:42Z"
    // Can have fractional seconds: "2025-04-30T00:53:42.8434646Z"
    if line.len() >= 20 {
        // Find the 'Z' that ends the timestamp
        if let Some(z_pos) = line[19..].find('Z') {
            let ts_end = 19 + z_pos + 1; // include 'Z'
            let ts = &line[..ts_end];
            // Validate it looks like a timestamp
            if ts.len() >= 20
                && ts.as_bytes()[4] == b'-'
                && ts.as_bytes()[7] == b'-'
                && ts.as_bytes()[10] == b'T'
                && ts.as_bytes()[13] == b':'
            {
                let rest = if line.len() > ts_end + 1 {
                    &line[ts_end + 1..] // skip the space after Z
                } else {
                    ""
                };
                return (Some(ts.to_string()), rest);
            }
        }
    }
    (None, line)
}

/// Parse `##[annotation]` prefixes from the content after the timestamp.
fn parse_annotation(content: &str) -> (LogLevel, String, bool, bool) {
    if let Some(rest) = content.strip_prefix("##[debug]") {
        (LogLevel::Debug, rest.to_string(), false, false)
    } else if let Some(rest) = content.strip_prefix("##[warning]") {
        (LogLevel::Warning, rest.to_string(), false, false)
    } else if let Some(rest) = content.strip_prefix("##[error]") {
        (LogLevel::Error, rest.to_string(), false, false)
    } else if let Some(rest) = content.strip_prefix("##[notice]") {
        (LogLevel::Notice, rest.to_string(), false, false)
    } else if let Some(rest) = content.strip_prefix("##[group]") {
        (LogLevel::Group, rest.to_string(), true, false)
    } else if content.starts_with("##[endgroup]") {
        (LogLevel::Info, String::new(), false, true)
    } else if let Some(rest) = content.strip_prefix("##[command]") {
        (LogLevel::Command, rest.to_string(), false, false)
    } else {
        (LogLevel::Info, content.to_string(), false, false)
    }
}

/// Parse a single runner diagnostic log line.
///
/// Format: `[2025-04-30 00:53:40Z INFO HostContext] message`
fn parse_runner_line(line_number: usize, line: &str) -> LogEntry {
    // Try to parse the `[timestamp LEVEL Source] message` format
    if let Some(inner) = line.strip_prefix('[')
        && let Some(bracket_end) = inner.find(']')
    {
        let bracket_content = &inner[..bracket_end];
        let full_bracket_end = bracket_end + 1; // account for stripped '['
        let rest = if line.len() > full_bracket_end + 2 {
            &line[full_bracket_end + 2..] // skip '] '
        } else {
            ""
        };

        // Split bracket content: "2025-04-30 00:53:40Z INFO HostContext"
        let parts: Vec<&str> = bracket_content.splitn(4, ' ').collect();
        if parts.len() >= 3 {
            // parts[0] = date, parts[1] = time (with Z), parts[2] = level
            let timestamp = format!("{}T{}", parts[0], parts[1]);
            let level = match parts[2] {
                "INFO" => LogLevel::Info,
                "WARN" => LogLevel::Warning,
                "ERR" => LogLevel::Error,
                "DEBUG" => LogLevel::Debug,
                _ => LogLevel::Info,
            };

            let epoch_millis = iso8601_to_epoch_millis(&timestamp);

            return LogEntry {
                line_number,
                timestamp: Some(timestamp),
                epoch_millis,
                level,
                message: rest.to_string(),
                group_start: false,
                group_end: false,
            };
        }
    }

    // Fallback: unparseable line
    LogEntry {
        line_number,
        timestamp: None,
        epoch_millis: None,
        level: LogLevel::Info,
        message: line.to_string(),
        group_start: false,
        group_end: false,
    }
}

/// Convert an ISO 8601 timestamp string to Unix epoch milliseconds.
///
/// Supports:
/// - `2025-04-30T00:53:42Z`
/// - `2025-04-30T00:53:42.8434646Z`
///
/// Returns `None` if the timestamp cannot be parsed.
fn iso8601_to_epoch_millis(ts: &str) -> Option<i64> {
    let ts = ts.strip_suffix('Z')?;
    let (datetime, frac) = if let Some((dt, f)) = ts.split_once('.') {
        (dt, f)
    } else {
        (ts, "")
    };

    let parts: Vec<&str> = datetime.split(['T', '-', ':']).collect();
    if parts.len() != 6 {
        return None;
    }

    let year: i64 = parts[0].parse().ok()?;
    let month: i64 = parts[1].parse().ok()?;
    let day: i64 = parts[2].parse().ok()?;
    let hour: i64 = parts[3].parse().ok()?;
    let min: i64 = parts[4].parse().ok()?;
    let sec: i64 = parts[5].parse().ok()?;

    // Parse fractional seconds to milliseconds
    let millis: i64 = if frac.is_empty() {
        0
    } else {
        // Pad or truncate to 3 digits
        let padded = format!("{:0<3}", &frac[..frac.len().min(3)]);
        padded.parse().unwrap_or(0)
    };

    // Days from year 1970 to the start of `year`
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * m + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;

    Some(days * 86_400_000 + hour * 3_600_000 + min * 60_000 + sec * 1_000 + millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_workflow_debug_line() {
        let line = "2025-04-30T00:53:42.8434646Z ##[debug]Starting: hello-world";
        let entries = parse_workflow_log(line);
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.timestamp.as_deref(), Some("2025-04-30T00:53:42.8434646Z"));
        assert_eq!(e.level, LogLevel::Debug);
        assert_eq!(e.message, "Starting: hello-world");
    }

    #[test]
    fn test_parse_workflow_group_line() {
        let line = "2025-04-30T00:53:42.8643242Z ##[group]Operating System";
        let entries = parse_workflow_log(line);
        let e = &entries[0];
        assert_eq!(e.level, LogLevel::Group);
        assert!(e.group_start);
        assert_eq!(e.message, "Operating System");
    }

    #[test]
    fn test_parse_workflow_endgroup() {
        let line = "2025-04-30T00:53:42.8646814Z ##[endgroup]";
        let entries = parse_workflow_log(line);
        let e = &entries[0];
        assert!(e.group_end);
    }

    #[test]
    fn test_parse_workflow_plain_line() {
        let line = "2025-04-30T00:53:44.8073929Z Hello, world!";
        let entries = parse_workflow_log(line);
        let e = &entries[0];
        assert_eq!(e.level, LogLevel::Info);
        assert_eq!(e.message, "Hello, world!");
    }

    #[test]
    fn test_parse_workflow_strips_leading_bom() {
        // GitHub Actions log files often start with a UTF-8 BOM. The parser
        // must strip it so line 1's timestamp + message parse correctly.
        let content = "\u{FEFF}2025-04-30T00:53:44.8073929Z Current runner version: '2.334.0'";
        let entries = parse_workflow_log(content);
        let e = &entries[0];
        assert_eq!(e.timestamp.as_deref(), Some("2025-04-30T00:53:44.8073929Z"));
        assert_eq!(e.message, "Current runner version: '2.334.0'");
    }

    #[test]
    fn test_parse_runner_log_line() {
        let line = "[2025-04-30 00:53:40Z INFO Listener] Version: 2.323.0";
        let entries = parse_runner_log(line);
        let e = &entries[0];
        assert_eq!(e.timestamp.as_deref(), Some("2025-04-30T00:53:40Z"));
        assert_eq!(e.level, LogLevel::Info);
        assert_eq!(e.message, "Version: 2.323.0");
    }

    #[test]
    fn test_parse_runner_warn_line() {
        let line = "[2025-04-30 00:53:40Z WARN SomeComponent] Something happened";
        let entries = parse_runner_log(line);
        let e = &entries[0];
        assert_eq!(e.level, LogLevel::Warning);
    }

    #[test]
    fn test_parse_multiline_workflow_log() {
        let content = "2025-04-30T00:53:42.8434646Z ##[debug]Starting\n\
                        2025-04-30T00:53:42.8464478Z Normal output\n\
                        2025-04-30T00:53:42.8612100Z ##[error]Something broke";
        let entries = parse_workflow_log(content);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].level, LogLevel::Debug);
        assert_eq!(entries[1].level, LogLevel::Info);
        assert_eq!(entries[2].level, LogLevel::Error);
        assert_eq!(entries[0].line_number, 1);
        assert_eq!(entries[2].line_number, 3);
    }

    #[test]
    fn test_parse_runner_line_bare_bracket() {
        // Lines like "]" or "] something" must not panic
        let entries = parse_runner_log("]\n] trailing\n[]\nplain text");
        assert_eq!(entries.len(), 4);
        for e in &entries {
            assert_eq!(e.level, LogLevel::Info);
            assert!(e.epoch_millis.is_none());
        }
    }

    #[test]
    fn test_epoch_millis_workflow() {
        // 2025-04-30T00:53:42Z = 1745974422000 ms
        let entries = parse_workflow_log("2025-04-30T00:53:42Z some text");
        let e = &entries[0];
        assert_eq!(e.epoch_millis, Some(1745974422000));
    }

    #[test]
    fn test_epoch_millis_fractional() {
        // 2025-04-30T00:53:42.843Z → 1745974422843 ms
        let entries = parse_workflow_log("2025-04-30T00:53:42.8434646Z some text");
        let e = &entries[0];
        assert_eq!(e.epoch_millis, Some(1745974422843));
    }

    #[test]
    fn test_epoch_millis_runner() {
        // Same moment via runner format
        let entries = parse_runner_log("[2025-04-30 00:53:42Z INFO Host] msg");
        let e = &entries[0];
        assert_eq!(e.epoch_millis, Some(1745974422000));
    }

    #[test]
    fn test_epoch_millis_epoch_start() {
        // 1970-01-01T00:00:00Z = 0
        assert_eq!(iso8601_to_epoch_millis("1970-01-01T00:00:00Z"), Some(0));
    }

    #[test]
    fn test_epoch_millis_known_date() {
        // 2000-01-01T00:00:00Z = 946684800000
        assert_eq!(
            iso8601_to_epoch_millis("2000-01-01T00:00:00Z"),
            Some(946684800000)
        );
    }

    #[test]
    fn test_extract_action_references() {
        let entries = parse_workflow_log(
            "2025-04-30T00:00:00Z Download action repository 'actions/checkout@08c6903cd8c0fde910a37f88322edcfb5dd907a8'\n\
             2025-04-30T00:00:01Z Preparing github/codeql-action/init@v3\n\
             2025-04-30T00:00:02Z Loading actions/create-github-app-token@main",
        );

        let refs = extract_action_references(&entries);
        assert_eq!(refs.len(), 3);
        assert_eq!(refs[0].owner_repo(), "actions/checkout");
        assert_eq!(refs[1].owner_repo(), "github/codeql-action");
        assert_eq!(refs[1].path.as_deref(), Some("/init"));
        assert_eq!(refs[2].reference, "main");
    }

    #[test]
    fn test_extract_action_references_deduplicates() {
        let entries = parse_workflow_log(
            "2025-04-30T00:00:00Z Run actions/checkout@v4\n\
             2025-04-30T00:00:01Z Reusing actions/checkout@v4",
        );

        let refs = extract_action_references(&entries);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].owner_repo(), "actions/checkout");
        assert_eq!(refs[0].first_line, 1);
    }

    #[test]
    fn test_extract_action_references_skips_job_defined_at() {
        // The "Job defined at:" line points to the workflow file that defines
        // the current job, not to a dependency. Filter it out.
        let entries = parse_workflow_log(
            "2025-04-30T00:00:00Z Run actions/checkout@v4\n\
             2025-04-30T00:00:01Z Job defined at: corycalahan/hello-world-workflow/.github/workflows/hello-world.yml@refs/heads/main",
        );

        let refs = extract_action_references(&entries);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].owner_repo(), "actions/checkout");
    }

    #[test]
    fn test_extract_action_references_keeps_reusable_workflows() {
        // Reusable workflow calls look like `owner/repo/.github/workflows/x.yml@ref`
        // and should still be captured — they're real dependencies.
        let entries = parse_workflow_log(
            "2025-04-30T00:00:00Z Download action repository 'octo-org/shared/.github/workflows/build.yml@v1'",
        );

        let refs = extract_action_references(&entries);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].owner, "octo-org");
        assert_eq!(refs[0].repo, "shared");
        assert_eq!(
            refs[0].path.as_deref(),
            Some("/.github/workflows/build.yml")
        );
        assert_eq!(refs[0].reference, "v1");
    }

    #[test]
    fn test_extract_action_references_skips_warning_prose() {
        // Real-world case from logs_66805516233/0_hello-world.txt: a
        // deprecation warning mentions "actions/checkout@v4." with a
        // sentence-ending period. Without the warning skip, the regex
        // captured "v4." as a separate reference.
        let entries = parse_workflow_log(
            "2026-04-29T23:55:40.4172303Z Download action repository 'actions/checkout@v4' (SHA:34e114876b0b11c390a56381ad16ebd13914f8d5)\n\
             2026-04-29T23:55:40.6646400Z ##[group]Run actions/checkout@v4\n\
             2026-04-29T23:55:41.5016115Z ##[warning]Node.js 20 actions are deprecated. The following actions are running on Node.js 20 and may not work as expected: actions/checkout@v4. Actions will be forced to run with Node.js 24 by default starting June 2nd, 2026.",
        );

        let refs = extract_action_references(&entries);
        assert_eq!(refs.len(), 1, "expected only the legitimate v4 ref");
        assert_eq!(refs[0].owner_repo(), "actions/checkout");
        assert_eq!(refs[0].reference, "v4");
    }

    #[test]
    fn test_extract_action_references_trims_trailing_punctuation() {
        // Defense-in-depth: if a non-warning info line ever contains a ref
        // followed by punctuation, normalize it rather than treating "v4,"
        // and "v4" as distinct references.
        let entries = parse_workflow_log(
            "2025-04-30T00:00:00Z Note: using actions/checkout@v4, then continuing.",
        );

        let refs = extract_action_references(&entries);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].reference, "v4");
    }
}
