//! 将 CLI 参数转换为只读 Chunk 请求；预演始终离线。
use super::common::{
    build_adrive_runtime, map_ids_error, output_result, resolve_auth_mode, resolve_target,
};
use crate::cli::{chunk::*, ADriveAuthArgs};
use crate::domain::auth::AuthMode;
use crate::domain::chunk::*;
use serde::Serialize;
use serde_json::json;
use tos_core::agent::{envelope::Envelope, error::CliError, global_args::GlobalArgs};

pub async fn handle_chunk_command(
    global: &GlobalArgs,
    auth: &ADriveAuthArgs,
    group: &ChunkCommand,
) -> Result<i32, CliError> {
    let command = group
        .action
        .as_ref()
        .map(|action| format!("ve-adrive chunk {}", action.name()))
        .unwrap_or_else(|| "ve-adrive chunk".into());
    if global.describe {
        output_result(
            global,
            &Envelope::success(&command, crate::chunk_metadata::describe(&command)),
        )?;
        return Ok(0);
    }
    let Some(action) = &group.action else {
        use clap::CommandFactory;
        #[derive(clap::Parser)]
        struct Help {
            #[command(flatten)]
            chunk: ChunkCommand,
        }
        Help::command().name("ve-adrive-cli chunk").print_help()?;
        println!();
        return Ok(0);
    };
    let (instance, input) = prepare(action)?;
    let mode = if global.dry_run {
        auth.auth_mode.unwrap_or(AuthMode::Oauth)
    } else {
        resolve_auth_mode(global, auth.auth_mode)?.mode
    };
    if mode != AuthMode::Oauth {
        return Err(CliError::ValidationError(
            "Chunk commands require --auth-mode oauth".into(),
        ));
    }
    if global.dry_run {
        let endpoint = global
            .endpoint
            .as_deref()
            .map(|endpoint| instance_endpoint(endpoint, &instance))
            .transpose()
            .map_err(map_ids_error)?;
        output_result(
            global,
            &Envelope::success(
                &command,
                json!({
                    "dry_run": true, "execution_status": "planned_not_executed",
                    "method": "POST", "path": format!("/v2/file/{}_chunk", action.name()),
                    "instance_id": instance, "endpoint_routing": "instance_subdomain", "endpoint": endpoint,
                    "body": input, "authentication": "oauth", "network_requests": 0
                }),
            ),
        )?;
        return Ok(0);
    }
    let runtime = build_adrive_runtime(global, Some(mode))?;
    let output = match &input {
        PreparedRequest::Search(input) => runtime.client.search_file_chunks(&instance, input).await,
        PreparedRequest::Read(input) => runtime.client.read_file_chunks(&instance, input).await,
    }
    .map_err(map_ids_error)?;
    output_result(global, &Envelope::success(command, output))?;
    Ok(0)
}

#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum PreparedRequest {
    Search(SearchChunkInput),
    Read(ReadChunkInput),
}

pub(crate) fn prepare(action: &ChunkAction) -> Result<(String, PreparedRequest), CliError> {
    let (selector, folder, file) = match action {
        ChunkAction::Search(args) => (&args.selector, args.folder.as_deref(), None),
        ChunkAction::Read(args) => (&args.selector, None, args.file.as_deref()),
    };
    let mut target = resolve_target(
        selector.target.as_deref(),
        selector.instance.as_deref(),
        selector.space.as_deref(),
        folder,
        file,
    )?;
    if selector.target.is_some() {
        if selector
            .target
            .as_ref()
            .is_some_and(|uri| uri.contains(['?', '#', '\\']))
        {
            return Err(CliError::ValidationError(
                "encode reserved URI characters or use explicit --file/--folder selectors".into(),
            ));
        }
        target.path =
            crate::domain::share_url::decode_component(&target.path, false).map_err(|_| {
                CliError::ValidationError("invalid percent-encoding in Chunk target".into())
            })?;
    }
    validate_component(&target.instance, "InstanceId").map_err(map_ids_error)?;
    let input = match action {
        ChunkAction::Search(args) => {
            let request = SearchChunkInput {
                space_ids: [target.space],
                folder_path: if target.path.is_empty() {
                    "/".into()
                } else {
                    target.path
                },
                recursive: !args.no_recursive,
                search_query: [ChunkQuery {
                    text: args.text.as_deref().unwrap_or("").trim().into(),
                }],
                limit: args.limit,
            };
            request.validate().map_err(map_ids_error)?;
            PreparedRequest::Search(request)
        }
        ChunkAction::Read(args) => {
            let range = args.range.as_deref().map(parse_range).transpose()?;
            let request = ReadChunkInput {
                space_id: target.space,
                file_path: target.path,
                etag: args.etag.clone().unwrap_or_default(),
                chunk_ordinals: args.ordinals.clone(),
                chunk_range: range,
            };
            request.validate().map_err(map_ids_error)?;
            PreparedRequest::Read(request)
        }
    };
    Ok((target.instance, input))
}

