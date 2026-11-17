//! Chunk 命令的能力发现、Describe 和 MCP 类型契约。
use crate::registry::{find_capability, CapabilityRow, RegistryParameter};
use serde_json::{json, Value};

const TARGET: RegistryParameter = RegistryParameter {
    name: "target",
    required: false,
    description: "ADrive URI with Instance and Space IDs; conflicts with explicit selectors",
};
const INSTANCE: RegistryParameter = RegistryParameter {
    name: "instance",
    required: false,
    description: "Instance ID; required with explicit selectors",
};
const SPACE: RegistryParameter = RegistryParameter {
    name: "space",
    required: false,
    description: "Space ID; required with explicit selectors",
};

pub(crate) const CAPABILITIES: &[CapabilityRow] = &[
    CapabilityRow {
        command: "ve-adrive chunk search", description: "Search document chunks in a Space directory",
        domain: "chunk", group: "chunk", layer: "low-level", risk_level: "low",
        destructive: false, supports_force: false, supports_dry_run: true, api_actions: &["SearchFileChunk"],
        parameters: &[TARGET, INSTANCE, SPACE,
              RegistryParameter { name: "folder", required: false, description: "Directory inside the Space; defaults to the root" },
              RegistryParameter { name: "text", required: true, description: "Search text; nonempty after trimming, at most 100 UTF-8 bytes" },
              RegistryParameter { name: "limit", required: false, description: "Maximum returned chunks, 1 through 100" },
              RegistryParameter { name: "no-recursive", required: false, description: "Search only direct children; by default include subdirectories" }],
        examples: &["ve-adrive-cli chunk search adrive://instance-id/space-id/handbook/ --text '出差住宿标准是多少' --limit 5 --output json"],
    },
    CapabilityRow {
        command: "ve-adrive chunk read", description: "Read indexed chunks of one file version",
        domain: "chunk", group: "chunk", layer: "low-level", risk_level: "low",
        destructive: false, supports_force: false, supports_dry_run: true, api_actions: &["ReadFileChunk"],
        parameters: &[TARGET, INSTANCE, SPACE,
              RegistryParameter { name: "file", required: false, description: "Exact file path inside the Space" },
              RegistryParameter { name: "etag", required: true, description: "File version from the same search hit, passed back unchanged" },
              RegistryParameter { name: "ordinals", required: false, description: "Comma-separated nonnegative chunk ordinals, at most 20 without duplicates" },
              RegistryParameter { name: "range", required: false, description: "Inclusive ordinal range START-END, at most 20 chunks" }],
        examples: &["ve-adrive-cli chunk read adrive://instance-id/space-id/handbook/travel.txt --etag 'etag-from-current-search' --ordinals 0,1 --output json"],
    },
];

pub fn parameter_schema(name: &str) -> Value {
    match name {
        "limit" => json!({"type":"integer","minimum":1,"maximum":100,"default":10}),
        "no-recursive" => json!({"type":"boolean","default":false}),
        "ordinals" => json!({"type":"array","minItems":1,"maxItems":20,"uniqueItems":true,
                             "items":{"type":"integer","minimum":0,"maximum":i64::MAX}}),
        _ => json!({"type":"string","minLength":1}),
    }
}

pub fn describe(command: &str) -> Option<Value> {
    if command == "ve-adrive chunk" {
        return Some(
            json!({"command":command,"kind":"command_group","layer":"low-level","description":"Search and read document chunks",
            "supports_describe":true,"supports_help":true,
            "subcommands":CAPABILITIES.iter().map(|row|json!({"name":row.command.rsplit(' ').next(),"description":row.description})).collect::<Vec<_>>() }),
        );
    }
    if !command.starts_with("ve-adrive chunk ") {
        return None;
    }
    let row = find_capability(command)?;
    Some(
        json!({"command":command,"layer":row.layer,"description":row.description,"risk_level":"low",
            "supports_dry_run":true,"supports_pipe":true,"destructive":false,"supports_force":false,
            "parameters":row.parameters.iter().map(|p|json!({"name":p.name,"required":p.required,
                "description":p.description,"location":if p.name=="target" {"positional"} else {"flag"},
                "schema":parameter_schema(p.name)})).collect::<Vec<_>>(),
            "semantics":[crate::cli::chunk::CHUNK_HELP, crate::cli::chunk::CHUNK_READ_HELP],
            "low_level_apis":row.api_actions,"wraps_apis":row.api_actions,"examples":row.examples,
            "output_schema":{"type":"object","required":["Items"],"properties":{"Items":{"type":"array",
                "items":{"type":"object","properties":{
                    "SpaceId":{"type":"string"},"FilePath":{"type":"string"},"ETag":{"type":"string"},
                    "ChunkOrdinal":{"type":"integer"},"ChunkCount":{"type":"integer"},
                    "ChunkTruncated":{"type":"boolean"},"TextContent":{"type":"string"}}}}}},
            "side_effects":["oauth_token_refresh"],"resource_mutations":false,"authentication":"oauth"
        }),
    )
}

