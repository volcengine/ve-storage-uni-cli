/*
 * Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 * http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

pub mod high_level;
pub mod low_level;
pub mod meta;

use clap::Subcommand;
use tos_core::agent::global_args::GROUPED_HELP_GLOBAL_OPTIONS;

#[derive(Debug, Subcommand)]
pub enum TosCommand {
    // ─── High-Level Commands ───────────────────────────────
    /// Copy local files, TOS objects, or prefixes
    Cp(high_level::CpArgs),
    /// Move files or objects by copy plus source delete
    Mv(high_level::MvArgs),
    /// Synchronize source and destination incrementally
    Sync(high_level::SyncArgs),
    /// Create a bucket
    Mb(high_level::MbArgs),
    /// Remove a bucket
    Rb(high_level::RbArgs),
    /// Create a folder
    Mkdir(high_level::MkdirArgs),
    /// Delete objects or prefixes
    Rm(high_level::RmArgs),
    /// List buckets or objects
    Ls(high_level::LsArgs),
    /// Show bucket or object metadata
    Stat(high_level::StatArgs),
    /// Calculate object size statistics for a prefix
    Du(high_level::DuArgs),
    /// Find objects by name, size, mtime, or storage class
    Find(high_level::FindArgs),
    /// Stream object content
    Cat(high_level::CatArgs),
    /// Upload stdin to an object
    Put(high_level::PutArgs),
    /// Generate presigned URL
    Presign(high_level::PresignArgs),
    /// Restore archived object
    Restore(high_level::RestoreArgs),

    // ─── Low-Level: Core Operations ────────────────────────
    /// Bucket core APIs
    Bucket(low_level::BucketCommand),
    /// Object core APIs
    Object(low_level::ObjectCommand),
    /// Multipart upload core APIs
    Multipart(low_level::MultipartCommand),
    /// Turbo core APIs
    Turbo(low_level::TurboCommand),

    // ─── Low-Level: Bucket Configuration ───────────────────
    /// Bucket storage quota
    Quota(low_level::QuotaCommand),
    /// Bucket policy management
    Policy(low_level::PolicyCommand),
    /// Lifecycle rule management
    Lifecycle(low_level::LifecycleCommand),
    /// Bucket default storage class
    /// [Review Fix #M5] Variant renamed to `Storageclass`; the legacy spelling
    /// `storgeclass` is kept as a clap alias to preserve backward compatibility
    /// for existing scripts and skill manifests.
    #[command(name = "storageclass", alias = "storgeclass")]
    Storageclass(low_level::StorageclassCommand),
    /// CORS configuration
    Cors(low_level::CorsCommand),
    /// Versioning configuration
    Versioning(low_level::VersioningCommand),
    /// Cross-region replication
    Replication(low_level::ReplicationCommand),
    /// Server-side encryption configuration
    Encryption(low_level::EncryptionCommand),
    /// Custom domain binding
    #[command(name = "custom-domain")]
    CustomDomain(low_level::CustomDomainCommand),
    /// Event notification configuration
    Notification(low_level::NotificationCommand),
    /// Static website hosting
    Website(low_level::WebsiteCommand),
    /// Mirror back-to-source rules
    Mirror(low_level::MirrorCommand),
    /// Bucket inventory configuration
    Inventory(low_level::InventoryCommand),
    /// Bucket tagging management
    Tagging(low_level::TaggingCommand),
    /// Bucket ACL management
    Acl(low_level::AclCommand),
    /// Bucket rename configuration
    Rename(low_level::RenameCommand),
    /// Real-time log analysis
    #[command(name = "real-time-log")]
    RealTimeLog(low_level::RealTimeLogCommand),
    /// Access monitoring configuration
    #[command(name = "access-monitor")]
    AccessMonitor(low_level::AccessMonitorCommand),
    /// WORM / object lock configuration
    Worm(low_level::WormCommand),
    /// Bucket trash configuration
    Trash(low_level::TrashCommand),
    /// Requester pays configuration
    Payment(low_level::PaymentCommand),
    /// Access log storage configuration
    Logging(low_level::LoggingCommand),
    /// Intelligent tiering configuration
    #[command(name = "intelligent-tiering")]
    IntelligentTiering(low_level::IntelligentTieringCommand),
    /// Transfer acceleration configuration
    #[command(name = "transfer-acceleration")]
    TransferAcceleration(low_level::TransferAccelerationCommand),
    /// CDN notification configuration
    #[command(name = "cdn-notification")]
    CdnNotification(low_level::CdnNotificationCommand),
    /// HTTPS/TLS configuration
    #[command(name = "https-config")]
    HttpsConfig(low_level::HttpsConfigCommand),
    /// Pay-by-traffic configuration
    #[command(name = "pay-by-traffic")]
    PayByTraffic(low_level::PayByTrafficCommand),
    /// Max-age cache configuration
    #[command(name = "max-age")]
    MaxAge(low_level::MaxAgeCommand),
    /// Data redundancy transition
    #[command(name = "redundancy-transition")]
    RedundancyTransition(low_level::RedundancyTransitionCommand),

    // ─── Low-Level: Advanced Features ──────────────────────
    /// Data processing (image styles, workflows, audits)
    #[command(name = "data-process")]
    DataProcess(low_level::DataProcessCommand),
    /// Object set management
    #[command(name = "object-set")]
    ObjectSet(low_level::ObjectSetCommand),
    /// Accelerator management
    Accelerator(low_level::AcceleratorCommand),
    /// Multi-region access point
    Mrap(low_level::MrapCommand),
    /// Access point management
    Ap(low_level::ApCommand),
    /// Converged access point
    Cap(low_level::CapCommand),
    /// Intelligent retrieval / dataset management
    Dataset(low_level::DatasetCommand),
    /// Control plane operations
    Control(low_level::ControlCommand),

    // ─── Meta / Utilities ──────────────────────────────────
    /// Discover CLI capabilities
    Capabilities(meta::CapabilitiesArgs),
    /// Raw API passthrough
    Api(meta::ApiArgs),
    /// Configuration management
    Config(meta::ConfigCommand),
    /// Generate shell completion
    Completion(meta::CompletionArgs),
    /// Start MCP server
    Serve(meta::ServeArgs),
    /// Manage/export skill metadata
    Skill(meta::SkillCommand),
    /// Environment diagnostics
    Doctor(meta::DoctorArgs),
}

/// Print grouped help output for the `tos` tool.
pub fn print_grouped_help() {
    let help_text = crate::registry::grouped_help_text(GROUPED_HELP_GLOBAL_OPTIONS);
    print!("{}", help_text);
}

/// [Spec §4/§5 — Controlled Output / Deterministic Errors] Map a parsed
/// `TosCommand` (or the absence of one) to a stable, user-facing command path
/// like `"ve-tos config show"`. This is what every error envelope's
/// `command` field carries, so it MUST be:
///
///   - free of Rust enum / struct debug syntax (no `Some(Config(...))`),
///   - stable across refactors (don't surface internal type renames),
///   - copy-paste-runnable as a CLI invocation.
///
/// We deliberately keep this mapping conservative: only the resolved subcommand
/// path is returned, never raw argv. Subcommand actions (e.g. `config show`)
/// are looked up where possible; for variants whose action enums we don't want
/// to inspect, we surface the group prefix only — still better than the old
/// `format!("{:?}", command)` which leaked the entire enum tree.
pub fn command_path(command: &Option<TosCommand>) -> String {
    let Some(command) = command else {
        return "ve-tos".to_string();
    };
    let suffix: &str = match command {
        // High-level
        TosCommand::Cp(_) => "cp",
        TosCommand::Mv(_) => "mv",
        TosCommand::Sync(_) => "sync",
        TosCommand::Mb(_) => "mb",
        TosCommand::Rb(_) => "rb",
        TosCommand::Mkdir(_) => "mkdir",
        TosCommand::Rm(_) => "rm",
        TosCommand::Ls(_) => "ls",
        TosCommand::Stat(_) => "stat",
        TosCommand::Du(_) => "du",
        TosCommand::Find(_) => "find",
        TosCommand::Cat(_) => "cat",
        TosCommand::Put(_) => "put",
        TosCommand::Presign(_) => "presign",
        TosCommand::Restore(_) => "restore",
        // Low-level core — resolve to action level so the error envelope
        // surfaces e.g. `ve-tos object upload` rather than the noisier `ve-tos object`.
        TosCommand::Bucket(cmd) => return bucket_command_path(cmd),
        TosCommand::Object(cmd) => return object_command_path(cmd),
        TosCommand::Multipart(cmd) => return multipart_command_path(cmd),
        TosCommand::Turbo(cmd) => return turbo_command_path(cmd),
        // Bucket configuration
        TosCommand::Quota(cmd) => return option_action_command_path("quota", &cmd.action),
        TosCommand::Policy(cmd) => return option_action_command_path("policy", &cmd.action),
        TosCommand::Lifecycle(cmd) => return option_action_command_path("lifecycle", &cmd.action),
        TosCommand::Storageclass(cmd) => {
            return option_action_command_path("storageclass", &cmd.action)
        }
        TosCommand::Cors(cmd) => return option_action_command_path("cors", &cmd.action),
        TosCommand::Versioning(cmd) => {
            return option_action_command_path("versioning", &cmd.action)
        }
        TosCommand::Replication(cmd) => {
            return option_action_command_path("replication", &cmd.action)
        }
        TosCommand::Encryption(cmd) => {
            return option_action_command_path("encryption", &cmd.action)
        }
        TosCommand::CustomDomain(cmd) => {
            return option_action_command_path("custom-domain", &cmd.action)
        }
        TosCommand::Notification(cmd) => {
            return option_action_command_path("notification", &cmd.action)
        }
        TosCommand::Website(cmd) => return option_action_command_path("website", &cmd.action),
        TosCommand::Mirror(cmd) => return option_action_command_path("mirror", &cmd.action),
        TosCommand::Inventory(cmd) => return option_action_command_path("inventory", &cmd.action),
        TosCommand::Tagging(cmd) => return option_action_command_path("tagging", &cmd.action),
        TosCommand::Acl(cmd) => return option_action_command_path("acl", &cmd.action),
        TosCommand::Rename(cmd) => return option_action_command_path("rename", &cmd.action),
        TosCommand::RealTimeLog(cmd) => {
            return option_action_command_path("real-time-log", &cmd.action)
        }
        TosCommand::AccessMonitor(cmd) => {
            return option_action_command_path("access-monitor", &cmd.action)
        }
        TosCommand::Worm(cmd) => return option_action_command_path("worm", &cmd.action),
        TosCommand::Trash(cmd) => return option_action_command_path("trash", &cmd.action),
        TosCommand::Payment(cmd) => return option_action_command_path("payment", &cmd.action),
        TosCommand::Logging(cmd) => return option_action_command_path("logging", &cmd.action),
        TosCommand::IntelligentTiering(cmd) => {
            return option_action_command_path("intelligent-tiering", &cmd.action)
        }
        TosCommand::TransferAcceleration(cmd) => {
            return option_action_command_path("transfer-acceleration", &cmd.action)
        }
        TosCommand::CdnNotification(cmd) => {
            return option_action_command_path("cdn-notification", &cmd.action)
        }
        TosCommand::HttpsConfig(cmd) => {
            return option_action_command_path("https-config", &cmd.action)
        }
        TosCommand::PayByTraffic(cmd) => {
            return option_action_command_path("pay-by-traffic", &cmd.action)
        }
        TosCommand::MaxAge(cmd) => return option_action_command_path("max-age", &cmd.action),
        TosCommand::RedundancyTransition(cmd) => {
            return option_action_command_path("redundancy-transition", &cmd.action)
        }
        // Advanced
        TosCommand::DataProcess(cmd) => {
            return option_action_command_path("data-process", &cmd.action)
        }
        TosCommand::ObjectSet(cmd) => return option_action_command_path("object-set", &cmd.action),
        TosCommand::Accelerator(cmd) => {
            return option_action_command_path("accelerator", &cmd.action)
        }
        TosCommand::Mrap(cmd) => return option_action_command_path("mrap", &cmd.action),
        TosCommand::Ap(cmd) => return option_action_command_path("ap", &cmd.action),
        TosCommand::Cap(cmd) => return option_action_command_path("cap", &cmd.action),
        TosCommand::Dataset(cmd) => return option_action_command_path("dataset", &cmd.action),
        TosCommand::Control(cmd) => return option_action_command_path("control", &cmd.action),
        // Meta — append the action where its identity is uncontroversial,
        // otherwise fall back to the group name only.
        TosCommand::Capabilities(_) => "capabilities",
        TosCommand::Api(_) => "api",
        TosCommand::Config(cmd) => return config_command_path(cmd),
        TosCommand::Completion(_) => "completion",
        TosCommand::Serve(_) => "serve",
        TosCommand::Skill(cmd) => return skill_command_path(cmd),
        TosCommand::Doctor(_) => "doctor",
    };
    format!("ve-tos {suffix}")
}

fn config_command_path(cmd: &meta::ConfigCommand) -> String {
    match &cmd.action {
        Some(meta::ConfigAction::Init { .. }) => "ve-tos config init".to_string(),
        Some(meta::ConfigAction::Show) => "ve-tos config show".to_string(),
        Some(meta::ConfigAction::Set { .. }) => "ve-tos config set".to_string(),
        None => "ve-tos config".to_string(),
    }
}

fn skill_command_path(cmd: &meta::SkillCommand) -> String {
    match &cmd.action {
        meta::SkillAction::List { .. } => "ve-tos skill list".to_string(),
        meta::SkillAction::Export { .. } => "ve-tos skill export".to_string(),
    }
}

fn option_action_command_path<T: std::fmt::Debug>(group: &str, action: &Option<T>) -> String {
    let Some(action) = action else {
        return format!("ve-tos {group}");
    };
    // [Review Fix #23] Keep error-envelope command names action-level without
    // leaking Rust Debug payloads. We only inspect the parsed enum variant name
    // (e.g. `DeleteLifecycle`) and convert it to the clap kebab-case command.
    let debug = format!("{action:?}");
    let variant = debug.split('(').next().unwrap_or(debug.as_str());
    format!("ve-tos {group} {}", pascal_to_kebab(variant))
}

fn pascal_to_kebab(value: &str) -> String {
    let mut output = String::new();
    let mut previous_was_lower_or_digit = false;
    for ch in value.chars() {
        if ch.is_ascii_uppercase() {
            if previous_was_lower_or_digit && !output.is_empty() {
                output.push('-');
            }
            output.push(ch.to_ascii_lowercase());
            previous_was_lower_or_digit = false;
        } else {
            output.push(ch);
            previous_was_lower_or_digit = ch.is_ascii_lowercase() || ch.is_ascii_digit();
        }
    }
    output
}

fn bucket_command_path(cmd: &low_level::BucketCommand) -> String {
    use low_level::BucketAction::*;
    match cmd.action.as_ref() {
        Some(Create(_)) => "ve-tos bucket create",
        Some(Head(_)) => "ve-tos bucket head",
        Some(Delete(_)) => "ve-tos bucket delete",
        Some(List(_)) => "ve-tos bucket list",
        Some(Stat(_)) => "ve-tos bucket stat",
        Some(Info(_)) => "ve-tos bucket info",
        Some(Location(_)) => "ve-tos bucket location",
        None => "ve-tos bucket",
    }
    .to_string()
}

fn object_command_path(cmd: &low_level::ObjectCommand) -> String {
    use low_level::ObjectAction::*;
    match cmd.action.as_ref() {
        Some(Upload(_)) => "ve-tos object upload",
        Some(Download(_)) => "ve-tos object download",
        Some(FormUpload(_)) => "ve-tos object form-upload",
        Some(Copy(_)) => "ve-tos object copy",
        Some(Delete(_)) => "ve-tos object delete",
        Some(BatchDelete(_)) => "ve-tos object batch-delete",
        Some(List(_)) => "ve-tos object list",
        Some(ListVersions(_)) => "ve-tos object list-versions",
        Some(Head(_)) => "ve-tos object head",
        Some(Stat(_)) => "ve-tos object stat",
        Some(SetMeta(_)) => "ve-tos object set-meta",
        Some(SetTime(_)) => "ve-tos object set-time",
        Some(SetExpires(_)) => "ve-tos object set-expires",
        Some(Append(_)) => "ve-tos object append",
        Some(SealAppend(_)) => "ve-tos object seal-append",
        Some(Modify(_)) => "ve-tos object modify",
        Some(Rename(_)) => "ve-tos object rename",
        Some(Restore(_)) => "ve-tos object restore",
        Some(Status(_)) => "ve-tos object status",
        Some(GetAcl(_)) => "ve-tos object get-acl",
        Some(SetAcl(_)) => "ve-tos object set-acl",
        Some(GetTagging(_)) => "ve-tos object get-tagging",
        Some(SetTagging(_)) => "ve-tos object set-tagging",
        Some(DeleteTagging(_)) => "ve-tos object delete-tagging",
        Some(Link(_)) => "ve-tos object link",
        Some(GetSymlink(_)) => "ve-tos object get-symlink",
        Some(CreateSymlink(_)) => "ve-tos object create-symlink",
        Some(GetFetchTask(_)) => "ve-tos object get-fetch-task",
        Some(CreateFetchTask(_)) => "ve-tos object create-fetch-task",
        Some(Fetch(_)) => "ve-tos object fetch",
        Some(SetRetention(_)) => "ve-tos object set-retention",
        Some(GetRetention(_)) => "ve-tos object get-retention",
        None => "ve-tos object",
    }
    .to_string()
}

fn multipart_command_path(cmd: &low_level::MultipartCommand) -> String {
    use low_level::MultipartAction::*;
    match cmd.action.as_ref() {
        Some(Create(_)) => "ve-tos multipart create",
        Some(Upload(_)) => "ve-tos multipart upload",
        Some(Complete(_)) => "ve-tos multipart complete",
        Some(Abort(_)) => "ve-tos multipart abort",
        Some(Copy(_)) => "ve-tos multipart copy",
        Some(ListParts(_)) => "ve-tos multipart list-parts",
        Some(List(_)) => "ve-tos multipart list",
        None => "ve-tos multipart",
    }
    .to_string()
}

fn turbo_command_path(cmd: &low_level::TurboCommand) -> String {
    use low_level::TurboAction::*;
    match cmd.action.as_ref() {
        Some(Open(_)) => "ve-tos turbo open",
        Some(Append(_)) => "ve-tos turbo append",
        Some(List(_)) => "ve-tos turbo list",
        Some(Close(_)) => "ve-tos turbo close",
        None => "ve-tos turbo",
    }
    .to_string()
}

#[cfg(test)]
mod command_path_tests {
    use super::*;

    /// [Spec §5] A `None` command (e.g. bare `tos`) yields a stable `"ve-tos"`
    /// label — never `"None"`, never `"ve-tos None"`.
    #[test]
    fn test_command_path_handles_no_subcommand() {
        assert_eq!(command_path(&None), "ve-tos");
    }

    /// [Spec §5] Meta subcommands surface their action so the error envelope
    /// is copy-paste-runnable: `"ve-tos config show"`, not `"ve-tos config"`.
    #[test]
    fn test_command_path_resolves_config_actions() {
        let init_cmd = TosCommand::Config(meta::ConfigCommand {
            action: Some(meta::ConfigAction::Init { profile: None }),
        });
        assert_eq!(command_path(&Some(init_cmd)), "ve-tos config init");

        let show_cmd = TosCommand::Config(meta::ConfigCommand {
            action: Some(meta::ConfigAction::Show),
        });
        assert_eq!(command_path(&Some(show_cmd)), "ve-tos config show");

        let bare_cmd = TosCommand::Config(meta::ConfigCommand { action: None });
        assert_eq!(command_path(&Some(bare_cmd)), "ve-tos config");
    }

    /// [Spec §5] Skill subcommands surface their action.
    #[test]
    fn test_command_path_resolves_skill_actions() {
        let list_cmd = TosCommand::Skill(meta::SkillCommand {
            action: meta::SkillAction::List {
                language: meta::DocumentationLanguage::En,
            },
        });
        assert_eq!(command_path(&Some(list_cmd)), "ve-tos skill list");
        let export_cmd = TosCommand::Skill(meta::SkillCommand {
            action: meta::SkillAction::Export {
                name: None,
                dir: "/tmp/x".to_string(),
                language: meta::DocumentationLanguage::En,
            },
        });
        assert_eq!(command_path(&Some(export_cmd)), "ve-tos skill export");
    }

    /// [Spec §5] High-level commands stay short — `"ve-tos cp"` is the canonical
    /// label every Agent expects to see.
    #[test]
    fn test_command_path_resolves_high_level_commands() {
        let cp_cmd = TosCommand::Cp(high_level::CpArgs {
            source: "a".to_string(),
            destination: "b".to_string(),
            recursive: false,
            include_parent: false,
            include: None,
            exclude: None,
            checkpoint: false,
            checkpoint_dir: None,
            content_type: None,
            storage_class: None,
            acl: None,
            meta: None,
            checkpoint_threshold: None,
            batch_concurrency: None,
            list_concurrency: None,
            recursive_list_mode: None,
            multipart_concurrency: None,
            progress_granularity: None,
            overwrite_strategy: None,
            report_path: None,
            report_failures_only: false,
            manifest_path: None,
            no_manifest: false,
            bandwidth_limit: None,
            list_echo: false,
            no_list_echo: false,
            progress: false,
            no_progress: false,
            force: false,
            no_clobber: false,
        });
        assert_eq!(command_path(&Some(cp_cmd)), "ve-tos cp");
    }
}