fn parse_range(value: &str) -> Result<ChunkRange, CliError> {
    let invalid =
        || CliError::ValidationError("--range must be START-END with nonnegative ordinals".into());
    let (start, end) = value.split_once('-').ok_or_else(invalid)?;
    Ok(ChunkRange {
        start: start.parse().map_err(|_| invalid())?,
        end: end.parse().map_err(|_| invalid())?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct TestCli {
        #[command(flatten)]
        chunk: ChunkCommand,
    }

    fn request(arguments: &[&str]) -> Result<(String, PreparedRequest), CliError> {
        let command =
            TestCli::try_parse_from(std::iter::once("test").chain(arguments.iter().copied()))
                .map_err(|error| CliError::ValidationError(error.to_string()))?;
        prepare(command.chunk.action.as_ref().unwrap())
    }

    #[test]
    fn chunk_search_preserves_scope_text_and_wire_names() {
        let (instance, input) = request(&[
            "search",
            "adrive://inst/space/handbook/",
            "--text",
            " 差旅 ",
            "--no-recursive",
            "--limit",
            "5",
        ])
        .unwrap();
        assert_eq!(instance, "inst");
        assert_eq!(
            serde_json::to_value(input).unwrap(),
            json!({"SpaceIds":["space"],
            "FolderPath":"handbook/","Recursive":false,"SearchQuery":[{"Text":"差旅"}],"Limit":5})
        );
        let (_, input) = request(&[
            "search",
            "--instance",
            "inst",
            "--space",
            "space",
            "--text",
            "x",
        ])
        .unwrap();
        assert_eq!(serde_json::to_value(input).unwrap()["FolderPath"], "/");
    }

    #[test]
    fn chunk_search_enforces_trimmed_utf8_bytes_and_count() {
        for text in [String::new(), " \t ".into(), "中".repeat(34)] {
            assert!(request(&["search", "adrive://i/s/", "--text", &text]).is_err());
        }
        let text = "中".repeat(33) + "a";
        assert!(request(&["search", "adrive://i/s/", "--text", &text]).is_ok());
        for limit in ["0", "101"] {
            assert!(
                request(&["search", "adrive://i/s/", "--text", "x", "--limit", limit]).is_err()
            );
        }
        assert!(request(&["search", "adrive://i/s/", "--text", "x", "--limit", "100"]).is_ok());
    }

    #[test]
    fn chunk_read_keeps_exact_version_and_ordinal_selector() {
        let (_, input) = request(&[
            "read",
            "adrive://inst/space/docs/%E4%B8%AD%20%23.txt",
            "--etag",
            "\"old-version\"",
            "--ordinals",
            "2,0",
        ])
        .unwrap();
        assert_eq!(
            serde_json::to_value(input).unwrap(),
            json!({"SpaceId":"space", "FilePath":"docs/中 #.txt", "ETag":"\"old-version\"", "ChunkOrdinals":[2,0]})
        );
        let (_, input) = request(&[
            "read",
            "--instance",
            "inst",
            "--space",
            "space",
            "--file",
            "a.txt",
            "--etag",
            "v",
            "--range",
            "0-19",
        ])
        .unwrap();
        let value = serde_json::to_value(input).unwrap();
        assert_eq!(value["ChunkRange"], json!({"Start":0,"End":19}));
        assert!(value.get("ChunkOrdinals").is_none());
    }

    #[test]
    fn chunk_read_rejects_conflicting_missing_and_invalid_inputs() {
        for tail in [
            vec![],
            vec!["--ordinals", "0,0"],
            vec!["--range", "2-1"],
            vec!["--range", "0-20"],
            vec!["--range", "0-18446744073709551615"],
            vec!["--ordinals", "18446744073709551615"],
            vec!["--ordinals", "-1"],
            vec!["--ordinals", "0", "--range", "0-0"],
        ] {
            let mut args = vec!["read", "adrive://i/s/a.txt", "--etag", "v"];
            args.extend(tail);
            assert!(request(&args).is_err(), "{args:?}");
        }
        assert!(request(&["read", "adrive://i/s/a.txt", "--ordinals", "0"]).is_err());
    }

    #[test]
    fn chunk_targets_cannot_silently_change_scope() {
        for args in [
            vec![
                "search",
                "adrive://i/s/docs/",
                "--instance",
                "other",
                "--text",
                "x",
            ],
            vec![
                "search",
                "adrive://i/s/docs/",
                "--folder",
                "other/",
                "--text",
                "x",
            ],
            vec!["search", "adrive://i", "--text", "x"],
            vec!["search", "adrive://i/s/file.txt", "--text", "x"],
            vec!["search", "adrive://i/s/../", "--text", "x"],
            vec![
                "read",
                "adrive://i/s/docs/",
                "--etag",
                "v",
                "--ordinals",
                "0",
            ],
            vec![
                "read",
                "adrive://i/s/a.txt?x",
                "--etag",
                "v",
                "--ordinals",
                "0",
            ],
        ] {
            assert!(request(&args).is_err(), "{args:?}");
        }
    }
}
