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

use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct RegistryParameter {
    pub name: &'static str,
    pub required: bool,
    pub description: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct CapabilityRow {
    pub command: &'static str,
    pub domain: &'static str,
    pub group: &'static str,
    pub layer: &'static str,
    pub description: &'static str,
    pub risk_level: &'static str,
    pub destructive: bool,
    pub supports_force: bool,
    pub supports_dry_run: bool,
    pub api_actions: &'static [&'static str],
    pub parameters: &'static [RegistryParameter],
    pub examples: &'static [&'static str],
}

const PATH_PARAM: RegistryParameter = RegistryParameter {
    name: "path",
    required: false,
    description: "Optional tos://bucket/key or tos://bucket/prefix target",
};

const BUCKET_PARAM: RegistryParameter = RegistryParameter {
    name: "bucket",
    required: false,
    description: "Bucket name when URI style is not used",
};

const KEY_PARAM: RegistryParameter = RegistryParameter {
    name: "key",
    required: false,
    description: "Object key or prefix when URI style is not used",
};

const SOURCE_PARAM: RegistryParameter = RegistryParameter {
    name: "source",
    required: true,
    description: "Local path or tos://bucket/key source",
};

const DESTINATION_PARAM: RegistryParameter = RegistryParameter {
    name: "destination",
    required: true,
    description: "Local path or tos://bucket/key destination",
};

const RECURSIVE_PARAM: RegistryParameter = RegistryParameter {
    name: "recursive",
    required: false,
    description: "Traverse prefixes recursively with delimiter=\"/\"",
};

const INCLUDE_PARENT_PARAM: RegistryParameter = RegistryParameter {
    name: "include-parent",
    required: false,
    description: "Include the source directory or prefix name under the destination prefix",
};

const INCLUDE_PARAM: RegistryParameter = RegistryParameter {
    name: "include",
    required: false,
    description: "Include pattern",
};

const EXCLUDE_PARAM: RegistryParameter = RegistryParameter {
    name: "exclude",
    required: false,
    description: "Exclude pattern",
};

const CHECKPOINT_PARAM: RegistryParameter = RegistryParameter {
    name: "checkpoint",
    required: false,
    description: "Enable resumable transfer or recursive item checkpointing",
};

const CHECKPOINT_DIR_PARAM: RegistryParameter = RegistryParameter {
    name: "checkpoint-dir",
    required: false,
    description: "Directory for transfer checkpoint state",
};

const CONTENT_TYPE_PARAM: RegistryParameter = RegistryParameter {
    name: "content-type",
    required: false,
    description: "Content-Type for uploaded or copied objects",
};

const ACL_PARAM: RegistryParameter = RegistryParameter {
    name: "acl",
    required: false,
    description: "Target object ACL",
};

const META_PARAM: RegistryParameter = RegistryParameter {
    name: "meta",
    required: false,
    description: "Custom TOS metadata as key=value pairs",
};

const CHECKPOINT_THRESHOLD_PARAM: RegistryParameter = RegistryParameter {
    name: "checkpoint-threshold",
    required: false,
    description: "File size threshold for checkpoint multipart/range transfer",
};

const BATCH_CONCURRENCY_PARAM: RegistryParameter = RegistryParameter {
    name: "batch-concurrency",
    required: false,
    description: "Maximum files/items running concurrently in batch commands",
};

const LIST_CONCURRENCY_PARAM: RegistryParameter = RegistryParameter {
    name: "list-concurrency",
    required: false,
    description: "Maximum prefixes listed concurrently in recursive batch commands",
};

const RECURSIVE_LIST_MODE_PARAM: RegistryParameter = RegistryParameter {
    name: "recursive-list-mode",
    required: false,
    description: "Recursive listing mode; tos accepts hierarchical listing semantics",
};

const MULTIPART_CONCURRENCY_PARAM: RegistryParameter = RegistryParameter {
    name: "multipart-concurrency",
    required: false,
    description: "Maximum parts/ranges running concurrently for one large file",
};

