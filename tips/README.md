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
type = "pattern_match"             # or "contains_any_patterns", "time_delta", "time_gap", "step_duration", "level_count", "missing_pattern", "missing_any_pattern"
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

## Starter Presets

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

Typical tuning pattern:

1. Start with broader thresholds/patterns to reduce false negatives.
2. Validate against a few known-good and known-bad logs.
3. Narrow thresholds/patterns to reduce false positives.