pub(crate) fn validate_mcp_arguments(
    command: &str,
    arguments: &serde_json::Map<String, Value>,
) -> Result<(), tos_core::agent::error::CliError> {
    use tos_core::agent::error::CliError;
    let row = find_capability(command)
        .ok_or_else(|| CliError::ValidationError("unknown Chunk command".into()))?;
    for parameter in row.parameters {
        let Some(value) = arguments.get(parameter.name) else {
            continue;
        };
        let valid = match parameter.name {
            "limit" => value.as_u64().is_some_and(|n| (1..=100).contains(&n)),
            "no-recursive" => value.is_boolean(),
            "ordinals" => value
                .as_array()
                .is_some_and(|items| items.iter().all(|n| n.as_u64().is_some())),
            "target" => value.as_str().is_some_and(|s| s.starts_with("adrive://")),
            _ => value.as_str().is_some_and(|s| !s.trim().is_empty()),
        };
        if !valid {
            return Err(CliError::ValidationError(format!(
                "invalid typed Chunk MCP argument: {}",
                parameter.name
            )));
        }
    }
    if arguments.contains_key("target")
        && ["instance", "space", "folder", "file"]
            .iter()
            .any(|key| arguments.contains_key(*key))
    {
        return Err(CliError::ValidationError(
            "Chunk target conflicts with explicit selectors".into(),
        ));
    }
    if arguments.contains_key("ordinals") && arguments.contains_key("range") {
        return Err(CliError::ValidationError(
            "select exactly one of --ordinals or --range".into(),
        ));
    }
    if arguments.get("describe").and_then(Value::as_bool) == Some(true) {
        return Ok(());
    }
    if arguments
        .get("auth_mode")
        .and_then(Value::as_str)
        .is_some_and(|mode| mode != "oauth")
    {
        return Err(CliError::ValidationError(
            "Chunk commands require --auth-mode oauth".into(),
        ));
    }
    use crate::cli::chunk::{ChunkAction, ChunkTargetArgs, ReadChunkArgs, SearchChunkArgs};
    let string = |name| {
        arguments
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let selector = ChunkTargetArgs {
        target: string("target"),
        instance: string("instance"),
        space: string("space"),
    };
    let action = if command == "ve-adrive chunk search" {
        ChunkAction::Search(SearchChunkArgs {
            selector,
            folder: string("folder"),
            text: string("text"),
            limit: arguments.get("limit").and_then(Value::as_u64).unwrap_or(10) as u32,
            no_recursive: arguments
                .get("no-recursive")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    } else {
        ChunkAction::Read(ReadChunkArgs {
            selector,
            file: string("file"),
            etag: string("etag"),
            range: string("range"),
            ordinals: arguments
                .get("ordinals")
                .and_then(Value::as_array)
                .map(|items| items.iter().filter_map(Value::as_u64).collect())
                .unwrap_or_default(),
        })
    };
    crate::handler::chunk::prepare(&action)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_discovery_uses_low_level_for_group_and_leaf_contracts() {
        assert_eq!(describe("ve-adrive chunk").unwrap()["layer"], "low-level");
        for row in CAPABILITIES {
            assert_eq!(row.layer, "low-level");
            assert_eq!(describe(row.command).unwrap()["layer"], row.layer);
            assert_eq!(
                crate::registry::business_domain(row.command),
                "adrive-chunk"
            );
        }
    }
}
