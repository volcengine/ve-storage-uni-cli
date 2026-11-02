//! Typed public metadata for share commands.
use crate::registry::{find_capability, RegistryParameter};
use serde_json::{json, Value};

/// Describe a canonical share leaf without config, authentication or password I/O.
/// Returns None for paths outside the registered share command surface.
pub fn describe_share_command(command: &str) -> Option<Value> {
    if command == "ve-adrive share" {
        return Some(
            json!({"command":command,"kind":"command_group","description":"Manage and receive share links","supports_describe":true,"supports_help":true,"subcommands":crate::registry::capabilities().iter().filter(|row|row.group=="share").map(|row|json!({"name":row.command.rsplit(' ').next(),"description":row.description,"risk_level":row.risk_level})).collect::<Vec<_>>() }),
        );
    }
    if !command.starts_with("ve-adrive share ") {
        return None;
    }
    let row = find_capability(command)?;
    let mut description = json!({"command":command,"layer":row.layer,"description":row.description,
        "risk_level":row.risk_level,"supports_dry_run":true,"supports_pipe":true,
        // [Review Fix #Rebase5] Expose the cancellation gate consistently
        // with capabilities and doctor without changing critical deletes.
        "destructive":row.destructive,"supports_force":row.supports_force,
        "supports_yes":supports_yes(command),
        "parameters":row.parameters.iter().map(|item| parameter(command,item)).collect::<Vec<_>>(),
        "output_schema": output_schema(command),
        // [Review Fix #LiveDocs1] Describe/MCP guidance must expose the same connection and root rules as help.
        "semantics": share_semantics(command),
        "low_level_apis":row.api_actions,"wraps_apis":row.api_actions,
        "examples":row.examples,"side_effects":share_side_effects(command)
    });
    // [Review Fix #D02Docs] Planning has its own shape; preserve the approved real-download output contract.
    if command == "ve-adrive share download" {
        description["dry_run_output_schema"] = download_dry_run_output_schema();
    }
    Some(description)
}

fn share_semantics(command: &str) -> Vec<&'static str> {
    let mut semantics = vec![crate::cli::share::SHARE_CONNECTION_HELP];
    if matches!(
        command,
        "ve-adrive share get" | "ve-adrive share update" | "ve-adrive share cancel"
    ) {
        semantics.push(crate::cli::share::SHARE_SELECTOR_HELP);
    }
    if command == "ve-adrive share download" {
        semantics.push(crate::cli::share::SHARE_DOWNLOAD_HELP);
        // [Review Fix #ShareDownloadSelectionDocs] Retain selection and password rules without files-only pagination flags.
        semantics.push(crate::cli::share::SHARE_DOWNLOAD_SELECTION_HELP);
    }
    if command == "ve-adrive share view" {
        semantics.push(crate::cli::share::SHARE_VIEW_HELP);
    }
    if command == "ve-adrive share list" {
        semantics.push(crate::cli::share::SHARE_LIST_HELP);
    }
    if matches!(command, "ve-adrive share create" | "ve-adrive share update") {
        semantics.push(crate::cli::share::SHARE_SETTINGS_HELP);
    }
    // [Review Fix #ShareReceiveDocs2] Append new guidance without changing the established ordering of existing semantics.
    if matches!(
        command.rsplit(' ').next(),
        Some("inspect" | "files" | "download" | "view")
    ) {
        semantics.push(crate::cli::share::SHARE_RECEIVER_HELP);
    }
    // [Review Fix #ShareFilesMaxKeysDocs] Pagination controls belong only to files; download enumerates its complete selection.
    if command == "ve-adrive share files" {
        semantics.push(crate::cli::share::SHARE_FILES_HELP);
    }
    append_planning_semantics(command, &mut semantics);
    semantics
}

fn append_planning_semantics(command: &str, semantics: &mut Vec<&'static str>) {
    // [Review Fix #WholeSpaceDocs] Expose root creation consistently in Describe, exported skills and MCP.
    if command == "ve-adrive share create" {
        semantics.push(crate::cli::share::SHARE_CREATE_HELP);
    }
    // [Review Fix #D02D05Docs] Help, Describe, generated skills and MCP must share the same read-only/input boundary.
    if command == "ve-adrive share download" {
        semantics.push(crate::cli::share::SHARE_DRY_RUN_HELP);
    }
    if supports_password(command) {
        semantics.push(crate::cli::share::SHARE_PASSWORD_HELP);
    }
    semantics.push(crate::cli::share::SHARE_OFFLINE_HELP);
}

