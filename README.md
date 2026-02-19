# actions-recon

A local web app for analyzing and visualizing GitHub Actions workflow logs. Upload a workflow log ZIP and explore each log file in a timeline table.

## Prerequisites

**Rust (stable toolchain)** — if you don't have it installed:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```
- Source: https://rust-lang.org/tools/install/

Follow the prompts, then restart your shell (or run `source "$HOME/.cargo/env"`). Verify with:

```sh
rustc --version
cargo --version
```

## Getting Started

```sh
# Clone the repo
git clone https://github.com/corycalahan/actions-recon.git
cd actions-recon

# Build the project
cargo build

# Run the server (default: http://127.0.0.1:8080)
cargo run
```

Open <http://127.0.0.1:8080> in your browser.

### Override the default port

```sh
cargo run -- --port 8181
```

Or set the `PORT` environment variable:

```sh
PORT=8181 cargo run
```

### Configuration

Copy the example env file and edit as needed:

```sh
cp .env.example .env
```

| Variable     | Default           | Description                           |
|-------------|-------------------|----------------------------------------|
| `PORT`       | `8080`            | Port the server listens on            |
| `UPLOAD_DIR` | `./data/uploads`  | Directory for extracted log files     |

CLI flags (e.g. `--port`) take precedence over environment variables.

## Usage

1. **Upload** — On the home page, select a GitHub Actions log ZIP file and click **Upload & Analyze**.
2. **Browse** — The analysis overview lists all extracted log files, grouped by category (Job Logs, Step Logs, System Logs, Runner Diagnostics). Click any file to view its timeline.
3. **Timeline** — Each log file is rendered as a timeline table with line numbers, timestamps, log levels, and messages. A referenced actions summary table appears when action refs are detected (owner/repo, optional path, and version/ref). Toggle debug lines and group markers on/off. Click **epoch** next to any timestamp to copy the Unix epoch (ms) to clipboard. Click the **#** column header to reverse row order (ascending/descending).
4. **Tips** — If any troubleshooting tips match the log, a collapsible "tips detected" banner appears above the timeline. Matched lines are marked with emoji indicators in the 💡 column. Toggle individual tips on/off via checkboxes; tips without matched lines are marked as banner-only and still toggle their banner state.
5. **Clean up** — Use the **Delete All Extracted Files** button on the home page to remove all locally stored data. The current storage path is displayed above the button.
6. **Light/Dark modes** — Toggle light/dark mode with the button in the header. Your preference is saved in `localStorage`.

The analysis ID in the URL is always the ZIP filename (e.g. uploading `1234567890.zip` → `/analysis/1234567890`).

Runner diagnostic logs (nested ZIPs inside `runner-diagnostic-logs/`) are automatically extracted during upload.

## Development

```sh
cargo build                        # compile
cargo run                          # start the server
cargo test                         # run tests
cargo fmt                          # auto-format code
cargo fmt --check                  # check formatting (CI)
cargo clippy -- -D warnings        # lint (warnings = errors)
```

### Project Structure

```
src/
  main.rs            # Server startup, router, tracing init
  config.rs          # CLI args (clap) + env var configuration
  extract/
    mod.rs           # Extraction module declarations
    zip_extract.rs   # Safe ZIP extraction with security validation
  models/
    mod.rs           # Model module declarations
    log_parser.rs    # Parse workflow logs and runner diagnostic logs
    tips.rs          # TOML-based troubleshooting tips loader & evaluator
  routes/
    mod.rs           # Route module declarations
    home.rs          # GET / — home page; POST /delete-all — clear files
    upload.rs        # POST /upload — ZIP upload + extraction
    analysis.rs      # GET /analysis/:id — overview; GET /analysis/:id/*logfile — timeline
    settings.rs      # GET /settings — tip admin; POST /settings/tips — save tip; POST /settings/tips/delete — delete tip
templates/
  base.html          # Shared HTML layout
  home.html          # Home page template
  analysis.html      # Analysis overview (file groups, tooltips)
  logfile.html       # Log file timeline table
  error.html         # Error page template
static/
  style.css          # Stylesheet
tips/                # Troubleshooting tip definitions (TOML files)
data/uploads/        # Extracted log files (gitignored)
```

### Tips Library

The `tips/` directory contains TOML files that define automated troubleshooting checks. When a log file is viewed, tips are evaluated against its entries with optional per-tip scope (`all`, `workflow`, or `runner`), applicability mode (`all`, `standard_logs`, `debug_logs_enabled`, or `diagnostic_logs_enabled`), and status (`enabled`/`disabled`). Triggered tips appear in a collapsible banner with emoji indicators on matched rows when applicable.

For recommended defaults and when-to-use guidance, see the **Starter Presets** section in [tips/README.md](tips/README.md).

**Adding a new tip:** Create a `.toml` file in `tips/` following the schema in [tips/README.md](tips/README.md). Supported check types include:

| Check Type        | Description                                                                |
|-------------------|----------------------------------------------------------------------------|
| `pattern_match`   | Flag lines matching a regex pattern                                        |
| `contains_any_patterns` | Flag lines matching any regex in a list                              |
| `time_delta`      | Flag when elapsed-from-start exceeds a threshold                           |
| `time_gap`        | Flag when a gap between adjacent lines is too large                        |
| `step_duration`   | Flag when a specific step exceeds duration threshold                       |
| `level_count`     | Flag when a log level count exceeds a threshold                            |
| `missing_pattern` | Flag when an expected pattern is absent from the log                       |
| `missing_any_pattern` | Flag when any expected pattern is absent                               |
| `version_check`   | Flag when a captured version is below `min_version` or above `max_version` |

Example tip files: `error_lines.toml`, `job_timeout_risk.toml`, `many_warnings.toml`, `disk_space_issue.toml`, `missing_job_completion.toml`, `any_failure_signals.toml`, `missing_expected_markers.toml`, `slow_deploy_step.toml`, `proxy_detected.toml`, `wireguard_detected.toml`, `runner_version_below_ghes_minimum.toml`, `runner_version_not_dotcom_latest.toml`.

### Routes

| Method | Path                        | Description                                |
|--------|-----------------------------|--------------------------------------------|
| GET    | `/`                         | Home page — upload form, previous analyses |
| POST   | `/upload`                   | Upload ZIP, extract, redirect to analysis  |
| GET    | `/analysis/:id`             | Analysis overview — grouped file listing   |
| GET    | `/analysis/:id/*logfile`    | Timeline view of a specific log file       |
| POST   | `/delete-all`               | Delete all extracted files, redirect to `/`|
| GET    | `/settings`                 | Tip admin — list, add, edit, delete tips   |
| POST   | `/settings/tips`            | Save (create or update) a tip              |
| POST   | `/settings/tips/delete`     | Delete a tip by source ID                  |

## Security Notes

- All uploaded ZIPs are treated as **untrusted input** — paths, sizes, and file counts are validated before extraction.
- The server binds to **127.0.0.1 only** (not exposed to the network). No authentication is needed for local-only use.
- ZIP extraction guards against **Zip Slip** (path traversal), symlinks, oversized files (100 MB), excessive file counts (1,000), and total extracted size (500 MB).
