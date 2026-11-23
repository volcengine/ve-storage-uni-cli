//! Experimental direct Doubao integration. Recommendations are parsed, never dispatched.
mod auth;
mod browser;
mod knowledge;
mod reply;
#[cfg(test)]
mod tests;

use clap::CommandFactory;
use serde_json::{json, Value};
use std::io::Write;
use tos_core::{
    agent::{
        envelope::{Envelope, ErrorDetail, ErrorKind},
        error::{CliError, ExitCode},
        global_args::GlobalArgs,
    },
    ask::cli::{AskAction, AskArgs, AuthAction},
};

pub(crate) struct Route<'a> {
    tool: &'static str,
    action: Action<'a>,
}
enum Action<'a> {
    Ask(Option<&'a str>),
    AuthUsage,
    Login(bool),
    Status,
    Logout,
    Invalid,
}

impl<'a> Route<'a> {
    pub(crate) fn from_tool(tool: &'a crate::ToolCommand) -> Option<Self> {
        use crate::ToolCommand;
        let (tool, action) = match tool {
            ToolCommand::TosCli {
                command: tos_cli::TosCliCommand::Ask(args),
                ..
            } => ("tos", common_action(args)),
            ToolCommand::Tos {
                command: Some(ve_tos_cli::TosCommand::Ask(args)),
                ..
            } => ("ve-tos", common_action(args)),
            ToolCommand::ADrive {
                command: ve_adrive_cli::ADriveCommand::Ask(args),
                ..
            } => ("ve-adrive", common_action(args)),
            _ => return None,
        };
        Some(Self { tool, action })
    }

    fn command(&self) -> String {
        let suffix = match self.action {
            Action::Ask(_) | Action::Invalid => "ask",
            Action::AuthUsage => "ask auth",
            Action::Login(_) => "ask auth login",
            Action::Status => "ask auth status",
            Action::Logout => "ask auth logout",
        };
        format!("{} {suffix}", self.tool)
    }
}

// [Review Fix #10] Route management only through the shared ask auth parser.
fn common_action(args: &AskArgs) -> Action<'_> {
    if args.action.is_some() && (args.question.is_some() || args.explicit_question.is_some()) {
        return Action::Invalid;
    }
    let Some(AskAction::Auth(auth)) = &args.action else {
        return Action::Ask(
            args.question
                .as_deref()
                .or(args.explicit_question.as_deref()),
        );
    };
    match auth.action {
        // [Review Fix #11] The group displays usage without selecting an auth operation.
        None => Action::AuthUsage,
        Some(AuthAction::Login { is_no_browser }) => Action::Login(is_no_browser),
        Some(AuthAction::Status) => Action::Status,
        Some(AuthAction::Logout) => Action::Logout,
    }
}

pub(crate) async fn dispatch(global: &GlobalArgs, route: Route<'_>, prefix: &str) {
    let command = route.command();
    let result = execute(global, &route, prefix).await;
    let exit_code = match result {
        Ok(payload) => {
            let envelope = Envelope::success(command, payload)
                .with_request_id(uuid::Uuid::now_v7().to_string());
            emit(global, &envelope, route.tool)
        }
        Err(error) => {
            let exit_code = error.exit_code().as_i32();
            let envelope = Envelope::failed(command, error_detail(global, &error, prefix))
                .with_request_id(uuid::Uuid::now_v7().to_string());
            emit_failure(&envelope, exit_code)
        }
    };
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}

fn emit_failure(envelope: &Envelope<()>, exit_code: i32) -> i32 {
    // [Review Fix #15] Failure envelopes have null data; table rendering and queries hide errors.
    let mut stderr = std::io::stderr().lock();
    let result = serde_json::to_writer_pretty(&mut stderr, envelope)
        .map_err(std::io::Error::other)
        .and_then(|()| writeln!(stderr));
    match result {
        Ok(()) => exit_code,
        Err(_) => ExitCode::Unknown.as_i32(),
    }
}

fn emit(global: &GlobalArgs, envelope: &impl serde::Serialize, tool: &str) -> i32 {
    // [Review Fix #7] Shared output must use the selected product's identity, then restore it.
    let binary = if tool == "ve-tos" { "ve-tos" } else { "tos" };
    let _presentation_guard = crate::EnvGuard::set(crate::TOS_CONFIG_BINARY_ENV, binary);
    match ve_tos_cli::handler::common::output_result(global, envelope) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("assistant output could not be rendered");
            error.exit_code().as_i32()
        }
    }
}