const PROGRESS_GRANULARITY_PARAM: RegistryParameter = RegistryParameter {
    name: "progress-granularity",
    required: false,
    description: "Progress granularity: part or byte",
};

const OVERWRITE_STRATEGY_PARAM: RegistryParameter = RegistryParameter {
    name: "overwrite-strategy",
    required: false,
    description: "Destination overwrite strategy",
};

const REPORT_PATH_PARAM: RegistryParameter = RegistryParameter {
    name: "report-path",
    required: false,
    description: "Write batch success/failure report to this path",
};

const REPORT_FAILURES_ONLY_PARAM: RegistryParameter = RegistryParameter {
    name: "report-failures-only",
    required: false,
    description: "Write only failed items to the batch report",
};

const MANIFEST_PATH_PARAM: RegistryParameter = RegistryParameter {
    name: "manifest-path",
    required: false,
    description: "Write planned operation manifest to this path",
};

const NO_MANIFEST_PARAM: RegistryParameter = RegistryParameter {
    name: "no-manifest",
    required: false,
    description: "Disable planned manifest output",
};

const BANDWIDTH_LIMIT_PARAM: RegistryParameter = RegistryParameter {
    name: "bandwidth-limit",
    required: false,
    description: "Throttle upload/download bandwidth, e.g. 100MB",
};

const LIST_ECHO_PARAM: RegistryParameter = RegistryParameter {
    name: "list-echo",
    required: false,
    description: "Enable listing-phase echo output on stderr",
};

const NO_LIST_ECHO_PARAM: RegistryParameter = RegistryParameter {
    name: "no-list-echo",
    required: false,
    description: "Disable listing-phase echo output",
};

const PROGRESS_PARAM: RegistryParameter = RegistryParameter {
    name: "progress",
    required: false,
    description: "Enable execution progress output on stderr",
};

const NO_PROGRESS_PARAM: RegistryParameter = RegistryParameter {
    name: "no-progress",
    required: false,
    description: "Disable execution progress output",
};

const FORCE_PARAM: RegistryParameter = RegistryParameter {
    name: "force",
    required: false,
    description: "Allow overwrite/delete operations without interactive confirmation",
};

const CONFIRM_PARAM: RegistryParameter = RegistryParameter {
    name: "confirm",
    required: false,
    description: "Exact target confirmation for non-interactive destructive commands",
};

const NO_CLOBBER_PARAM: RegistryParameter = RegistryParameter {
    name: "no-clobber",
    required: false,
    description: "Fail when the destination already exists",
};

// [Review Fix #6] ByteCloud tos capabilities intentionally mirror supported
// high-level batch controls while omitting ve-tos-only creation/delete flags.
const CP_PARAMS: &[RegistryParameter] = &[
    SOURCE_PARAM,
    DESTINATION_PARAM,
    RECURSIVE_PARAM,
    INCLUDE_PARENT_PARAM,
    INCLUDE_PARAM,
    EXCLUDE_PARAM,
    CHECKPOINT_PARAM,
    CHECKPOINT_DIR_PARAM,
    CONTENT_TYPE_PARAM,
    ACL_PARAM,
    META_PARAM,
    CHECKPOINT_THRESHOLD_PARAM,
    BATCH_CONCURRENCY_PARAM,
    LIST_CONCURRENCY_PARAM,
    RECURSIVE_LIST_MODE_PARAM,
    MULTIPART_CONCURRENCY_PARAM,
    PROGRESS_GRANULARITY_PARAM,
    OVERWRITE_STRATEGY_PARAM,
    REPORT_PATH_PARAM,
    REPORT_FAILURES_ONLY_PARAM,
    MANIFEST_PATH_PARAM,
    NO_MANIFEST_PARAM,
    BANDWIDTH_LIMIT_PARAM,
    LIST_ECHO_PARAM,
    NO_LIST_ECHO_PARAM,
    PROGRESS_PARAM,
    NO_PROGRESS_PARAM,
    FORCE_PARAM,
    NO_CLOBBER_PARAM,
];

