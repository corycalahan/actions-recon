# Tips Library

Each `.toml` file in this directory defines a troubleshooting tip that the
actions-recon engine evaluates against every log file.

## TOML Schema

```toml
# ── Metadata ──────────────────────────────────────────────
schema_version = 1                # optional today; defaults to 1 if omitted
id    = "unique-kebab-id"          # unique identifier
name  = "Human-Readable Name"     # shown in the UI banner
emoji = "⏱️"                       # marker shown on matching lines
docs  = "https://..."             # optional link to relevant docs
enabled = true                     # optional: true (active) | false (inactive)
created = "2026-02-13"             # date the tip was first created (YYYY-MM-DD)
updated = "2026-02-13"             # date of last update (YYYY-MM-DD, auto-set on save)

# Multi-line description displayed in the banner when the tip triggers.
description = """
Explain what the tip checks and what the engineer should do next.
"""

# ── Check ─────────────────────────────────────────────────
# Exactly ONE of the following check types:

[check]
type = "pattern_match"             # or "contains_any_patterns", "time_delta", "time_gap", "step_duration", "level_count", "missing_pattern", "missing_any_pattern", "version_check", "action_version_check"
scope = "all"                      # optional: "all" | "workflow" | "runner"
applies_to = "all"                 # optional: "all" | "standard_logs" | "debug_logs_enabled" | "diagnostic_logs_enabled"

# --- pattern_match ---
# Flags every line whose message matches the regex.
pattern = "(?i)error|fail"

# --- contains_any_patterns ---
# Flags lines that match any regex in the list.
patterns = ["(?i)error", "Process completed with exit code [^0]"]

# --- time_delta ---
# Flags runs where elapsed wall-clock time from the first timestamped line
# exceeds the threshold.
# threshold_secs: seconds from first timestamped line.
# mark: "first_last" marks first+last timestamped lines;
#       "last" marks the first line where the threshold is crossed.
threshold_secs = 21600
mark = "last"

# --- time_gap ---
# Flags when the largest gap between consecutive timestamped lines exceeds the threshold.
# threshold_secs: max allowed seconds between two adjacent timestamped lines.
# mark: "first_last" marks both lines around the largest gap;
#       "last" marks the later line after the gap.
threshold_secs = 900
mark = "last"

# --- step_duration ---
# Flags when elapsed time between "Starting: <step>" and "Finishing: <step>"
# exceeds the threshold.
step = "Deploy"
threshold_secs = 1800
mark = "last"

# --- level_count ---
# Flags when the count of a given log level exceeds a threshold.
level = "error"                    # "error" | "warning" | "debug" | ...
threshold = 5

# --- missing_pattern ---
# Flags when an expected pattern is NOT found anywhere in the log.
pattern = "Job completed"

# --- missing_any_pattern ---
# Flags when one or more expected patterns are absent.
patterns = ["Finishing:", "Upload complete"]

# --- version_check ---
# Flags when the version extracted by `pattern` (capture group 1) is below
# min_version or above max_version. Comparison is semantic (major.minor.patch).
# At least one of min_version or max_version must be set.
# pattern: regex with one capture group that yields the version string.
# min_version: the minimum acceptable version (inclusive, optional).
# max_version: the maximum acceptable version (inclusive, optional).
pattern = "Current runner version: '(\\d+\.\\d+\.\\d+)'"
min_version = "2.319.1"
# max_version = "3.0.0"   # optional upper bound

# --- action_version_check ---
# Flags when the resolved semver of a specific first-party action (read from the
# "Version: X.Y.Z" line that follows every "Download immutable action package"
# header) is below min_version or above max_version.
# Works for both SHA-pinned and tag-pinned action references.
# action: the owner/repo identifier of the action (e.g. "actions/checkout").
# min_version: the minimum acceptable version (inclusive, optional).
# max_version: the maximum acceptable version (inclusive, optional).
# At least one of min_version or max_version must be set.
action      = "actions/checkout"
min_version = "6.0.0"
# max_version = "7.0.0"   # optional upper bound

# Scope notes:
# - "all" (default): evaluate against workflow and runner logs
# - "workflow": only evaluate against workflow logs
# - "runner": only evaluate against runner diagnostic logs
#
# Applies-to notes:
# - "all" (default): evaluate regardless of log mode
# - "standard_logs": workflow logs without debug lines
# - "debug_logs_enabled": workflow logs containing debug lines
# - "diagnostic_logs_enabled": runner diagnostic logs
#
# Enabled notes:
# - true (default): tip is active and evaluated
# - false: tip is inactive and not evaluated until re-enabled
#
# Compatibility notes:
# - If `schema_version` is omitted, it is treated as version 1 for backward compatibility.
# - Unsupported schema versions are rejected at load time.
# - Saving a tip writes the current supported schema version.
```

## Adding a New Tip

1. Create a new `.toml` file in this directory (copy an existing one as a template).
2. Pick a unique `id` and an `emoji`.
3. Define the check type and parameters.
4. Restart the server — tips are loaded at startup.

## Starter Example: Standard Logs Only

Use this template when you want a tip to run only for workflow logs where
debug logging is not enabled:

```toml
schema_version = 1
id    = "standard-failure-signal"
name  = "Standard Failure Signal"
emoji = "🔎"
docs  = "https://docs.github.com/en/actions/monitoring-and-troubleshooting-workflows/troubleshooting-workflows"

description = """
Flags common failure signals in standard (non-debug) workflow logs.
"""

[check]
type = "contains_any_patterns"
scope = "workflow"
applies_to = "standard_logs"
patterns = [
	"(?i)process completed with exit code [^0]",
	"(?i)failed",
]
```

