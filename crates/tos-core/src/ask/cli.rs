//! Shared parser-only assistant arguments; parsing never loads credentials.
use clap::{Args, Subcommand};

/// Shared long-help overview for the three product-specific assistant commands.
pub const ASK_HELP_OVERVIEW: &str = "Put the quoted question directly after ask to get command recommendations.\n\nUse ask auth to manage Doubao login. Recommended commands are never executed.";

/// Builds copyable question and login examples for the dedicated CLI `binary_name`.
/// Returns help text without configuration access, I/O, or command execution.
pub fn question_help_examples(binary_name: &str) -> String {
    // [Review Fix #26] Use the existing after-help example layout across all products.
    format!("Examples:\n  {binary_name} ask '如何上传本地文件？'\n  {binary_name} ask auth login\n  {binary_name} ask --question 'auth'")
}

/// Returns separate question and authorization usage forms for the dedicated CLI `binary_name`.
/// Only formats help text; the existing parser still accepts bare ask for usage guidance.
pub fn question_help_usage(binary_name: &str) -> String {
    // [Review Fix #25] Show alternatives explicitly instead of suggesting question and auth combine.
    format!("{binary_name} ask '<QUESTION>' [OPTIONS]\n       {binary_name} ask auth <COMMAND> [OPTIONS]")
}

/// Ask a single storage CLI question; omission prints usage without starting a session.
#[derive(Debug, Args)]
// [Review Fix #10] Restore the previously accepted ask auth namespace.
#[command(disable_help_subcommand = true)]
pub struct AskArgs {
    /// Question text placed directly after ask (quote it as one argument)
    #[arg(conflicts_with = "explicit_question")]
    pub question: Option<String>,
    /// Explicit question, including the reserved word auth
    #[arg(long = "question", value_name = "QUESTION")]
    pub explicit_question: Option<String>,
    /// Manage the separate Doubao OAuth login
    #[command(subcommand)]
    pub action: Option<AskAction>,
}

/// Assistant management commands, separate from storage authentication.
#[derive(Debug, Subcommand)]
pub enum AskAction {
    /// Manage the separate Doubao OAuth login
    Auth(AuthArgs),
}

/// Manage the separate Doubao OAuth login.
#[derive(Debug, Args)]
pub struct AuthArgs {
    /// Login, inspect local status, or remove local credentials
    // [Review Fix #11] Omission shows group usage, including metadata-only previews.
    #[command(subcommand)]
    pub action: Option<AuthAction>,
}

/// Explicit Doubao OAuth actions.
#[derive(Debug, Subcommand)]
pub enum AuthAction {
    /// Authorize directly with Doubao using a loopback callback
    Login {
        /// Print the authorization URL without opening a browser
        #[arg(long = "no-browser")]
        is_no_browser: bool,
    },
    /// Inspect local expiry only; remote revocation is not checked
    Status,
    /// Remove locally saved Doubao tokens; does not revoke the remote grant
    Logout,
}