const MV_PARAMS: &[RegistryParameter] = &[
    SOURCE_PARAM,
    DESTINATION_PARAM,
    RECURSIVE_PARAM,
    INCLUDE_PARENT_PARAM,
    INCLUDE_PARAM,
    EXCLUDE_PARAM,
    CHECKPOINT_DIR_PARAM,
    CONTENT_TYPE_PARAM,
    ACL_PARAM,
    META_PARAM,
    CHECKPOINT_THRESHOLD_PARAM,
    BATCH_CONCURRENCY_PARAM,
    LIST_CONCURRENCY_PARAM,
    RECURSIVE_LIST_MODE_PARAM,
    MULTIPART_CONCURRENCY_PARAM,
    PROGRESS_GRANULARITY_PARAM,
    OVERWRITE_STRATEGY_PARAM,
    REPORT_PATH_PARAM,
    REPORT_FAILURES_ONLY_PARAM,
    MANIFEST_PATH_PARAM,
    NO_MANIFEST_PARAM,
    LIST_ECHO_PARAM,
    NO_LIST_ECHO_PARAM,
    PROGRESS_PARAM,
    NO_PROGRESS_PARAM,
    FORCE_PARAM,
    CONFIRM_PARAM,
];

const SYNC_PARAMS: &[RegistryParameter] = &[
    SOURCE_PARAM,
    DESTINATION_PARAM,
    RegistryParameter {
        name: "delete",
        required: false,
        description: "Delete extraneous destination objects",
    },
    FORCE_PARAM,
    CONFIRM_PARAM,
    RegistryParameter {
        name: "size-only",
        required: false,
        description: "Compare by size only",
    },
    RegistryParameter {
        name: "exact-timestamps",
        required: false,
        description: "Use exact timestamps for comparison",
    },
    INCLUDE_PARENT_PARAM,
    INCLUDE_PARAM,
    EXCLUDE_PARAM,
    CHECKPOINT_DIR_PARAM,
    CONTENT_TYPE_PARAM,
    ACL_PARAM,
    META_PARAM,
    CHECKPOINT_THRESHOLD_PARAM,
    BATCH_CONCURRENCY_PARAM,
    LIST_CONCURRENCY_PARAM,
    RECURSIVE_LIST_MODE_PARAM,
    MULTIPART_CONCURRENCY_PARAM,
    PROGRESS_GRANULARITY_PARAM,
    OVERWRITE_STRATEGY_PARAM,
    REPORT_PATH_PARAM,
    REPORT_FAILURES_ONLY_PARAM,
    MANIFEST_PATH_PARAM,
    NO_MANIFEST_PARAM,
    BANDWIDTH_LIMIT_PARAM,
    LIST_ECHO_PARAM,
    NO_LIST_ECHO_PARAM,
    PROGRESS_PARAM,
    NO_PROGRESS_PARAM,
];

const MKDIR_PARAMS: &[RegistryParameter] = &[
    PATH_PARAM,
    BUCKET_PARAM,
    KEY_PARAM,
    RegistryParameter {
        name: "parents",
        required: false,
        description: "Create parent folder markers as needed",
    },
];

const RM_PARAMS: &[RegistryParameter] = &[
    PATH_PARAM,
    BUCKET_PARAM,
    KEY_PARAM,
    RECURSIVE_PARAM,
    FORCE_PARAM,
    CONFIRM_PARAM,
    RegistryParameter {
        name: "all-versions",
        required: false,
        description: "Delete every object version and delete marker",
    },
    RegistryParameter {
        name: "include-uploads",
        required: false,
        description: "Abort incomplete multipart uploads matching the prefix",
    },
    REPORT_PATH_PARAM,
    REPORT_FAILURES_ONLY_PARAM,
    MANIFEST_PATH_PARAM,
    NO_MANIFEST_PARAM,
    BATCH_CONCURRENCY_PARAM,
    LIST_CONCURRENCY_PARAM,
    RECURSIVE_LIST_MODE_PARAM,
    INCLUDE_PARAM,
    EXCLUDE_PARAM,
    LIST_ECHO_PARAM,
    NO_LIST_ECHO_PARAM,
    PROGRESS_PARAM,
    NO_PROGRESS_PARAM,
];

