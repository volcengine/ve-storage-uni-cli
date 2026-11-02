//! Adapt canonical MCP command paths to the running executable surface.

use tos_core::agent::error::CliError;

/// Return subprocess arguments for a unified or direct ve-tos invocation.
/// Invalid or missing canonical command paths fail before a process is spawned.
pub(super) fn process_argv(
    command: &str,
    mut argv: Vec<String>,
    is_direct: bool,
) -> Result<Vec<String>, CliError> {
    if !is_direct {
        return Ok(argv);
    }
    let parts = command.split_whitespace().collect::<Vec<_>>();
    if parts.len() < 2 || parts[0] != "ve-tos" {
        return Err(CliError::ValidationError(
            "invalid MCP ve-tos command path".into(),
        ));
    }
    let position = argv
        .windows(parts.len())
        .position(|candidate| {
            candidate
                .iter()
                .map(String::as_str)
                .eq(parts.iter().copied())
        })
        .ok_or_else(|| {
            CliError::ValidationError("MCP command path is missing from prepared arguments".into())
        })?;
    // [Review Fix #TosMcpEntrypoint] Match the full command so profile values named ve-tos remain intact.
    argv.remove(position);
    Ok(argv)
}

#[cfg(test)]
mod tests {
    use super::process_argv;

    #[test]
    fn direct_invocation_removes_only_the_service_command() {
        let argv = [
            "--profile",
            "ve-tos",
            "--output",
            "json",
            "ve-tos",
            "ls",
            "tos://bucket/",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            process_argv("ve-tos ls", argv.clone(), false).unwrap(),
            argv
        );
        let direct = process_argv("ve-tos ls", argv.clone(), true).unwrap();
        assert_eq!(&direct[..4], &argv[..4]);
        assert_eq!(&direct[4..], &argv[5..]);
    }

    #[test]
    fn invalid_command_paths_fail_before_dispatch() {
        for command in ["", "ve-tos", "tos ls", "ve-tos cp"] {
            assert!(process_argv(command, vec!["ve-tos".into(), "ls".into()], true).is_err());
        }
    }
}