fn share_side_effects(command: &str) -> Vec<&'static str> {
    // [Review Fix #D02Docs] A non-destructive transfer still writes locally and URL issuance can consume quota.
    match command.rsplit(' ').next() {
        Some("create" | "update" | "cancel") => {
            vec!["Share settings are mutable; the shared target cannot change."]
        }
        Some("download") => vec![
            crate::cli::share::SHARE_RECEIVER_HELP,
            crate::cli::share::SHARE_DOWNLOAD_HELP,
            crate::cli::share::SHARE_DRY_RUN_HELP,
        ],
        Some("view") => vec![crate::cli::share::SHARE_VIEW_HELP],
        Some("files") => vec![
            crate::cli::share::SHARE_RECEIVER_HELP,
            crate::cli::share::SHARE_FILES_HELP,
        ],
        Some("inspect") => vec![crate::cli::share::SHARE_RECEIVER_HELP],
        _ => vec![],
    }
}

fn download_dry_run_output_schema() -> Value {
    json!({"type":"object","required":["dry_run","action","execution_status","selection"],"properties":{
        "dry_run":{"const":true},"action":{"const":"download"},
        "execution_status":{"const":"planned_not_executed"},
        "report_path":{"type":["string","null"]},"manifest_path":{"type":["string","null"]},"checkpoint_path":{"type":["string","null"]},
        "selection":download_plan_selection_schema()
    }})
}

fn download_plan_selection_schema() -> Value {
    json!({"type":"object","required":["Manifest","PlanComplete","FileCount","DirectoryCount","TotalBytes","SkippedCount","Conflicts","NetworkRequests","MetadataResponses","DownloadsIssued","LocalWrites","SnapshotGuaranteed"],"properties":{
            "ShareUrl":{"type":"string","format":"uri"},"Path":{"type":["string","null"]},
            "DestinationDir":{"type":"string"},"Checkpoint":{"type":["string","null"]},
            "Resume":{"type":"boolean"},"Recursive":{"type":"boolean"},"Overwrite":{"type":"boolean"},
            "PlanComplete":{"const":true},"FileCount":{"type":"integer","minimum":0},
            "DirectoryCount":{"type":"integer","minimum":0},"TotalBytes":{"type":"integer","minimum":0},
            "SkippedCount":{"type":"integer","minimum":0},"Conflicts":{"type":"array","items":{"type":"object"}},
            "NetworkRequests":{"type":"null"},"MetadataResponses":{"type":"integer","minimum":0},
            "DownloadsIssued":{"const":0},"LocalWrites":{"const":false},"SnapshotGuaranteed":{"const":false},
            "Manifest":download_plan_manifest_schema()
    }})
}

fn download_plan_manifest_schema() -> Value {
    json!({"type":"object","required":["Roots","Files","Directories"],"properties":{
                "Roots":{"type":"array","items":{"type":"string"}},
                "Files":{"type":"array","items":{"type":"object","required":["FilePath","LocalPath","DestinationPath","Size","Action"],"properties":{
                    "FilePath":{"type":"string"},"LocalPath":{"type":"string"},"DestinationPath":{"type":"string"},
                    "Size":{"type":"integer","minimum":0},"Action":{"enum":["create","overwrite","skip","conflict"]}}}},
                "Directories":{"type":"array","items":{"type":"object","required":["LocalPath","DestinationPath","Action"],"properties":{
                    "LocalPath":{"type":"string"},"DestinationPath":{"type":"string"},"Action":{"enum":["create","existing","conflict"]}}}}
    }})
}

/// Return whether global `--yes` supplies this share command's confirmation.
/// The canonical `command` must be `ve-adrive share cancel`; other paths return
/// false. This metadata lookup performs no I/O and cannot fail.
pub fn supports_yes(command: &str) -> bool {
    // [Review Fix #Rebase5] Cancellation revokes a share and uses its approved
    // --yes gate; it does not bypass force/confirm guards for file deletion.
    command == "ve-adrive share cancel"
}