const LS_PARAMS: &[RegistryParameter] = &[
    PATH_PARAM,
    BUCKET_PARAM,
    KEY_PARAM,
    RegistryParameter {
        name: "max-keys",
        required: false,
        description: "Maximum buckets, objects, or prefixes to return",
    },
    RegistryParameter {
        name: "continuation-token",
        required: false,
        description: "Continuation token returned by a previous listing",
    },
    RegistryParameter {
        name: "human-readable",
        required: false,
        description: "Render human-readable sizes",
    },
    RegistryParameter {
        name: "sort",
        required: false,
        description: "Sort field",
    },
    RegistryParameter {
        name: "columns",
        required: false,
        description: "Comma-separated output columns",
    },
    MANIFEST_PATH_PARAM,
];

const STAT_PARAMS: &[RegistryParameter] = &[
    PATH_PARAM,
    BUCKET_PARAM,
    KEY_PARAM,
    RegistryParameter {
        name: "version-id",
        required: false,
        description: "Object version ID",
    },
];

const DU_PARAMS: &[RegistryParameter] = &[
    PATH_PARAM,
    BUCKET_PARAM,
    KEY_PARAM,
    RegistryParameter {
        name: "human-readable",
        required: false,
        description: "Render human-readable sizes",
    },
    RegistryParameter {
        name: "max-depth",
        required: false,
        description: "Maximum directory depth",
    },
    RegistryParameter {
        name: "top-k",
        required: false,
        description: "Number of largest/oldest object samples to keep in verbose diagnostics; 0 disables samples",
    },
    RegistryParameter {
        name: "cost",
        required: false,
        description: "Include estimated monthly storage cost",
    },
    RegistryParameter {
        name: "storage-price",
        required: false,
        description: "Override storage price as CLASS=PRICE",
    },
    MANIFEST_PATH_PARAM,
    LIST_CONCURRENCY_PARAM,
    LIST_ECHO_PARAM,
    NO_LIST_ECHO_PARAM,
    PROGRESS_PARAM,
    NO_PROGRESS_PARAM,
];

const FIND_PARAMS: &[RegistryParameter] = &[
    PATH_PARAM,
    BUCKET_PARAM,
    KEY_PARAM,
    RegistryParameter {
        name: "name",
        required: false,
        description: "Name pattern",
    },
    RegistryParameter {
        name: "size",
        required: false,
        description: "Size filter",
    },
    RegistryParameter {
        name: "mtime",
        required: false,
        description: "Modification time filter",
    },
    MANIFEST_PATH_PARAM,
    LIST_ECHO_PARAM,
    NO_LIST_ECHO_PARAM,
    PROGRESS_PARAM,
    NO_PROGRESS_PARAM,
];

const CAT_PARAMS: &[RegistryParameter] = &[
    PATH_PARAM,
    BUCKET_PARAM,
    KEY_PARAM,
    RegistryParameter {
        name: "range",
        required: false,
        description: "Byte range",
    },
    RegistryParameter {
        name: "version-id",
        required: false,
        description: "Object version ID",
    },
];

const PUT_PARAMS: &[RegistryParameter] = &[
    PATH_PARAM,
    BUCKET_PARAM,
    KEY_PARAM,
    CONTENT_TYPE_PARAM,
    ACL_PARAM,
    META_PARAM,
    RegistryParameter {
        name: "multipart-threshold",
        required: false,
        description: "Stdin size threshold for switching to multipart upload",
    },
    NO_CLOBBER_PARAM,
    PROGRESS_PARAM,
    NO_PROGRESS_PARAM,
];

