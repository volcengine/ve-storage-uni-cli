//! Bounded share-password input, separate from argument parsing and plans.
use std::io::{BufRead, IsTerminal, Read, Write};
use std::time::Duration;
use tos_core::agent::{error::CliError, global_args::GlobalArgs, output::OutputFormat};
const MAX_PASSWORD_BYTES: usize = 4096;
const PASSWORD_TIMEOUT_SECONDS: u64 = 30;

/// Validate one bounded password without trimming significant spaces.
/// Only one terminal LF or CRLF is stripped; multiple lines and controls are rejected.
pub(crate) fn validate_password(password: &str) -> Result<(), CliError> {
    if password.len() > MAX_PASSWORD_BYTES || password.chars().any(char::is_control) {
        return Err(CliError::ValidationError(
            "share password must be at most 4096 bytes and contain no controls or multiple lines"
                .into(),
        ));
    }
    Ok(())
}
fn normalize_password(mut bytes: Vec<u8>) -> Result<String, CliError> {
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    let password = String::from_utf8(bytes)
        .map_err(|_| CliError::ValidationError("share password must be UTF-8".into()))?;
    validate_password(&password)?;
    Ok(password)
}

/// Read a single bounded password from stdin with a 30 second deadline.
/// The detached reader does not hold Tokio runtime shutdown hostage on stalled input.
/// Returns a validation error for empty normalized input, invalid content or timeout.
pub(crate) async fn read_password_stdin() -> Result<String, CliError> {
    let password = read_password(false).await?;
    // [Review Fix #D05] Explicit EOF/blank input is a missing secret; confirmation lines may still be empty.
    if password.is_empty() {
        return Err(CliError::ValidationError(
            "--password-stdin requires a nonempty password; received empty input".into(),
        ));
    }
    Ok(password)
}
/// Read one ordinary confirmation line with bounded input and a cancellation-safe waiter.
pub(crate) async fn read_confirmation_line() -> Result<String, CliError> {
    read_password(true).await.map_err(|_| {
        CliError::ValidationError("share cancellation confirmation failed or timed out".into())
    })
}
async fn read_password(is_tty: bool) -> Result<String, CliError> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = if is_tty {
            std::io::stdin()
                .lock()
                .take((MAX_PASSWORD_BYTES + 3) as u64)
                .read_until(b'\n', &mut bytes)
        } else {
            std::io::stdin()
                .lock()
                .take((MAX_PASSWORD_BYTES + 3) as u64)
                .read_to_end(&mut bytes)
        };
        let result = result
            .map_err(CliError::Io)
            .and_then(|_| normalize_password(bytes));
        // Timeout or cancellation closes the receiver; dropping the password is intentional.
        let _ = sender.send(result);
    });
    tokio::time::timeout(Duration::from_secs(PASSWORD_TIMEOUT_SECONDS), receiver)
        .await
        .map_err(|_| {
            CliError::ValidationError("share password stdin timed out after 30 seconds".into())
        })?
        .map_err(|_| CliError::ValidationError("share password reader stopped".into()))?
}

/// Check whether an ordinary interactive output session may prompt for a hidden password.
pub(crate) fn can_prompt(global: &GlobalArgs) -> bool {
    !global.quiet
        && !matches!(
            global.output.unwrap_or_else(OutputFormat::auto_detect),
            OutputFormat::Json
        )
        && std::io::stdin().is_terminal()
        && std::io::stderr().is_terminal()
}

#[cfg(unix)]
struct EchoGuard {
    saved: Option<rustix::termios::Termios>,
}
#[cfg(unix)]
impl EchoGuard {
    fn hide() -> Result<Self, CliError> {
        let stdin = std::io::stdin();
        let saved = rustix::termios::tcgetattr(&stdin).map_err(std::io::Error::from)?;
        let mut hidden = saved.clone();
        hidden
            .local_modes
            .remove(rustix::termios::LocalModes::ECHO | rustix::termios::LocalModes::ECHONL);
        rustix::termios::tcsetattr(&stdin, rustix::termios::OptionalActions::Now, &hidden)
            .map_err(std::io::Error::from)?;
        Ok(Self { saved: Some(saved) })
    }
    fn restore(&mut self) -> Result<(), CliError> {
        if let Some(saved) = &self.saved {
            rustix::termios::tcsetattr(
                std::io::stdin(),
                rustix::termios::OptionalActions::Now,
                saved,
            )
            .map_err(std::io::Error::from)?;
            self.saved = None;
        }
        Ok(())
    }
}
#[cfg(unix)]
impl Drop for EchoGuard {
    fn drop(&mut self) {
        if let Err(_error) = self.restore() {
            eprintln!("Failed to restore terminal echo; reset the terminal before continuing.");
        }
    }
}

/// Prompt once with echo disabled, restoring the terminal after success, failure or timeout.
pub(crate) async fn prompt_password(global: &GlobalArgs) -> Result<String, CliError> {
    if !can_prompt(global) {
        return Err(CliError::ValidationError(
            "share password required; use --password-stdin or the share URL pwd query".into(),
        ));
    }
    #[cfg(unix)]
    {
        let mut guard = EchoGuard::hide()?;
        eprint!("Share password: ");
        std::io::stderr().flush()?;
        let result = read_password(true).await;
        guard.restore()?;
        eprintln!();
        result
    }
    #[cfg(not(unix))]
    {
        Err(CliError::ValidationError(
            "hidden password prompting is unavailable; use --password-stdin".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn password_preserves_spaces_and_strips_one_terminal_newline() {
        assert_eq!(
            normalize_password(b"  secret  \r\n".to_vec()).unwrap(),
            "  secret  "
        );
        assert_eq!(normalize_password(b"\n".to_vec()).unwrap(), "");
    }
    #[test]
    fn password_rejects_multiple_lines_controls_and_oversize() {
        for bytes in [b"a\nb\n".to_vec(), b"a\0b".to_vec(), vec![b'a'; 4097]] {
            assert!(normalize_password(bytes).is_err());
        }
    }
}
