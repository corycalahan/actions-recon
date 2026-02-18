use askama::Template;
use axum::Form;
use axum::extract::State;
use axum::response::{Html, Redirect};
use std::sync::Arc;

use crate::config::AppConfig;
use crate::models::tips;

/// Template for the settings / tip admin page.
#[derive(Template)]
#[template(path = "settings.html")]
struct SettingsTemplate {
    tips: Vec<tips::TipSummary>,
    flash: Option<String>,
    flash_error: Option<String>,
}

/// `GET /settings` — admin panel listing all tips with edit/create forms.
pub async fn index(State(_config): State<Arc<AppConfig>>) -> Html<String> {
    let summaries = tips::load_tip_summaries(std::path::Path::new("tips"));

    let template = SettingsTemplate {
        tips: summaries,
        flash: None,
        flash_error: None,
    };

    Html(
        template
            .render()
            .unwrap_or_else(|e| format!("Template error: {e}")),
    )
}

/// Form data for saving a tip.
#[derive(serde::Deserialize)]
pub struct TipForm {
    pub id: String,
    #[serde(default)]
    pub source_id: String,
    pub name: String,
    pub emoji: String,
    #[serde(default)]
    pub docs: String,
    #[serde(default)]
    pub description: String,
    pub check_type: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub applies_to: String,
    #[serde(default)]
    pub enabled: String,
    #[serde(default)]
    pub pattern: String,
    #[serde(default)]
    pub pattern_missing: String,
    #[serde(default)]
    pub patterns_any: String,
    #[serde(default)]
    pub patterns_missing_any: String,
    #[serde(default)]
    pub threshold_secs: String,
    #[serde(default)]
    pub threshold_secs_step: String,
    #[serde(default)]
    pub mark: String,
    #[serde(default)]
    pub mark_step: String,
    #[serde(default)]
    pub step_name: String,
    #[serde(default)]
    pub level: String,
    #[serde(default)]
    pub threshold: String,
}

fn parse_optional_number<T>(field_name: &str, raw: &str) -> anyhow::Result<Option<T>>
where
    T: std::str::FromStr,
    <T as std::str::FromStr>::Err: std::fmt::Display,
{
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    let parsed = trimmed
        .parse::<T>()
        .map_err(|e| anyhow::anyhow!("{field_name} must be a valid number: {e}"))?;
    Ok(Some(parsed))
}

fn parse_patterns_list(raw: &str) -> Vec<String> {
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

/// `POST /settings/tips` — create or update a tip.
pub async fn save_tip(
    State(_config): State<Arc<AppConfig>>,
    Form(form): Form<TipForm>,
) -> Result<Redirect, Html<String>> {
    let tips_dir = std::path::Path::new("tips");

    let threshold_secs = match parse_optional_number::<u64>("threshold_secs", &form.threshold_secs)
    {
        Ok(value) => value,
        Err(e) => {
            let summaries = tips::load_tip_summaries(tips_dir);
            let template = SettingsTemplate {
                tips: summaries,
                flash: None,
                flash_error: Some(format!("Failed to save tip: {e}")),
            };
            return Err(Html(
                template
                    .render()
                    .unwrap_or_else(|te| format!("Template error: {te}")),
            ));
        }
    };

    let threshold = match parse_optional_number::<usize>("threshold", &form.threshold) {
        Ok(value) => value,
        Err(e) => {
            let summaries = tips::load_tip_summaries(tips_dir);
            let template = SettingsTemplate {
                tips: summaries,
                flash: None,
                flash_error: Some(format!("Failed to save tip: {e}")),
            };
            return Err(Html(
                template
                    .render()
                    .unwrap_or_else(|te| format!("Template error: {te}")),
            ));
        }
    };

    let threshold_secs_step =
        match parse_optional_number::<u64>("threshold_secs_step", &form.threshold_secs_step) {
            Ok(value) => value,
            Err(e) => {
                let summaries = tips::load_tip_summaries(tips_dir);
                let template = SettingsTemplate {
                    tips: summaries,
                    flash: None,
                    flash_error: Some(format!("Failed to save tip: {e}")),
                };
                return Err(Html(
                    template
                        .render()
                        .unwrap_or_else(|te| format!("Template error: {te}")),
                ));
            }
        };

    let selected_pattern = if form.check_type == "missing_pattern" {
        Some(form.pattern_missing.as_str())
    } else if form.check_type == "pattern_match" {
        Some(form.pattern.as_str())
    } else {
        None
    };

    let selected_patterns = if form.check_type == "contains_any_patterns" {
        Some(parse_patterns_list(&form.patterns_any))
    } else if form.check_type == "missing_any_pattern" {
        Some(parse_patterns_list(&form.patterns_missing_any))
    } else {
        None
    };

    let selected_step = if form.check_type == "step_duration" {
        Some(form.step_name.as_str())
    } else {
        None
    };

    let selected_mark = if form.check_type == "step_duration" {
        form.mark_step.as_str()
    } else {
        form.mark.as_str()
    };

    let selected_threshold_secs = if form.check_type == "step_duration" {
        threshold_secs_step
    } else {
        threshold_secs
    };

    let result = tips::save_tip(tips::SaveTipInput {
        dir: tips_dir,
        id: &form.id,
        source_id: Some(form.source_id.as_str()),
        name: &form.name,
        emoji: &form.emoji,
        enabled: form.enabled.as_str() == "on",
        docs: Some(form.docs.as_str()),
        description: Some(form.description.as_str()),
        check_type: &form.check_type,
        scope: Some(form.scope.as_str()),
        applies_to: Some(form.applies_to.as_str()),
        pattern: selected_pattern,
        patterns: selected_patterns,
        step: selected_step,
        threshold_secs: selected_threshold_secs,
        mark: Some(selected_mark),
        level: Some(form.level.as_str()),
        threshold,
    });

    match result {
        Ok(()) => Ok(Redirect::to("/settings")),
        Err(e) => {
            // Re-render the page with an error flash
            let summaries = tips::load_tip_summaries(tips_dir);
            let template = SettingsTemplate {
                tips: summaries,
                flash: None,
                flash_error: Some(format!("Failed to save tip: {e}")),
            };
            Err(Html(
                template
                    .render()
                    .unwrap_or_else(|te| format!("Template error: {te}")),
            ))
        }
    }
}

/// Form data for deleting a tip.
#[derive(serde::Deserialize)]
pub struct DeleteTipForm {
    pub source_id: String,
}

/// `POST /settings/tips/delete` — delete a tip by ID.
pub async fn delete_tip(
    State(_config): State<Arc<AppConfig>>,
    Form(form): Form<DeleteTipForm>,
) -> Result<Redirect, Html<String>> {
    let tips_dir = std::path::Path::new("tips");

    match tips::delete_tip(tips_dir, &form.source_id) {
        Ok(()) => Ok(Redirect::to("/settings")),
        Err(e) => {
            let summaries = tips::load_tip_summaries(tips_dir);
            let template = SettingsTemplate {
                tips: summaries,
                flash: None,
                flash_error: Some(format!("Failed to delete tip: {e}")),
            };
            Err(Html(
                template
                    .render()
                    .unwrap_or_else(|te| format!("Template error: {te}")),
            ))
        }
    }
}