const PRESIGN_PARAMS: &[RegistryParameter] = &[
    PATH_PARAM,
    BUCKET_PARAM,
    KEY_PARAM,
    RegistryParameter {
        name: "expires",
        required: false,
        description: "URL expiration time in seconds",
    },
    RegistryParameter {
        name: "method",
        required: false,
        description: "HTTP method: GET or PUT",
    },
];

const EMPTY_PARAMS: &[RegistryParameter] = &[];

const API_PARAMS: &[RegistryParameter] = &[
    RegistryParameter {
        name: "group",
        required: true,
        description: "API metadata group name",
    },
    RegistryParameter {
        name: "action",
        required: true,
        description: "API metadata action name",
    },
    RegistryParameter {
        name: "request",
        required: false,
        description: "Optional dry-run request JSON or file://path",
    },
];

const COMPLETION_PARAMS: &[RegistryParameter] = &[RegistryParameter {
    name: "shell",
    required: true,
    description: "Shell type: bash, zsh, fish, or powershell",
}];

const SERVE_PARAMS: &[RegistryParameter] = &[
    RegistryParameter {
        name: "mcp",
        required: false,
        description: "Start the MCP server instead of returning registry metadata",
    },
    RegistryParameter {
        name: "transport",
        required: false,
        description: "MCP transport: stdio or sse",
    },
    RegistryParameter {
        name: "port",
        required: false,
        description: "SSE listen port",
    },
];