async fn execute(global: &GlobalArgs, route: &Route<'_>, prefix: &str) -> Result<Value, CliError> {
    // [Review Fix #10] Reject mixed input before previews while accepting inherited global flags.
    match route.action {
        Action::Invalid => Err(CliError::ValidationError(
            "question inputs cannot be combined with ask auth".into(),
        )),
        _ if global.describe || global.dry_run => Ok(preview(global, route, prefix)),
        Action::Ask(None) => Ok(
            json!({"usage":format!("{prefix} ask 'question'"),"experimental":true,"executed":false}),
        ),
        // [Review Fix #11] Group invocation is pure usage and never loads auth configuration.
        Action::AuthUsage => Ok(json!({
            "usage":format!("{prefix} ask auth <login|status|logout>"),
            "experimental":true,"executed":false
        })),
        Action::Ask(Some(question)) => ask_question(global, route.tool, prefix, question).await,
        Action::Login(is_no_browser) => auth::login(global, prefix, is_no_browser).await,
        Action::Status => auth::status(global),
        Action::Logout => auth::logout(global),
    }
}

fn preview(global: &GlobalArgs, route: &Route<'_>, prefix: &str) -> Value {
    // [Review Fix #8] Localize only owned preview prose; command/configuration fields stay stable.
    let (description, security_notice) = if global.uses_chinese_documentation() {
        (
            "单次存储 CLI 问答与命令建议。登录、刷新和问答直接连接豆包；不执行建议的命令。",
            "请勿在问题中包含凭证或敏感资源信息。无法保证阻止云端工具的副作用。",
        )
    } else {
        (
            "One-shot storage CLI recommendations. Login, refresh and Q&A are direct to Doubao; commands are never executed.",
            "Do not include credentials or sensitive resource information in questions. Cloud tool effects cannot be guaranteed prevented.",
        )
    };
    // [Review Fix #10] Auth previews advertise their own accepted invocation.
    let usage = match route.action {
        Action::Ask(_) | Action::Invalid => format!("{prefix} ask 'question'"),
        Action::AuthUsage => format!("{prefix} ask auth <login|status|logout>"),
        Action::Login(_) => format!("{prefix} ask auth login [--no-browser]"),
        Action::Status => format!("{prefix} ask auth status"),
        Action::Logout => format!("{prefix} ask auth logout"),
    };
    json!({"experimental":true,"dry_run":global.dry_run,"command":route.command(),"provider":"doubao","executed":false,
        "description":description,"security_notice":security_notice,
        "usage":usage,"login":auth::fix_command(global, prefix),
        "application_bundled":tos_core::ask::Config::has_bundled_application()})
}

async fn ask_question(
    global: &GlobalArgs,
    tool: &str,
    prefix: &str,
    question: &str,
) -> Result<Value, CliError> {
    let catalog = knowledge::Knowledge::new(crate::Cli::command(), tool)?;
    let prompt = catalog.prompt(question)?;
    let token = auth::access_token(global, prefix).await?;
    let client = tos_core::ask::SessionClient::new().map_err(auth::session_error)?;
    let response = client
        .ask(&token, &prompt)
        .await
        .map_err(auth::session_error)?;
    reply::validate(&catalog, &response.text, prefix)
}

fn error_detail(global: &GlobalArgs, error: &CliError, prefix: &str) -> ErrorDetail {
    let is_auth = matches!(error, CliError::AuthFailed(_));
    let is_config = matches!(error, CliError::ConfigMissing(_));
    ErrorDetail {
        status_code: None, code: if is_auth { "doubao_login_required" } else if is_config { "doubao_configuration" } else { "doubao_assistant_failed" }.into(),
        message: error.to_string(), exit_code: error.exit_code().as_i32(),
        kind: error_kind(error),
        category: error.agent_semantics().category,
        suggested_action: Some(if is_config { "Use an official prebuilt binary with Doubao configured, or contact the publisher. Internal application overrides must be complete." } else { "Review the error. Questions are never automatically replayed after an uncertain response." }.into()),
        fix_command: is_auth.then(|| auth::fix_command(global, prefix)), doctor_hint: None, docs_url: None,
    }
}

// [Review Fix #2] Structured errors must preserve actionable auth/permission/quota categories.
fn error_kind(error: &CliError) -> ErrorKind {
    match error {
        CliError::AuthFailed(_) => ErrorKind::AuthFailed,
        CliError::ConfigMissing(_) => ErrorKind::ConfigMissing,
        CliError::PermissionDenied(_) => ErrorKind::PermissionDenied,
        CliError::RateLimited(_) => ErrorKind::RateLimited,
        CliError::ValidationError(_) => ErrorKind::ValidationError,
        _ => ErrorKind::Unknown,
    }
}
