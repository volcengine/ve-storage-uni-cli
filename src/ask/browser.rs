use std::{process::Stdio, time::Duration};
use tos_core::agent::error::CliError;

const OPEN_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(any(windows, test))]
mod windows;

#[derive(Debug, PartialEq, Eq)]
enum Backend {
    Process(&'static str),
    WindowsShell,
}

fn backend_for(os: &str) -> Backend {
    match os {
        "windows" => Backend::WindowsShell,
        "macos" => Backend::Process("open"),
        _ => Backend::Process("xdg-open"),
    }
}

fn failure() -> CliError {
    CliError::Unknown("browser could not be opened; retry ask auth login --no-browser".into())
}

pub(super) async fn open(url: &str) -> Result<(), CliError> {
    match backend_for(std::env::consts::OS) {
        Backend::Process(program) => run_launcher(launcher(program, url), OPEN_TIMEOUT).await,
        Backend::WindowsShell => {
            #[cfg(windows)]
            {
                open_windows(url).await
            }
            #[cfg(not(windows))]
            {
                Err(failure())
            }
        }
    }
}

#[cfg(windows)]
async fn open_windows(url: &str) -> Result<(), CliError> {
    let target = tos_core::ask::Secret::new(url.to_owned());
    let (sender, receiver) = tokio::sync::oneshot::channel();
    // [Review Fix #1] A dedicated STA thread isolates COM and cannot block Tokio runtime shutdown after timeout.
    std::thread::Builder::new()
        .name("doubao-browser".into())
        .spawn(move || {
            let is_opened = windows::open(target.expose());
            // A receiver may have timed out; its failure has already been reported.
            let _ = sender.send(is_opened);
        })
        .map_err(|_| failure())?;
    wait_for_shell(receiver, OPEN_TIMEOUT).await
}

#[cfg(any(windows, test))]
async fn wait_for_shell(
    receiver: tokio::sync::oneshot::Receiver<bool>,
    timeout: Duration,
) -> Result<(), CliError> {
    match tokio::time::timeout(timeout, receiver).await {
        Ok(Ok(true)) => Ok(()),
        _ => Err(failure()),
    }
}

fn launcher(program: &str, url: &str) -> tokio::process::Command {
    let mut process = tokio::process::Command::new(program);
    process
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    process
}

async fn run_launcher(
    mut process: tokio::process::Command,
    timeout: Duration,
) -> Result<(), CliError> {
    let mut child = process.spawn().map_err(|_| failure())?;
    let status = tokio::time::timeout(timeout, child.wait())
        .await
        .map_err(|_| failure())?
        .map_err(|_| failure())?;
    if !status.success() {
        return Err(failure());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_uses_the_system_shell_instead_of_a_unix_opener() {
        assert_eq!(backend_for("windows"), Backend::WindowsShell);
        assert_eq!(backend_for("macos"), Backend::Process("open"));
        assert_eq!(backend_for("linux"), Backend::Process("xdg-open"));
    }

    #[test]
    fn authorization_url_remains_one_opaque_argument() {
        let url = "https://accounts.doubao.com/authorize?state=a&scope=b%20c&redirect_uri=http%3A%2F%2Flocalhost";
        let process = launcher("synthetic-opener", url);
        let arguments: Vec<_> = process.as_std().get_args().collect();
        assert_eq!(arguments, vec![std::ffi::OsStr::new(url)]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn launcher_success_does_not_wait_for_the_browser_to_close() {
        assert!(
            run_launcher(launcher("/usr/bin/true", "ignored"), Duration::from_secs(1))
                .await
                .is_ok()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn missing_and_failed_launchers_return_manual_login_recovery() {
        for program in ["/nonexistent/doubao-opener", "/usr/bin/false"] {
            let error = run_launcher(
                launcher(program, "private-auth-url"),
                Duration::from_secs(1),
            )
            .await
            .unwrap_err();
            assert!(error.to_string().contains("--no-browser"));
            assert!(!error.to_string().contains("private-auth-url"));
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_stuck_launcher_times_out() {
        let error = run_launcher(launcher("/bin/sleep", "5"), Duration::from_millis(20))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("--no-browser"));
    }

    #[tokio::test]
    async fn shell_completion_propagates_success_failure_and_worker_exit() {
        for is_opened in [true, false] {
            let (sender, receiver) = tokio::sync::oneshot::channel();
            sender.send(is_opened).unwrap();
            assert_eq!(
                wait_for_shell(receiver, Duration::from_secs(1))
                    .await
                    .is_ok(),
                is_opened
            );
        }
        let (sender, receiver) = tokio::sync::oneshot::channel();
        drop(sender);
        assert!(wait_for_shell(receiver, Duration::from_secs(1))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn shell_timeout_returns_without_waiting_for_the_worker() {
        let (_sender, receiver) = tokio::sync::oneshot::channel();
        assert!(wait_for_shell(receiver, Duration::from_millis(10))
            .await
            .is_err());
    }
}