pub const CAPABILITIES: &[CapabilityRow] = &[
    row(
        "tos cp",
        "cp",
        "high_level",
        "Copy local files, TOS objects, or prefixes",
        "high",
        false,
        true,
        CP_PARAMS,
        &[
            "ListObjectsType2",
            "HeadObject",
            "GetObject",
            "PutObject",
            "CopyObject",
            "CreateMultipartUpload",
            "UploadPart",
            "UploadPartCopy",
            "ListParts",
            "CompleteMultipartUpload",
        ],
        &["tos-cli cp ./a.txt tos://bucket/a.txt"],
    ),
    row(
        "tos mv",
        "mv",
        "high_level",
        "Move files or objects by copy plus source delete",
        "critical",
        true,
        true,
        MV_PARAMS,
        &[
            "ListObjectsType2",
            "HeadObject",
            "GetObject",
            "PutObject",
            "CopyObject",
            "CreateMultipartUpload",
            "UploadPart",
            "UploadPartCopy",
            "ListParts",
            "CompleteMultipartUpload",
            "DeleteObject",
        ],
        &["tos-cli mv tos://bucket/a.txt tos://bucket/b.txt --force --confirm tos://bucket/a.txt"],
    ),
    row(
        "tos sync",
        "sync",
        "high_level",
        "Synchronize source and destination incrementally",
        "critical",
        true,
        true,
        SYNC_PARAMS,
        &[
            "ListObjectsType2",
            "HeadObject",
            "GetObject",
            "PutObject",
            "CopyObject",
            "CreateMultipartUpload",
            "UploadPart",
            "UploadPartCopy",
            "ListParts",
            "CompleteMultipartUpload",
            "DeleteObject",
        ],
        &["tos-cli sync ./dir tos://bucket/dir/"],
    ),
    row(
        "tos mkdir",
        "mkdir",
        "high_level",
        "Create a folder marker",
        "medium",
        false,
        true,
        MKDIR_PARAMS,
        &["PutObject"],
        &["tos-cli mkdir tos://bucket/path/"],
    ),
    row(
        "tos rm",
        "rm",
        "high_level",
        "Delete objects or prefixes",
        "critical",
        true,
        true,
        RM_PARAMS,
        &[
            "DeleteObject",
            "ListObjectsType2",
            "ListObjectVersions",
            "ListMultipartUploads",
            "AbortMultipartUpload",
        ],
        &["tos-cli rm tos://bucket/path/ --recursive --force --confirm tos://bucket/path/"],
    ),
    row(
        "tos ls",
        "ls",
        "high_level",
        "List object prefixes or objects within a bucket",
        "low",
        false,
        true,
        LS_PARAMS,
        &["ListObjectsType2"],
        &["tos-cli ls tos://bucket/prefix/"],
    ),
    row(
        "tos stat",
        "stat",
        "high_level",
        "Show bucket or object metadata",
        "low",
        false,
        true,
        STAT_PARAMS,
        &["HeadBucket", "HeadObject"],
        &["tos-cli stat tos://bucket/a.txt"],
    ),
    row(
        "tos du",
        "du",
        "high_level",
        "Calculate size statistics for a prefix",
        "low",
        false,
        true,
        DU_PARAMS,
        &["ListObjectsType2"],
        &["tos-cli du tos://bucket/prefix/ --human-readable"],
    ),
    row(
        "tos find",
        "find",
        "high_level",
        "Find objects by filters",
        "low",
        false,
        true,
        FIND_PARAMS,
        &["ListObjectsType2"],
        &["tos-cli find tos://bucket/ --name \"*.log\""],
    ),
    row(
        "tos cat",
        "cat",
        "high_level",
        "Stream object content",
        "low",
        false,
        true,
        CAT_PARAMS,
        &["GetObject"],
        &["tos-cli cat tos://bucket/a.txt"],
    ),
    row(
        "tos put",
        "put",
        "high_level",
        "Upload stdin to an object",
        "high",
        false,
        true,
        PUT_PARAMS,
        &[
            "PutObject",
            "CreateMultipartUpload",
            "UploadPart",
            "CompleteMultipartUpload",
            "AbortMultipartUpload",
        ],
        &["echo hello | tos-cli put tos://bucket/hello.txt"],
    ),
    row(
        "tos presign",
        "presign",
        "high_level",
        "Generate presigned URL",
        "low",
        false,
        true,
        PRESIGN_PARAMS,
        &["Presign"],
        &["tos-cli presign tos://bucket/a.txt"],
    ),
    row(
        "tos capabilities",
        "capabilities",
        "utilities",
        "Discover CLI capabilities",
        "low",
        false,
        false,
        EMPTY_PARAMS,
        &[],
        &["tos-cli capabilities --view full"],
    ),
    row(
        "tos api",
        "api",
        "utilities",
        "Guarded API metadata and dry-run planning utility",
        "low",
        false,
        true,
        API_PARAMS,
        &[],
        &["tos-cli api object list --describe"],
    ),
    row(
        "tos config",
        "config",
        "utilities",
        "Configuration management",
        "low",
        false,
        true,
        EMPTY_PARAMS,
        &[],
        &["tos-cli config show"],
    ),
    row(
        "tos completion",
        "completion",
        "utilities",
        "Generate shell completion",
        "low",
        false,
        false,
        COMPLETION_PARAMS,
        &[],
        &["tos-cli completion bash --output json"],
    ),
    row(
        "tos serve",
        "serve",
        "utilities",
        "Start or plan MCP serving",
        "low",
        false,
        true,
        SERVE_PARAMS,
        &[],
        &["tos-cli serve --mcp --dry-run --output json"],
    ),
    row(
        "tos skill",
        "skill",
        "utilities",
        "List TOS skill metadata or export Markdown SKILL.md files",
        "low",
        false,
        true,
        EMPTY_PARAMS,
        &[],
        &[
            "tos-cli skill list",
            "tos-cli skill export --dir ./tos-skills",
            "tos-cli skill export --language zh --dir ./tos-skills-zh",
        ],
    ),
    row(
        "tos doctor",
        "doctor",
        "utilities",
        "Environment diagnostics",
        "low",
        false,
        false,
        EMPTY_PARAMS,
        &[],
        &["tos-cli doctor"],
    ),
];

const fn row(
    command: &'static str,
    domain: &'static str,
    layer: &'static str,
    description: &'static str,
    risk_level: &'static str,
    destructive: bool,
    supports_dry_run: bool,
    parameters: &'static [RegistryParameter],
    api_actions: &'static [&'static str],
    examples: &'static [&'static str],
) -> CapabilityRow {
    CapabilityRow {
        command,
        domain,
        group: layer,
        layer,
        description,
        risk_level,
        destructive,
        supports_force: destructive,
        supports_dry_run,
        api_actions,
        parameters,
        examples,
    }
}

