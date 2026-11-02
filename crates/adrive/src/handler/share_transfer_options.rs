//! Resolve local transfer controls without loading receiver credentials or changing files.
use crate::cli::{
    high_level::{OverwriteStrategy, ProgressGranularity},
    share::ShareDownloadArgs,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tos_core::{
    agent::{error::CliError, global_args::GlobalArgs},
    infra::config::{AdriveOverride, ConfigFile},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SelectionPolicy {
    pub is_include_parent: bool,
    pub include: Option<String>,
    pub exclude: Option<String>,
    pub overwrite: String,
}

impl SelectionPolicy {
    pub(super) fn from_args(args: &ShareDownloadArgs) -> Self {
        Self {
            is_include_parent: args.is_include_parent,
            include: args.include.clone(),
            exclude: args.exclude.clone(),
            overwrite: match args.overwrite_strategy.unwrap_or(OverwriteStrategy::Force) {
                OverwriteStrategy::Force => "force",
                OverwriteStrategy::NoClobber => "no-clobber",
                OverwriteStrategy::Newer => "newer",
            }
            .into(),
        }
    }
}

/// Resolve CLI > local transfer profile > scoped environment > cp defaults.
/// Does not load the credentials store, create artifacts, or select a receiver API.
pub(super) fn resolve(
    global: &GlobalArgs,
    args: &ShareDownloadArgs,
) -> Result<ShareDownloadArgs, CliError> {
    let profile = local_profile(global)?;
    let mut resolved = args.clone();
    resolved.destination_dir = super::high_level::expand_user_path(&args.destination_dir)
        .to_string_lossy()
        .into();
    resolved.batch_concurrency = Some(positive(
        args.batch_concurrency,
        profile.batch_concurrency,
        "BATCH_CONCURRENCY",
        16,
    )?);
    resolved.list_concurrency = Some(positive(
        args.list_concurrency,
        profile.list_concurrency,
        "LIST_CONCURRENCY",
        4,
    )?);
    resolved.multipart_concurrency = Some(positive(
        args.multipart_concurrency,
        profile.multipart_concurrency,
        "MULTIPART_CONCURRENCY",
        4,
    )?);
    resolved.checkpoint_threshold = args
        .checkpoint_threshold
        .clone()
        .or(profile.checkpoint_threshold.clone())
        .or_else(|| environment("CHECKPOINT_THRESHOLD"));
    resolved.progress_granularity = Some(granularity(args.progress_granularity, &profile)?);
    if !args.is_progress && !args.is_no_progress {
        resolved.is_no_progress = !progress_setting(
            profile.progress_enabled,
            environment("PROGRESS_ENABLED").as_deref(),
        )?;
    }
    resolved.overwrite_strategy = Some(overwrite(args, &profile)?);
    resolved.is_overwrite = resolved.overwrite_strategy != Some(OverwriteStrategy::NoClobber);
    resolved.checkpoint = checkpoint(global, &resolved, &profile)?;
    validate(&resolved)?;
    Ok(resolved)
}

/// Read only transfer settings from the chosen profile, without decoding credentials or validating unused API/auth fields.
pub(super) fn local_profile(global: &GlobalArgs) -> Result<AdriveOverride, CliError> {
    let config = ConfigFile::load_from(&global.existing_runtime_config_path()?)?;
    let shared = config
        .profiles
        .get(&global.profile)
        .cloned()
        .unwrap_or_default();
    let mut profile = shared.adrive.clone().unwrap_or_default();
    profile.batch_concurrency = profile.batch_concurrency.or(shared.batch_concurrency);
    profile.list_concurrency = profile.list_concurrency.or(shared.list_concurrency);
    profile.multipart_concurrency = profile
        .multipart_concurrency
        .or(shared.multipart_concurrency);
    profile.checkpoint_threshold = profile.checkpoint_threshold.or(shared.checkpoint_threshold);
    profile.checkpoint_dir = profile.checkpoint_dir.or(shared.checkpoint_dir);
    profile.batch_report_dir = profile.batch_report_dir.or(shared.batch_report_dir);
    profile.batch_report_format = profile.batch_report_format.or(shared.batch_report_format);
    profile.progress_enabled = profile.progress_enabled.or(shared.progress_enabled);
    profile.progress_granularity = profile.progress_granularity.or(shared.progress_granularity);
    profile.overwrite_strategy = profile.overwrite_strategy.or(shared.overwrite_strategy);
    Ok(profile)
}

/// Reject invalid local controls before any password input or remote requests.
pub(super) fn validate(args: &ShareDownloadArgs) -> Result<(), CliError> {
    for (name, value) in [
        ("batch-concurrency", args.batch_concurrency),
        ("list-concurrency", args.list_concurrency),
        ("multipart-concurrency", args.multipart_concurrency),
    ] {
        if value == Some(0) {
            return Err(invalid(&format!("--{name} must be positive")));
        }
    }
    if args.is_no_manifest && args.manifest_path.is_some() {
        return Err(invalid("--no-manifest conflicts with --manifest-path"));
    }
    for pattern in [&args.include, &args.exclude].into_iter().flatten() {
        if pattern.chars().any(char::is_control) {
            return Err(invalid(
                "transfer patterns must not contain control characters",
            ));
        }
    }
    // [Review Fix #ShareParity2] Validate resolved profile values in read-only plans too.
    super::share_content::ContentRuntime::for_args(args)?;
    Ok(())
}

fn environment(name: &str) -> Option<String> {
    std::env::var(format!("ADRIVE_{name}")).ok()
}
fn progress_setting(configured: Option<bool>, environment: Option<&str>) -> Result<bool, CliError> {
    // [Review Fix #ShareParity12] Ignore invalid lower-priority settings when the profile already supplies a value.
    match configured {
        Some(value) => Ok(value),
        None => environment
            .map(|value| {
                value
                    .parse::<bool>()
                    .map_err(|_| invalid("progress_enabled must be true or false"))
            })
            .transpose()
            .map(|value| value.unwrap_or(true)),
    }
}
fn invalid(message: &str) -> CliError {
    CliError::ValidationError(message.into())
}

/// Apply cp's overwrite/skip policy to an already validated ordinary local file.
pub(super) fn skip_existing(
    args: &ShareDownloadArgs,
    source_millis: i64,
    metadata: &std::fs::Metadata,
) -> Result<bool, CliError> {
    match args.overwrite_strategy.unwrap_or(OverwriteStrategy::Force) {
        OverwriteStrategy::Force => Ok(false),
        OverwriteStrategy::NoClobber => Ok(true),
        OverwriteStrategy::Newer => {
            let modified = metadata
                .modified()?
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| invalid("local timestamp predates epoch"))?
                .as_millis();
            // [Review Fix #ShareParity15] cp treats nonpositive remote timestamps as older, avoiding a signed-to-unsigned wrap or unintended overwrite.
            Ok(source_millis <= 0 || source_millis as u128 <= modified)
        }
    }
}

fn positive(
    cli: Option<usize>,
    configured: Option<usize>,
    name: &str,
    default: usize,
) -> Result<usize, CliError> {
    let value = match cli.or(configured) {
        Some(value) => value,
        None => environment(name)
            .map(|value| {
                value
                    .parse()
                    .map_err(|_| invalid("invalid transfer concurrency"))
            })
            .transpose()?
            .unwrap_or(default),
    };
    if value == 0 {
        return Err(invalid("transfer concurrency must be positive"));
    }
    Ok(value)
}

fn granularity(
    cli: Option<ProgressGranularity>,
    profile: &AdriveOverride,
) -> Result<ProgressGranularity, CliError> {
    if let Some(value) = cli {
        return Ok(value);
    }
    let configured = profile
        .progress_granularity
        .clone()
        .or_else(|| environment("PROGRESS_GRANULARITY"));
    match configured
        .as_deref()
        .unwrap_or("part")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "part" | "parts" => Ok(ProgressGranularity::Part),
        "byte" | "bytes" => Ok(ProgressGranularity::Byte),
        _ => Err(invalid("progress-granularity must be part or byte")),
    }
}

