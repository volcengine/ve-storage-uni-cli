use crate::platform::{
    config::{ExpectedEnvironment, Locator},
    AppError,
};
use std::{ffi::OsString, net::SocketAddr, path::PathBuf};
pub(crate) const USAGE: &str = "ask-auth serve --environment boe|prod [--namespace storage.tos.cli] [--space default] [--listen 127.0.0.1:8080]\nask-auth probe --environment boe|prod [--namespace storage.tos.cli] [--space default]\nask-auth migrate --config /absolute/protected/operator.json\nask-auth --help\n";
pub(crate) enum Command {
    Help,
    Serve(BootstrapArgs),
    Probe(BootstrapArgs),
    Migrate(PathBuf),
}
pub(crate) struct BootstrapArgs {
    pub(crate) environment: ExpectedEnvironment,
    pub(crate) locator: Locator,
    pub(crate) listen: SocketAddr,
}
pub(crate) fn parse(arguments: impl IntoIterator<Item = OsString>) -> Result<Command, AppError> {
    let words = arguments
        .into_iter()
        .map(|word| word.into_string().map_err(|_| AppError::Arguments))
        .collect::<Result<Vec<_>, _>>()?;
    if words == ["--help"] {
        return Ok(Command::Help);
    }
    let (command, options) = words.split_first().ok_or(AppError::Arguments)?;
    let options = options_map(options)?;
    match command.as_str() {
        "serve" => bootstrap(options, true).map(Command::Serve),
        "probe" => bootstrap(options, false).map(Command::Probe),
        "migrate" if options.len() == 1 => {
            let path = PathBuf::from(options.get("--config").ok_or(AppError::Arguments)?);
            if !path.is_absolute() {
                return Err(AppError::Arguments);
            }
            Ok(Command::Migrate(path))
        }
        _ => Err(AppError::Arguments),
    }
}
fn options_map(words: &[String]) -> Result<std::collections::BTreeMap<&str, &str>, AppError> {
    let mut options = std::collections::BTreeMap::new();
    let mut pairs = words.chunks_exact(2);
    for pair in &mut pairs {
        if options.insert(pair[0].as_str(), pair[1].as_str()).is_some() {
            return Err(AppError::Arguments);
        }
    }
    if !pairs.remainder().is_empty() {
        return Err(AppError::Arguments);
    }
    Ok(options)
}
fn bootstrap(
    options: std::collections::BTreeMap<&str, &str>,
    is_serve: bool,
) -> Result<BootstrapArgs, AppError> {
    if options.keys().any(|key| {
        // [Review Fix #82] Express the accepted flag set directly so serve-only options remain auditable.
        !(matches!(*key, "--environment" | "--namespace" | "--space")
            || is_serve && *key == "--listen")
    }) {
        return Err(AppError::Arguments);
    }
    let environment = match options.get("--environment") {
        Some(&"boe") => ExpectedEnvironment::Boe,
        Some(&"prod") => ExpectedEnvironment::Production,
        _ => return Err(AppError::Arguments),
    };
    let locator = Locator::new(
        options.get("--namespace").unwrap_or(&"storage.tos.cli"),
        options.get("--space").unwrap_or(&"default"),
        environment,
    )
    .map_err(|_| AppError::Arguments)?;
    let listen = options
        .get("--listen")
        .unwrap_or(&"127.0.0.1:8080")
        .parse()
        .map_err(|_| AppError::Arguments)?;
    Ok(BootstrapArgs {
        environment,
        locator,
        listen,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn command(values: &[&str]) -> Result<Command, AppError> {
        parse(values.iter().map(OsString::from))
    }
    #[test]
    fn explicit_environment_is_required_and_only_known_nonsecret_flags_are_accepted() {
        assert!(matches!(
            command(&["serve", "--environment", "boe"]).unwrap(),
            Command::Serve(_)
        ));
        assert!(matches!(
            command(&["probe", "--environment", "prod"]).unwrap(),
            Command::Probe(_)
        ));
        for values in [
            vec!["serve"],
            vec![
                "probe",
                "--environment",
                "boe",
                "--listen",
                "127.0.0.1:8080",
            ],
            vec![
                "serve",
                "--environment",
                "boe",
                "--client-secret",
                "PRIVATE",
            ],
            vec!["serve", "--environment", "boe", "--environment", "prod"],
        ] {
            assert!(matches!(command(&values), Err(AppError::Arguments)));
        }
    }
    #[test]
    fn static_help_and_separate_migration_have_no_tcc_requirement() {
        assert!(matches!(command(&["--help"]).unwrap(), Command::Help));
        assert!(matches!(
            command(&["migrate", "--config", "/protected/operator.json"]).unwrap(),
            Command::Migrate(_)
        ));
        assert!(command(&["migrate", "--config", "relative.json"]).is_err());
        assert_eq!(
            command(&["https://SECRET.example"])
                .err()
                .unwrap()
                .to_string(),
            "invalid bootstrap arguments"
        );
    }
    #[test]
    #[cfg(unix)]
    fn non_utf8_arguments_are_sanitized() {
        use std::os::unix::ffi::OsStringExt;
        assert!(matches!(
            parse([OsString::from_vec(vec![255])]),
            Err(AppError::Arguments)
        ));
    }
}