pub fn capabilities() -> &'static [CapabilityRow] {
    CAPABILITIES
}

pub fn find_capability(command: &str) -> Option<&'static CapabilityRow> {
    capabilities().iter().find(|row| row.command == command)
}

pub fn command_domains() -> Vec<&'static str> {
    let mut domains = capabilities()
        .iter()
        .map(|row| business_domain(row.command))
        .collect::<Vec<_>>();
    domains.sort_unstable();
    domains.dedup();
    domains
}

pub fn business_domain(command: &str) -> &'static str {
    let root = command.split_whitespace().nth(1).unwrap_or(command);
    match find_capability(command).map(|row| row.group).unwrap_or("") {
        "high_level" => "tos-transfer",
        "utilities" => match root {
            "doctor" | "capabilities" => "tos-admin",
            _ => "tos-shared",
        },
        _ => "tos-shared",
    }
}

pub fn public_tos_command(command: &str) -> String {
    let prefix = std::env::var("VE_STORAGE_UNI_BYTED_TOS_EXAMPLE_PREFIX")
        .unwrap_or_else(|_| "tos-cli".to_string());
    command
        .strip_prefix("tos ")
        .or_else(|| command.strip_prefix("tos-cli "))
        .or_else(|| command.strip_prefix("ve-storage-uni-cli tos "))
        .map(|suffix| format!("{prefix} {suffix}"))
        .unwrap_or_else(|| command.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parameter_names(command: &str) -> Vec<&'static str> {
        find_capability(command)
            .expect("capability")
            .parameters
            .iter()
            .map(|parameter| parameter.name)
            .collect()
    }

    #[test]
    fn high_level_registry_lists_sdk_multipart_actions() {
        let cp = find_capability("tos cp").expect("cp capability");
        for api in [
            "CreateMultipartUpload",
            "UploadPart",
            "UploadPartCopy",
            "ListParts",
            "CompleteMultipartUpload",
        ] {
            assert!(
                cp.api_actions.contains(&api),
                "tos cp should advertise {api}"
            );
        }

        let rm = find_capability("tos rm").expect("rm capability");
        for api in [
            "ListObjectVersions",
            "ListMultipartUploads",
            "AbortMultipartUpload",
        ] {
            assert!(
                rm.api_actions.contains(&api),
                "tos rm should advertise {api}"
            );
        }
    }

    #[test]
    fn tos_high_level_registry_exposes_batch_controls_without_ve_tos_only_flags() {
        for command in ["tos cp", "tos mv", "tos sync"] {
            let parameters = parameter_names(command);
            for parameter in [
                "batch-concurrency",
                "list-concurrency",
                "checkpoint-threshold",
                "multipart-concurrency",
                "progress-granularity",
                "overwrite-strategy",
                "manifest-path",
                "no-manifest",
                "progress",
                "no-progress",
            ] {
                assert!(
                    parameters.contains(&parameter),
                    "{command} should advertise {parameter}"
                );
            }
            assert!(
                !parameters.contains(&"storage-class"),
                "{command} must not advertise ve-tos-only --storage-class"
            );
        }

        let rm = parameter_names("tos rm");
        for parameter in [
            "batch-concurrency",
            "list-concurrency",
            "manifest-path",
            "no-manifest",
            "all-versions",
            "include-uploads",
        ] {
            assert!(
                rm.contains(&parameter),
                "tos rm should advertise {parameter}"
            );
        }
        assert!(
            !rm.contains(&"recursive-delete-mode"),
            "tos rm must not advertise ve-tos-only --recursive-delete-mode"
        );

        let put = parameter_names("tos put");
        assert!(put.contains(&"multipart-threshold"));
        assert!(!put.contains(&"storage-class"));
    }
}