fn output_schema(command: &str) -> Value {
    if command == "ve-adrive share view" {
        return json!({"type":"object","required":["ResultType","FilePath","StatusCode"],"properties":{
            "ResultType":{"enum":["raw","document"]},"FilePath":{"type":"string"},
            "StatusCode":{"type":"integer"},"ShareUrl":{"type":"string","format":"uri"},
            "Headers":{"type":"object","description":"Safe content headers for raw metadata results"},
            "PreviewURL":{"type":"string","format":"uri","description":"Short-lived document capability URL"},
            "ExpiresAt":{"type":"string","format":"date-time"},"PreviewType":{"type":"string"}
        }});
    }
    // [Review Fix #ShareServerURLSchema] Missing service URLs preserve success and expose an optional warning on every management result.
    if matches!(
        command,
        "ve-adrive share create" | "ve-adrive share get" | "ve-adrive share update"
    ) {
        return management_output_schema(command != "ve-adrive share update");
    }
    if command == "ve-adrive share list" {
        return json!({"type":"object","properties":{
            "Items":{"type":"array","items":management_output_schema(false)},
            "Total":{"type":"integer"},"Offset":{"type":"integer"},"Limit":{"type":"integer"}
        }});
    }
    if command == "ve-adrive share download" {
        return json!({"type":"object","required":["Complete","PlanComplete","Summary","Files"],"properties":{
            "Complete":{"type":"boolean"},"PlanComplete":{"type":"boolean"},
            "CheckpointAvailable":{"type":"boolean"},"IssuanceOutcomeUnknown":{"type":"boolean"},
            "report_path":{"type":["string","null"]},"manifest_path":{"type":["string","null"]},"checkpoint_path":{"type":["string","null"]},
            "DirectoryFailures":{"type":"array","items":{"type":"object","required":["LocalPath","Path","Message"],"properties":{"LocalPath":{"type":"string"},"Path":{"type":"string"},"Message":{"type":"string"}}}},
            "Summary":{"type":"object","properties":{"Total":{"type":"integer"},"Completed":{"type":"integer"},"Failed":{"type":"integer"},"Pending":{"type":"integer"},"Skipped":{"type":"integer","description":"Validated resumed files and overwrite-policy skips; included in Completed; table succeeded equals Completed minus Skipped"}}},
            "Files":{"type":"array","items":{"type":"object","properties":{"Path":{"type":"string"},"LocalPath":{"type":"string"},"Status":{"enum":["completed","failed","pending","skipped"]},"SHA256":{"type":["string","null"]}}}}
        }});
    }
    json!({"type":"object"})
}

fn management_output_schema(is_delivery_authorized: bool) -> Value {
    let mut schema = json!({"type":"object","properties":{
        "ShareID":{"type":"string"},
        // [Review Fix #WholeSpaceSchema] Omission means unknown; never default a missing service field to false.
        "ShareAllFiles":{"type":"boolean","description":"Whether the share covers the whole Space; omitted when not returned by the service"},
        "ShareUrl":{"type":"string","format":"uri","description":"Server ShareURL or a URL built from the configured share-endpoint; omitted when unavailable"},
        "ShareUrlWarning":{"type":"string","description":"Successful share result has no usable URL; retain ShareID and configure share-endpoint or retrieve the share again"}
    }});
    // [Review Fix #ShareDelivery1] Delivery passwords belong only to authenticated create/get results, never receiver or mutation metadata.
    if is_delivery_authorized {
        schema["properties"]["SharePwd"] = json!({"type":"string","description":"Share password returned for authorized delivery"});
    }
    schema
}

fn parameter(command: &str, item: &RegistryParameter) -> Value {
    json!({"name":item.name,"required":item.required,"description":item.description,
        "location":if matches!(item.name,"target"|"share-id"|"share-url"|"destination-dir") { "positional" } else { "flag" },
        "schema":parameter_schema(command,item.name)})
}

