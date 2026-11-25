use super::knowledge::Knowledge;
use serde::Deserialize;
use serde_json::{json, Value};
use tos_core::agent::error::CliError;

const MAX_REPLY_BYTES: usize = 64 * 1024;
const MAX_TEXT_BYTES: usize = 4096;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    result_type: ResultType,
    summary: String,
    steps: Vec<Step>,
}
#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum ResultType {
    Recommendation,
    Explanation,
    Clarification,
    OutOfScope,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Step {
    command_id: String,
    argv: Vec<String>,
    description: String,
}

pub(super) fn validate(catalog: &Knowledge, body: &str, prefix: &str) -> Result<Value, CliError> {
    if body.len() > MAX_REPLY_BYTES || !body.trim_start().starts_with('{') {
        return Err(invalid());
    }
    let reply: Reply = serde_json::from_str(body).map_err(|_| invalid())?;
    valid_field(&reply.summary)?;
    let is_recommendation = reply.result_type == ResultType::Recommendation;
    if reply.steps.len() > 8 || is_recommendation == reply.steps.is_empty() {
        return Err(invalid());
    }
    let steps = reply
        .steps
        .iter()
        .map(|step| validate_step(catalog, step, prefix))
        .collect::<Result<Vec<_>, _>>()?;
    let kind = match reply.result_type {
        ResultType::Recommendation => "recommendation",
        ResultType::Explanation => "explanation",
        ResultType::Clarification => "clarification",
        ResultType::OutOfScope => "out_of_scope",
    };
    Ok(
        json!({"result_type":kind,"summary":reply.summary,"steps":steps,
        "syntax_validated":is_recommendation,"executed":false,
        "review_notice":"Review semantics, resource identifiers, permissions and effects before running any command. Syntax validation does not establish safety or correctness of the requested operation."}),
    )
}

fn validate_step(catalog: &Knowledge, step: &Step, prefix: &str) -> Result<Value, CliError> {
    valid_field(&step.command_id)?;
    valid_field(&step.description)?;
    if step.argv.is_empty() || step.argv.len() > 128 {
        return Err(invalid());
    }
    for argument in &step.argv {
        if argument.len() > MAX_TEXT_BYTES || !safe_text(argument) {
            return Err(invalid());
        }
    }
    catalog.validate(&step.command_id, &step.argv)?;
    let command = format!(
        "{prefix} {}",
        step.argv
            .iter()
            .map(|argument| quote(argument))
            .collect::<Vec<_>>()
            .join(" ")
    );
    Ok(
        json!({"command_id":step.command_id,"argv":step.argv,"description":step.description,"command":command}),
    )
}

fn valid_field(value: &str) -> Result<(), CliError> {
    if value.trim().is_empty() || value.len() > MAX_TEXT_BYTES || !safe_text(value) {
        return Err(invalid());
    }
    Ok(())
}
pub(super) fn safe_text(value: &str) -> bool {
    // [Review Fix #1] Reject deprecated bidi formatting and Unicode line separators too.
    !value.chars().any(|character| character.is_control() || matches!(character, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{206f}'))
}
pub(super) fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
fn invalid() -> CliError {
    CliError::ValidationError(
        "Doubao returned an invalid structured recommendation; no command was executed".into(),
    )
}