## Starter Tips

Use these built-in tips as quick starting points:

- `disk_space_issue.toml` (`pattern_match`) — Detect common disk exhaustion signatures.
- `error_lines.toml` (`level_count`) — Flag any error-level annotations in a log.
- `job_timeout_risk.toml` (`time_delta`) — Track elapsed-from-start timeout risk.
- `large_time_gap.toml` (`time_gap`) — Detect unusually large gaps between adjacent timestamped lines.
- `many_warnings.toml` (`level_count`) — Detect warning-volume spikes.
- `missing_job_completion.toml` (`missing_pattern`) — Detect missing expected completion markers.
- `any_failure_signals.toml` (`contains_any_patterns`) — Match any of several failure signatures.
- `missing_expected_markers.toml` (`missing_any_pattern`) — Trigger when any expected lifecycle marker is absent.
- `slow_deploy_step.toml` (`step_duration`) — Detect a specific step taking longer than expected.
- `proxy_detected.toml` (`contains_any_patterns`) — Detect proxy environment/configuration signals.
- `wireguard_detected.toml` (`contains_any_patterns`) — Detect WireGuard setup/use signals for private networking.
- `runner_version_below_ghes_minimum.toml` (`version_check`) — Flag self-hosted runners below the minimum version required by the oldest supported GitHub Enterprise Server (GHES) release.
- `runner_version_not_dotcom_latest.toml` (`version_check`) — Flag runners below the latest version available on GitHub.com.

Frequently used Actions published by GitHub:

- `actions-checkout_outdated_version.toml` (`action_version_check`) — Flag when a newer version of [`actions/checkout`](https://github.com/actions/checkout) is available.
- `actions-upload-artifact_outdated_version.toml` (`action_version_check`) — Flag when a newer version of [`actions/upload-artifact`](https://github.com/actions/upload-artifact) is available.
- `actions-download-artifact_outdated_version.toml` (`action_version_check`) — Flag when a newer version of [`actions/download-artifact`](https://github.com/actions/download-artifact) is available.
- `actions-cache_outdated_version.toml` (`action_version_check`) — Flag when a newer version of [`actions/cache`](https://github.com/actions/cache) is available.
- `actions-setup-node_outdated_version.toml` (`action_version_check`) — Flag when a newer version of [`actions/setup-node`](https://github.com/actions/setup-node) is available.
- `actions-setup-python_outdated_version.toml` (`action_version_check`) — Flag when a newer version of [`actions/setup-python`](https://github.com/actions/setup-python) is available.
- `actions-setup-java_outdated_version.toml` (`action_version_check`) — Flag when a newer version of [`actions/setup-java`](https://github.com/actions/setup-java) is available.
- `actions-setup-go_outdated_version.toml` (`action_version_check`) — Flag when a newer version of [`actions/setup-go`](https://github.com/actions/setup-go) is available.
- `actions-setup-dotnet_outdated_version.toml` (`action_version_check`) — Flag when a newer version of [`actions/setup-dotnet`](https://github.com/actions/setup-dotnet) is available.
- `actions-create-release_outdated_version.toml` (`action_version_check`) — Flag when [`actions/create-release`](https://github.com/actions/create-release) is in use (archived; consider a maintained alternative).
- `actions-delete-package-versions_outdated_version.toml` (`action_version_check`) — Flag when a newer version of [`actions/delete-package-versions`](https://github.com/actions/delete-package-versions) is available.
- `actions-deploy-pages_outdated_version.toml` (`action_version_check`) — Flag when a newer version of [`actions/deploy-pages`](https://github.com/actions/deploy-pages) is available.
- `actions-github-script_outdated_version.toml` (`action_version_check`) — Flag when a newer version of [`actions/github-script`](https://github.com/actions/github-script) is available.
- `actions-jekyll-build-pages_outdated_version.toml` (`action_version_check`) — Flag when a newer version of [`actions/jekyll-build-pages`](https://github.com/actions/jekyll-build-pages) is available.
- `actions-labeler_outdated_version.toml` (`action_version_check`) — Flag when a newer version of [`actions/labeler`](https://github.com/actions/labeler) is available.
- `actions-stale_outdated_version.toml` (`action_version_check`) — Flag when a newer version of [`actions/stale`](https://github.com/actions/stale) is available.
- `github-dependabot-action_outdated_version.toml` (`action_version_check`) — Flag when a newer version of [`github/dependabot-action`](https://github.com/github/dependabot-action) is available.
- `github-codeql-action-init_outdated_version.toml` (`action_version_check`) — Flag when a newer version of [`github/codeql-action/init`](https://github.com/github/codeql-action) is available.
- `github-codeql-action-analyze_outdated_version.toml` (`action_version_check`) — Flag when a newer version of [`github/codeql-action/analyze`](https://github.com/github/codeql-action) is available.
- `github-codeql-action-autobuild_outdated_version.toml` (`action_version_check`) — Flag when a newer version of [`github/codeql-action/autobuild`](https://github.com/github/codeql-action) is available.
- `github-codeql-action-upload-sarif_outdated_version.toml` (`action_version_check`) — Flag when a newer version of [`github/codeql-action/upload-sarif`](https://github.com/github/codeql-action) is available.

Typical tuning pattern:

1. Start with broader thresholds/patterns to reduce false negatives.
2. Validate against a few known-good and known-bad logs.
3. Narrow thresholds/patterns to reduce false positives.