/// Return the share-specific schema, preserving scalar types and explicit bool values.
/// `command` is the canonical command path and `name` is a registry parameter.
/// Returns its JSON schema without I/O; this lookup cannot fail.
pub fn parameter_schema(command: &str, name: &str) -> Value {
    if is_share_boolean_parameter(name) {
        return json!({"type":"boolean"});
    }
    match name {
        "creator" => json!({"type":"array","items":{"type":"string"}}),
        "preview-limit" | "download-limit" => json!({"type":"integer","minimum":0}),
        "offset" => json!({"type":"integer","minimum":0,"maximum":10000}),
        "created-at-start" | "created-at-end" => json!({"type":"string","format":"date-time"}),
        "updated-at" => json!({"type":"integer"}),
        "batch-concurrency" => json!({"type":"integer","minimum":1,"default":16}),
        "list-concurrency" | "multipart-concurrency" => {
            json!({"type":"integer","minimum":1,"default":4})
        }
        "checkpoint-threshold" => json!({"type":"string","default":"20MB"}),
        "checkpoint" if command == "ve-adrive share download" => {
            json!({"oneOf":[{"type":"boolean"},{"type":"string","minLength":1}]})
        }
        "overwrite-strategy" => {
            json!({"type":"string","enum":["force","no-clobber","newer"],"default":"force"})
        }
        "progress-granularity" => json!({"type":"string","enum":["part","byte"],"default":"part"}),
        "bandwidth-limit" => json!({"type":"string","default":"0"}),
        "sort" => json!({"type":"string","enum":["key","name","size","last-modified","mtime"]}),
        // [Review Fix #ShareFilesMaxKeysSchema] Keep discovery and typed MCP validation aligned with the files CLI's bounded i32 cap.
        "max-keys" if command == "ve-adrive share files" => {
            json!({"type":"integer","minimum":1,"maximum":i32::MAX,"default":1000})
        }
        "limit" => json!({"type":"integer","minimum":1}),
        "permission" if command.ends_with("create") => {
            json!({"type":"string","enum":["view","download"],"default":crate::cli::share::DEFAULT_CREATE_SHARE_PERMISSION})
        }
        "permission" => json!({"type":"string","enum":["view","download"]}),
        "status" => json!({"type":"string","enum":["active","expired","revoked"]}),
        "expires-in" if command.ends_with("create") => json!({"type":"string","default":"0"}),
        _ => json!({"type":"string"}),
    }
}

fn is_share_boolean_parameter(name: &str) -> bool {
    matches!(
        name,
        "require-login"
            | "password-stdin"
            | "by-name"
            | "recursive"
            | "overwrite"
            | "resume"
            | "progress"
            | "no-progress"
            | "raw"
            | "include-parent"
            | "force"
            | "no-clobber"
            | "report-failures-only"
            | "no-manifest"
            | "list-echo"
            | "no-list-echo"
            | "human-readable"
    )
}