fn overwrite(
    args: &ShareDownloadArgs,
    profile: &AdriveOverride,
) -> Result<OverwriteStrategy, CliError> {
    if let Some(value) = args.overwrite_strategy {
        return Ok(value);
    }
    if args.is_no_clobber {
        return Ok(OverwriteStrategy::NoClobber);
    }
    if args.is_force || args.is_overwrite {
        return Ok(OverwriteStrategy::Force);
    }
    let configured = profile
        .overwrite_strategy
        .clone()
        .or_else(|| environment("OVERWRITE_STRATEGY"));
    match configured
        .as_deref()
        .unwrap_or("force")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "force" | "overwrite" => Ok(OverwriteStrategy::Force),
        "no-clobber" | "no_clobber" | "skip-existing" | "skip_existing" => {
            Ok(OverwriteStrategy::NoClobber)
        }
        "newer" | "if-newer" | "if_newer" => Ok(OverwriteStrategy::Newer),
        _ => Err(invalid(
            "overwrite-strategy must be force, no-clobber or newer",
        )),
    }
}

fn checkpoint(
    global: &GlobalArgs,
    args: &ShareDownloadArgs,
    profile: &AdriveOverride,
) -> Result<Option<String>, CliError> {
    let Some(selected) = args.checkpoint.as_deref() else {
        return Ok(None);
    };
    if selected != "__auto__" {
        return Ok(Some(
            super::high_level::expand_user_path(selected)
                .to_string_lossy()
                .into(),
        ));
    }
    let origin = crate::domain::share_url::parse_share_url(&args.share_url)
        .map_err(super::common::map_ids_error)?;
    let destination =
        super::share_plan_local::resolve_directory(std::path::Path::new(&args.destination_dir))?;
    let fingerprint = serde_json::to_vec(&(
        origin.safe_url,
        &args.path,
        destination,
        &global.profile,
        &global.endpoint,
        args.is_recursive,
        SelectionPolicy::from_args(args),
    ))?;
    let name = format!(
        "share-download-{}.json",
        &hex::encode(Sha256::digest(fingerprint))[..32]
    );
    let folder = args
        .checkpoint_dir
        .clone()
        .or(profile.checkpoint_dir.clone())
        .or_else(|| environment("CHECKPOINT_DIR"))
        .unwrap_or_else(|| "~/.tos/checkpoints/ve-adrive".into());
    let path: PathBuf = super::high_level::expand_user_path(&folder).join(name);
    Ok(Some(path.to_string_lossy().into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn progress_profile_overrides_an_invalid_environment_value() {
        assert!(!progress_setting(Some(false), Some("invalid")).unwrap());
        assert!(progress_setting(Some(true), Some("invalid")).unwrap());
        assert!(progress_setting(None, Some("invalid")).is_err());
        assert!(!progress_setting(None, Some("false")).unwrap());
        assert!(progress_setting(None, None).unwrap());
    }
    #[test]
    fn newer_skips_nonpositive_timestamps_and_only_overwrites_newer_sources() {
        let settings = SettingsDirectory::new("");
        let metadata = std::fs::metadata(settings.0.join("config.toml")).unwrap();
        let args = ShareDownloadArgs {
            overwrite_strategy: Some(OverwriteStrategy::Newer),
            ..Default::default()
        };
        for timestamp in [-1, 0, 1] {
            assert!(skip_existing(&args, timestamp, &metadata).unwrap());
        }
        assert!(!skip_existing(&args, i64::MAX, &metadata).unwrap());
    }
    struct SettingsDirectory(PathBuf);
    impl SettingsDirectory {
        fn new(settings: &str) -> Self {
            let directory =
                std::env::temp_dir().join(format!("share-settings-{}", ulid::Ulid::new()));
            std::fs::create_dir(&directory).unwrap();
            std::fs::write(directory.join("config.toml"), settings).unwrap();
            Self(directory)
        }
        fn global(&self) -> GlobalArgs {
            GlobalArgs {
                config_path: Some(self.0.join("config.toml")),
                credentials_path: Some(self.0.join("missing-credentials")),
                ..GlobalArgs::default()
            }
        }
    }
    impl Drop for SettingsDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn invalid_resolved_threshold_and_bandwidth_are_rejected_before_planning() {
        let settings = SettingsDirectory::new("[default.adrive]\ncheckpoint_threshold='0'\n");
        let args = ShareDownloadArgs::default();
        assert!(resolve(&settings.global(), &args).is_err());
        let settings = SettingsDirectory::new("[default]\ncheckpoint_threshold='invalid'\n");
        assert!(resolve(&settings.global(), &args).is_err());
        let settings = SettingsDirectory::new("");
        let args = ShareDownloadArgs {
            bandwidth_limit: Some("invalid".into()),
            ..args
        };
        assert!(resolve(&settings.global(), &args).is_err());
        assert!(!settings.0.join("missing-credentials").exists());
    }
    #[test]
    fn local_profile_uses_scoped_then_shared_and_explicit_controls() {
        let settings = SettingsDirectory::new("[default]\nbatch_concurrency=3\nlist_concurrency=2\nprogress_enabled=false\n[default.adrive]\nbatch_concurrency=5\noverwrite_strategy='no-clobber'\n");
        let args = ShareDownloadArgs {
            batch_concurrency: Some(7),
            is_progress: true,
            ..Default::default()
        };
        let resolved = resolve(&settings.global(), &args).unwrap();
        assert_eq!(resolved.batch_concurrency, Some(7));
        assert_eq!(resolved.list_concurrency, Some(2));
        assert!(!resolved.is_no_progress);
        assert_eq!(
            resolved.overwrite_strategy,
            Some(OverwriteStrategy::NoClobber)
        );
        let resolved = resolve(&settings.global(), &ShareDownloadArgs::default()).unwrap();
        assert_eq!(resolved.batch_concurrency, Some(5));
        assert!(resolved.is_no_progress);
        assert!(!settings.0.join("missing-credentials").exists());
    }
}
