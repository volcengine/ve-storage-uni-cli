//! 文件核心 API 命令处理；set-meta 只更新元数据，不重传内容。
use super::common::{
    build_adrive_runtime, map_ids_error, output_envelope, output_result, parse_adrive_uri,
};
use super::high_level::resolve_parsed_target_by_name;
use crate::cli::{file::*, ADriveAuthArgs};
use crate::domain::types::UpdateFileMetaInput;
use serde_json::json;
use tos_core::agent::{envelope::Envelope, error::CliError, global_args::GlobalArgs};

pub async fn handle_file_command(
    global: &GlobalArgs,
    auth: &ADriveAuthArgs,
    group: &FileCommand,
) -> Result<i32, CliError> {
    let command = group
        .action
        .as_ref()
        .map(|action| format!("ve-adrive file {}", action.name()))
        .unwrap_or_else(|| "ve-adrive file".into());
    if global.describe {
        let description = crate::file_metadata::describe(&command)
            .unwrap_or_else(|| json!({ "command": command }));
        output_result(global, &Envelope::success(&command, description))?;
        return Ok(0);
    }
    let Some(action) = &group.action else {
        use clap::CommandFactory;
        #[derive(clap::Parser)]
        struct Help {
            #[command(flatten)]
            file: FileCommand,
        }
        Help::command().name("ve-adrive-cli file").print_help()?;
        println!();
        return Ok(0);
    };
    match action {
        FileAction::SetMeta(args) => execute_set_meta(global, auth, &command, args).await,
    }
}

async fn execute_set_meta(
    global: &GlobalArgs,
    auth: &ADriveAuthArgs,
    command: &str,
    args: &SetMetaArgs,
) -> Result<i32, CliError> {
    crate::file_metadata::validate_set_meta_target(&args.path)?;
    let target = parse_adrive_uri(&args.path, false)?;
    if global.dry_run {
        output_envelope(
            global,
            &Envelope::success(
                command,
                json!({
                    "command": command,
                    "dry_run": true,
                    "execution_status": "planned_not_executed",
                    "method": "PUT",
                    "path": format!(
                        "/v1/instances/{}/spaces/{}/files/{}:meta",
                        target.instance, target.space, target.path
                    ),
                    "body": {"Meta": args.meta},
                    "by_name": args.by_name,
                    "network_requests": 0,
                    "semantics": "Incremental update; omitted keys are preserved and empty values are not deletions"
                }),
            ),
        )?;
        return Ok(0);
    }
    let runtime = build_adrive_runtime(global, auth.auth_mode)?;
    let target = if args.by_name {
        resolve_parsed_target_by_name(&runtime.client, target).await?
    } else {
        target
    };
    let output = runtime
        .client
        .update_file_meta(&UpdateFileMetaInput {
            instance_id: target.instance,
            space_id: target.space,
            file_path: target.path,
            meta: args.meta.clone(),
        })
        .await
        .map_err(map_ids_error)?;
    output_envelope(global, &Envelope::success(command, output))?;
    Ok(0)
}