/// Whether this command supports a virtual MCP password passed only through stdin.
pub fn supports_password(command: &str) -> bool {
    matches!(
        command,
        "ve-adrive share create"
            | "ve-adrive share update"
            | "ve-adrive share files"
            | "ve-adrive share view"
            | "ve-adrive share download"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfer_discovery_includes_cp_options_and_artifact_results() {
        let download = describe_share_command("ve-adrive share download").unwrap();
        let parameters = download["parameters"].as_array().unwrap();
        for name in [
            "include-parent",
            "include",
            "exclude",
            "force",
            "no-clobber",
            "overwrite-strategy",
            "checkpoint-dir",
            "batch-concurrency",
            "list-concurrency",
            "progress-granularity",
            "bandwidth-limit",
            "report-path",
            "report-failures-only",
            "manifest-path",
            "no-manifest",
            "list-echo",
            "no-list-echo",
        ] {
            assert!(
                parameters.iter().any(|parameter| parameter["name"] == name),
                "missing {name}"
            );
        }
        let properties = &download["output_schema"]["properties"];
        for field in ["report_path", "manifest_path", "checkpoint_path"] {
            assert!(properties.get(field).is_some(), "missing {field}");
        }
        assert!(properties["Files"]["items"]["properties"]["Status"]["enum"]
            .as_array()
            .unwrap()
            .contains(&json!("skipped")));
        let files = describe_share_command("ve-adrive share files").unwrap();
        for name in ["human-readable", "sort", "columns", "manifest-path"] {
            assert!(
                files["parameters"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|parameter| parameter["name"] == name),
                "missing {name}"
            );
        }
    }

    #[test]
    fn transfer_schemas_preserve_boolean_flags_defaults_and_choices() {
        let command = "ve-adrive share download";
        for name in [
            "include-parent",
            "force",
            "no-clobber",
            "report-failures-only",
            "no-manifest",
            "list-echo",
            "no-list-echo",
        ] {
            assert_eq!(
                parameter_schema(command, name)["type"],
                "boolean",
                "wrong schema for {name}"
            );
        }
        for (name, expected) in [
            ("batch-concurrency", 16),
            ("list-concurrency", 4),
            ("multipart-concurrency", 4),
        ] {
            let schema = parameter_schema(command, name);
            assert_eq!(schema["type"], "integer");
            assert_eq!(schema["minimum"], 1);
            assert_eq!(schema["default"], expected);
        }
        assert_eq!(
            parameter_schema(command, "progress-granularity")["enum"],
            json!(["part", "byte"])
        );
        assert_eq!(
            parameter_schema(command, "overwrite-strategy")["enum"],
            json!(["force", "no-clobber", "newer"])
        );
        assert_eq!(
            parameter_schema(command, "overwrite-strategy")["default"],
            "force"
        );
        assert!(parameter_schema(command, "checkpoint")["oneOf"].is_array());
    }

    #[test]
    fn download_help_describes_current_resume_mapping_and_failures() {
        let semantics =
            describe_share_command("ve-adrive share download").unwrap()["semantics"].to_string();
        for phrase in [
            "--include-parent",
            "--include",
            "--exclude",
            "--no-clobber",
            "--report-path",
            "--no-manifest",
            "--list-echo",
            "--bandwidth-limit",
            "--progress-granularity",
            "partial ranges",
            "16 files",
            "4 directory",
        ] {
            assert!(semantics.contains(phrase), "missing {phrase}");
        }
        assert!(!semantics.contains("completed-file resume remains file-level"));
        assert!(!semantics.contains("20,000"));
    }

    #[test]
    fn files_describe_exposes_bounded_max_keys_without_legacy_limit() {
        let description = describe_share_command("ve-adrive share files").unwrap();
        let parameters = description["parameters"].as_array().unwrap();
        let max_keys = parameters
            .iter()
            .find(|parameter| parameter["name"] == "max-keys")
            .expect("files must advertise max-keys");
        assert_eq!(max_keys["schema"]["type"], "integer");
        assert_eq!(max_keys["schema"]["default"], 1000);
        assert_eq!(max_keys["schema"]["minimum"], 1);
        assert_eq!(max_keys["schema"]["maximum"], i32::MAX);
        assert!(!parameters
            .iter()
            .any(|parameter| parameter["name"] == "limit"));
        let semantics = description["semantics"].to_string();
        assert!(semantics.contains("--max-keys") && semantics.contains("1000"));
        assert!(semantics.contains("--marker"));
        assert!(!semantics.contains("one directory page"));
    }

    #[test]
    fn download_semantics_exclude_files_only_pagination_flags() {
        let description = describe_share_command("ve-adrive share download").unwrap();
        let semantics = description["semantics"].to_string();
        for flag in ["--max-keys", "--marker", "--limit"] {
            assert!(!semantics.contains(flag), "download advertises {flag}");
        }
        assert!(semantics.contains("EOF"));
        assert!(semantics.contains("default to anonymous"));
    }

    #[test]
    fn download_describe_retains_automatic_selection_and_password_precedence() {
        let description = describe_share_command("ve-adrive share download").unwrap();
        let semantics = description["semantics"].to_string();
        for phrase in [
            "Downloads enumerate all selected service pages automatically",
            "Exact file paths select one file",
            "directory paths end in / and require --recursive",
            "stdin > URL pwd > hidden ordinary-TTY prompt",
            "passwords are not saved",
        ] {
            assert!(
                semantics.contains(phrase),
                "missing download guidance: {phrase}"
            );
        }
    }

    #[test]
    fn list_describe_preserves_management_limit_schema() {
        let description = describe_share_command("ve-adrive share list").unwrap();
        let parameters = description["parameters"].as_array().unwrap();
        let limit = parameters
            .iter()
            .find(|parameter| parameter["name"] == "limit")
            .unwrap();
        assert_eq!(limit["schema"], json!({"type":"integer","minimum":1}));
        assert!(!parameters
            .iter()
            .any(|parameter| parameter["name"] == "max-keys"));
    }

    #[test]
    fn download_describe_separates_read_only_plan_schema_from_transfer_results() {
        let description = describe_share_command("ve-adrive share download").unwrap();
        let selection = &description["dry_run_output_schema"]["properties"]["selection"];
        let required = selection["required"]
            .as_array()
            .expect("planner selection schema");
        for field in [
            "Manifest",
            "PlanComplete",
            "FileCount",
            "DirectoryCount",
            "TotalBytes",
            "SkippedCount",
            "Conflicts",
            "MetadataResponses",
        ] {
            assert!(
                required.contains(&json!(field)),
                "missing planner field {field}"
            );
        }
        assert_eq!(selection["properties"]["NetworkRequests"]["type"], "null");
        assert_eq!(selection["properties"]["DownloadsIssued"]["const"], 0);
        assert_eq!(selection["properties"]["LocalWrites"]["const"], false);
        assert_eq!(
            selection["properties"]["SnapshotGuaranteed"]["const"],
            false
        );
        assert_eq!(
            description["output_schema"]["required"],
            json!(["Complete", "PlanComplete", "Summary", "Files"])
        );
        assert!(description["output_schema"]["properties"]
            .get("selection")
            .is_none());
        assert!(describe_share_command("ve-adrive share files")
            .unwrap()
            .get("dry_run_output_schema")
            .is_none());
    }

    #[test]
    fn download_side_effects_disclose_local_writes_and_read_only_planning() {
        let description = describe_share_command("ve-adrive share download").unwrap();
        let side_effects = description["side_effects"].to_string();
        for phrase in [
            "temporary files",
            "checkpoint",
            "--overwrite",
            "quota",
            "metadata",
            "no local writes",
        ] {
            assert!(
                side_effects.contains(phrase),
                "missing side effect {phrase}"
            );
        }
    }

    #[test]
    fn receiver_describe_exposes_explicit_oauth_and_pagination_rules() {
        for action in ["inspect", "files", "download"] {
            let value = describe_share_command(&format!("ve-adrive share {action}")).unwrap();
            let semantics = value["semantics"].to_string();
            assert!(
                semantics.contains("default to anonymous")
                    && semantics.contains("--auth-mode oauth")
            );
            assert!(semantics.contains("selected profile API or --endpoint"));
            if action == "files" {
                assert!(semantics.contains("--max-keys") && semantics.contains("--marker"));
            }
            if action != "inspect" {
                assert!(semantics.contains("EOF"));
            }
        }
    }

    #[test]
    fn create_permission_default_is_view_and_update_has_no_default() {
        assert_eq!(
            parameter_schema("ve-adrive share create", "permission")["default"],
            "view"
        );
        assert!(parameter_schema("ve-adrive share update", "permission")
            .get("default")
            .is_none());
    }

    #[test]
    fn management_instance_flag_is_optional_for_share_uri_selectors() {
        for action in ["get", "update", "cancel"] {
            let description = describe_share_command(&format!("ve-adrive share {action}")).unwrap();
            let instance = description["parameters"]
                .as_array()
                .unwrap()
                .iter()
                .find(|parameter| parameter["name"] == "instance")
                .unwrap();
            assert_eq!(instance["required"], false);
            assert!(instance["description"]
                .as_str()
                .unwrap()
                .contains("bare ShareID"));
            assert!(description["semantics"]
                .to_string()
                .contains("adrive://instance/share-id"));
        }
    }

    #[test]
    fn share_list_describes_optional_uri_and_instance_selectors() {
        let description = describe_share_command("ve-adrive share list").unwrap();
        let parameters = description["parameters"].as_array().unwrap();
        let target = parameters
            .iter()
            .find(|parameter| parameter["name"] == "target")
            .unwrap();
        assert_eq!(target["location"], "positional");
        assert_eq!(target["required"], false);
        let instance = parameters
            .iter()
            .find(|parameter| parameter["name"] == "instance")
            .unwrap();
        assert_eq!(instance["required"], false);
        assert!(description["semantics"]
            .to_string()
            .contains("list accepts adrive://instance"));
    }

    #[test]
    fn delivery_password_output_schema_is_limited_to_create_and_get() {
        for action in [
            "create", "get", "list", "update", "cancel", "inspect", "files", "view", "download",
        ] {
            let command = format!("ve-adrive share {action}");
            let description = describe_share_command(&command).unwrap();
            let property = &description["output_schema"]["properties"]["SharePwd"];
            if matches!(action, "create" | "get") {
                assert_eq!(property["type"], "string");
                assert_eq!(
                    property["description"],
                    "Share password returned for authorized delivery"
                );
                assert!(property.get("writeOnly").is_none());
            } else {
                assert!(property.is_null());
            }
        }
    }
}
