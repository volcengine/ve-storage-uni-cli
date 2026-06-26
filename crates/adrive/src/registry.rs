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

const BY_NAME_PARAMETER: RegistryParameter = RegistryParameter {
    name: "by-name",
    required: false,
    description: "Treat ADrive instance/space target segments as names and resolve them to IDs",
};

const TARGET_PARAMS: &[RegistryParameter] = &[
    BY_NAME_PARAMETER,
    RegistryParameter {
        name: "instance",
        required: true,
        description: "ADrive instance identifier",
    },
    RegistryParameter {
        name: "space",
        required: true,
        description: "ADrive space identifier",
    },
    RegistryParameter {
        name: "folder",
        required: false,
        description: "Folder path inside the space",
    },
    RegistryParameter {
        name: "file",
        required: false,
        description: "File name inside the folder",
    },
];

const DU_PARAMS: &[RegistryParameter] = &[
    BY_NAME_PARAMETER,
    RegistryParameter {
        name: "path",
        required: false,
        description: "Optional adrive://instance/space/folder target",
    },
    // [Review Fix #1] `ve-adrive du` accepts either a URI path or
    // --instance/--space selectors; the flat registry cannot express that
    // conditional requirement, so each selector must stay optional.
    RegistryParameter {
        name: "instance",
        required: false,
        description: "ADrive instance identifier",
    },
    RegistryParameter {
        name: "space",
        required: false,
        description: "ADrive space identifier",
    },
    RegistryParameter {
        name: "folder",
        required: false,
        description: "Folder path inside the space",
    },
    RegistryParameter {
        name: "human-readable",
        required: false,
        description: "Render human-readable total size",
    },
    RegistryParameter {
        name: "max-depth",
        required: false,
        description: "Maximum directory aggregation depth",
    },
    RegistryParameter {
        name: "top-k",
        required: false,
        description: "Number of largest and oldest file samples to keep in verbose diagnostics; 0 disables samples",
    },
    RegistryParameter {
        name: "cost",
        required: false,
        description: "Include estimated monthly storage cost by storage class",
    },
    RegistryParameter {
        name: "storage-price",
        required: false,
        description: "Override storage price as CLASS=PRICE in CNY/GB/month",
    },
    RegistryParameter {
        name: "manifest-path",
        required: false,
        description: "Optionally write traversed-file manifest CSV base path",
    },
    RegistryParameter {
        name: "list-concurrency",
        required: false,
        description: "Maximum folder prefixes listed concurrently while measuring recursively",
    },
    RegistryParameter {
        name: "list-echo",
        required: false,
        description: "Enable traversal echo output",
    },
    RegistryParameter {
        name: "no-list-echo",
        required: false,
        description: "Disable traversal echo output",
    },
    RegistryParameter {
        name: "progress",
        required: false,
        description: "Legacy alias to enable traversal echo when list echo flags are absent",
    },
    RegistryParameter {
        name: "no-progress",
        required: false,
        description: "Legacy alias to disable traversal echo when list echo flags are absent",
    },
];

const PUT_PARAMS: &[RegistryParameter] = &[
    BY_NAME_PARAMETER,
    RegistryParameter {
        name: "path",
        required: true,
        description: "ADrive destination URI: adrive://instance/space/path",
    },
    RegistryParameter {
        name: "content-type",
        required: false,
        description: "Content-Type for uploaded stdin",
    },
    RegistryParameter {
        name: "multipart-threshold",
        required: false,
        description:
            "Stdin size threshold for multipart upload; defaults to shared checkpoint_threshold",
    },
    RegistryParameter {
        name: "no-clobber",
        required: false,
        description: "Fail when the destination file already exists",
    },
    RegistryParameter {
        name: "progress",
        required: false,
        description: "Enable execution progress output on stderr",
    },
    RegistryParameter {
        name: "no-progress",
        required: false,
        description: "Disable execution progress output on stderr",
    },
];

const MKDIR_PARAMS: &[RegistryParameter] = &[
    BY_NAME_PARAMETER,
    RegistryParameter {
        name: "instance",
        required: true,
        description: "ADrive instance identifier",
    },
    RegistryParameter {
        name: "space",
        required: true,
        description: "ADrive space identifier",
    },
    RegistryParameter {
        name: "folder",
        required: true,
        description: "Folder path inside the space",
    },
    RegistryParameter {
        name: "parents",
        required: false,
        description: "Create parent folders as needed",
    },
];

// [Review Fix #ADrive-Discovery] Capabilities must expose every implemented
// high-level transfer flag so agents do not have to infer behavior from help.
const CP_PARAMS: &[RegistryParameter] = &[
    BY_NAME_PARAMETER,
    RegistryParameter {
        name: "source",
        required: true,
        description: "Local path or adrive://instance/space/path source",
    },
    RegistryParameter {
        name: "destination",
        required: true,
        description: "Local path or adrive://instance/space/path destination",
    },
    RegistryParameter {
        name: "recursive",
        required: false,
        description: "Traverse folders recursively",
    },
    RegistryParameter {
        name: "include-parent",
        required: false,
        description: "Include the source directory or prefix name under the destination path",
    },
    RegistryParameter {
        name: "force",
        required: false,
        description: "Allow overwrite/delete operations without interactive confirmation",
    },
    RegistryParameter {
        name: "include",
        required: false,
        description: "Include only paths matching this pattern during recursive transfers",
    },
    RegistryParameter {
        name: "exclude",
        required: false,
        description: "Exclude paths matching this pattern during recursive transfers",
    },
    RegistryParameter {
        name: "checkpoint",
        required: false,
        description: "Enable resumable upload/download or recursive item checkpointing",
    },
    RegistryParameter {
        name: "checkpoint-dir",
        required: false,
        description: "Directory for transfer checkpoint state",
    },
    RegistryParameter {
        name: "checkpoint-threshold",
        required: false,
        description: "File size threshold for checkpoint multipart/range transfer",
    },
    RegistryParameter {
        name: "batch-concurrency",
        required: false,
        description: "Maximum files/items running concurrently in batch execution",
    },
    RegistryParameter {
        name: "list-concurrency",
        required: false,
        description: "Maximum folder prefixes listed concurrently in recursive batch commands",
    },
    RegistryParameter {
        name: "multipart-concurrency",
        required: false,
        description: "Maximum parts/ranges running concurrently for one large file",
    },
    RegistryParameter {
        name: "progress-granularity",
        required: false,
        description: "Progress granularity: part or byte",
    },
    RegistryParameter {
        name: "overwrite-strategy",
        required: false,
        description: "Destination overwrite strategy",
    },
    RegistryParameter {
        name: "report-path",
        required: false,
        description: "Write success/failure report CSV base path",
    },
    RegistryParameter {
        name: "report-failures-only",
        required: false,
        description: "Write only failed items to the batch report",
    },
    RegistryParameter {
        name: "manifest-path",
        required: false,
        description: "Write planned transfer manifest CSV base path",
    },
    RegistryParameter {
        name: "no-manifest",
        required: false,
        description: "Disable planned manifest output",
    },
    RegistryParameter {
        name: "bandwidth-limit",
        required: false,
        description: "Throttle upload/download bandwidth, e.g. 100MB",
    },
    RegistryParameter {
        name: "list-echo",
        required: false,
        description: "Enable listing-phase echo output on stderr",
    },
    RegistryParameter {
        name: "no-list-echo",
        required: false,
        description: "Disable listing-phase echo output on stderr",
    },
    RegistryParameter {
        name: "progress",
        required: false,
        description: "Enable execution progress output on stderr",
    },
    RegistryParameter {
        name: "no-progress",
        required: false,
        description: "Disable execution progress output on stderr",
    },
    RegistryParameter {
        name: "no-clobber",
        required: false,
        description: "Fail when the destination already exists",
    },
];

const MV_PARAMS: &[RegistryParameter] = &[
    BY_NAME_PARAMETER,
    RegistryParameter {
        name: "source",
        required: true,
        description: "Local path or adrive://instance/space/path source",
    },
    RegistryParameter {
        name: "destination",
        required: true,
        description: "Local path or adrive://instance/space/path destination",
    },
    RegistryParameter {
        name: "recursive",
        required: false,
        description: "Traverse folders recursively",
    },
    RegistryParameter {
        name: "include-parent",
        required: false,
        description: "Include the source directory or prefix name under the destination path",
    },
    RegistryParameter {
        name: "force",
        required: false,
        description: "Allow overwrite/delete operations without interactive confirmation",
    },
    RegistryParameter {
        name: "include",
        required: false,
        description: "Include only paths matching this pattern during recursive moves",
    },
    RegistryParameter {
        name: "exclude",
        required: false,
        description: "Exclude paths matching this pattern during recursive moves",
    },
    RegistryParameter {
        name: "checkpoint-dir",
        required: false,
        description: "Directory reserved for transfer checkpoint state",
    },
    RegistryParameter {
        name: "checkpoint-threshold",
        required: false,
        description: "File size threshold for checkpoint multipart/range transfer",
    },
    RegistryParameter {
        name: "batch-concurrency",
        required: false,
        description: "Maximum files/items running concurrently in batch execution",
    },
    RegistryParameter {
        name: "list-concurrency",
        required: false,
        description: "Maximum folder prefixes listed concurrently in recursive batch commands",
    },
    RegistryParameter {
        name: "multipart-concurrency",
        required: false,
        description: "Maximum parts/ranges running concurrently for one large file",
    },
    RegistryParameter {
        name: "progress-granularity",
        required: false,
        description: "Progress granularity: part or byte",
    },
    RegistryParameter {
        name: "overwrite-strategy",
        required: false,
        description: "Destination overwrite strategy",
    },
    RegistryParameter {
        name: "report-path",
        required: false,
        description: "Write success/failure report CSV base path",
    },
    RegistryParameter {
        name: "report-failures-only",
        required: false,
        description: "Write only failed items to the batch report",
    },
    RegistryParameter {
        name: "manifest-path",
        required: false,
        description: "Write planned transfer manifest CSV base path",
    },
    RegistryParameter {
        name: "no-manifest",
        required: false,
        description: "Disable planned manifest output",
    },
    RegistryParameter {
        name: "list-echo",
        required: false,
        description: "Enable listing-phase echo output on stderr",
    },
    RegistryParameter {
        name: "no-list-echo",
        required: false,
        description: "Disable listing-phase echo output on stderr",
    },
    RegistryParameter {
        name: "progress",
        required: false,
        description: "Enable execution progress output on stderr",
    },
    RegistryParameter {
        name: "no-progress",
        required: false,
        description: "Disable execution progress output on stderr",
    },
];

const SYNC_PARAMS: &[RegistryParameter] = &[
    BY_NAME_PARAMETER,
    RegistryParameter {
        name: "source",
        required: true,
        description: "Local path or adrive://instance/space/path source",
    },
    RegistryParameter {
        name: "destination",
        required: true,
        description: "Local path or adrive://instance/space/path destination",
    },
    RegistryParameter {
        name: "delete",
        required: false,
        description: "Delete extraneous destination files/folders",
    },
    RegistryParameter {
        name: "force",
        required: false,
        description: "Required safety gate when --delete is enabled",
    },
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
    RegistryParameter {
        name: "include-parent",
        required: false,
        description: "Include the source directory or prefix name under the destination path",
    },
    RegistryParameter {
        name: "include",
        required: false,
        description: "Include only paths matching this pattern",
    },
    RegistryParameter {
        name: "exclude",
        required: false,
        description: "Exclude paths matching this pattern",
    },
    RegistryParameter {
        name: "checkpoint-dir",
        required: false,
        description: "Directory reserved for transfer checkpoint state",
    },
    RegistryParameter {
        name: "checkpoint-threshold",
        required: false,
        description: "File size threshold for checkpoint multipart/range transfer",
    },
    RegistryParameter {
        name: "batch-concurrency",
        required: false,
        description: "Maximum files/items running concurrently in batch execution",
    },
    RegistryParameter {
        name: "list-concurrency",
        required: false,
        description: "Maximum folder prefixes listed concurrently in recursive batch commands",
    },
    RegistryParameter {
        name: "multipart-concurrency",
        required: false,
        description: "Maximum parts/ranges running concurrently for one large file",
    },
    RegistryParameter {
        name: "progress-granularity",
        required: false,
        description: "Progress granularity: part or byte",
    },
    RegistryParameter {
        name: "overwrite-strategy",
        required: false,
        description: "Destination overwrite strategy",
    },
    RegistryParameter {
        name: "report-path",
        required: false,
        description: "Write success/failure report CSV base path",
    },
    RegistryParameter {
        name: "report-failures-only",
        required: false,
        description: "Write only failed items to the batch report",
    },
    RegistryParameter {
        name: "manifest-path",
        required: false,
        description: "Write planned transfer manifest CSV base path",
    },
    RegistryParameter {
        name: "no-manifest",
        required: false,
        description: "Disable planned manifest output",
    },
    RegistryParameter {
        name: "bandwidth-limit",
        required: false,
        description: "Throttle upload/download bandwidth, e.g. 100MB",
    },
    RegistryParameter {
        name: "list-echo",
        required: false,
        description: "Enable listing-phase echo output on stderr",
    },
    RegistryParameter {
        name: "no-list-echo",
        required: false,
        description: "Disable listing-phase echo output on stderr",
    },
    RegistryParameter {
        name: "progress",
        required: false,
        description: "Enable execution progress output on stderr",
    },
    RegistryParameter {
        name: "no-progress",
        required: false,
        description: "Disable execution progress output on stderr",
    },
];

const CREATE_PARAMS: &[RegistryParameter] = &[
    BY_NAME_PARAMETER,
    RegistryParameter {
        name: "path",
        required: false,
        description: "adrive://instance-name or adrive://instance-id/space-name target",
    },
    RegistryParameter {
        name: "instance",
        required: false,
        description: "Instance name to create, or existing instance ID when --space is set",
    },
    RegistryParameter {
        name: "space",
        required: false,
        description: "Space name to create under --instance",
    },
    RegistryParameter {
        name: "display-name",
        required: false,
        description: "Display name for the created instance or space",
    },
    RegistryParameter {
        name: "description",
        required: false,
        description: "Description for the created instance or space",
    },
    RegistryParameter {
        name: "index-enabled",
        required: false,
        description: "Enable search indexing for a newly-created space",
    },
];

const DELETE_PARAMS: &[RegistryParameter] = &[
    BY_NAME_PARAMETER,
    RegistryParameter {
        name: "path",
        required: false,
        description: "adrive://instance-id or adrive://instance-id/space-id target",
    },
    RegistryParameter {
        name: "instance",
        required: false,
        description: "Instance ID to delete, or containing instance ID when --space is set",
    },
    RegistryParameter {
        name: "space",
        required: false,
        description: "Space ID to delete under --instance",
    },
    RegistryParameter {
        name: "force",
        required: true,
        description: "Required safety gate for destructive deletion; non-interactive critical execution also requires global --confirm <target>",
    },
];

const RM_PARAMS: &[RegistryParameter] = &[
    BY_NAME_PARAMETER,
    RegistryParameter {
        name: "path",
        required: false,
        description: "adrive://instance/space/folder[/file] target",
    },
    RegistryParameter {
        name: "instance",
        required: false,
        description: "ADrive instance identifier",
    },
    RegistryParameter {
        name: "space",
        required: false,
        description: "ADrive space identifier",
    },
    RegistryParameter {
        name: "folder",
        required: false,
        description: "Folder path inside the space",
    },
    RegistryParameter {
        name: "file",
        required: false,
        description: "File name inside the folder",
    },
    RegistryParameter {
        name: "recursive",
        required: false,
        description: "Delete folders recursively when supported",
    },
    RegistryParameter {
        name: "recursive-delete-mode",
        required: false,
        description: "Recursive folder delete strategy: bottom-up or direct",
    },
    RegistryParameter {
        name: "force",
        required: true,
        description: "Required safety gate for destructive deletion; non-interactive critical execution also requires global --confirm <target>",
    },
    RegistryParameter {
        name: "include-uploads",
        required: false,
        description: "Also abort incomplete multipart uploads recorded in ADrive checkpoints matching the target",
    },
    RegistryParameter {
        name: "checkpoint-dir",
        required: false,
        description: "Checkpoint directory to scan when include-uploads is enabled",
    },
    RegistryParameter {
        name: "report-path",
        required: false,
        description: "Write success/failure report CSV base path",
    },
    RegistryParameter {
        name: "report-failures-only",
        required: false,
        description: "Write only failed items to the batch report",
    },
    RegistryParameter {
        name: "manifest-path",
        required: false,
        description: "Write planned delete manifest CSV base path",
    },
    RegistryParameter {
        name: "no-manifest",
        required: false,
        description: "Disable planned manifest output",
    },
    RegistryParameter {
        name: "batch-concurrency",
        required: false,
        description: "Maximum files/items running concurrently in this batch delete",
    },
    RegistryParameter {
        name: "list-concurrency",
        required: false,
        description: "Maximum folder prefixes listed concurrently in recursive batch deletes",
    },
    RegistryParameter {
        name: "include",
        required: false,
        description: "Include only paths matching this pattern during bottom-up recursive deletes",
    },
    RegistryParameter {
        name: "exclude",
        required: false,
        description: "Exclude paths matching this pattern during bottom-up recursive deletes",
    },
    RegistryParameter {
        name: "list-echo",
        required: false,
        description: "Enable listing-phase echo output on stderr",
    },
    RegistryParameter {
        name: "no-list-echo",
        required: false,
        description: "Disable listing-phase echo output on stderr",
    },
    RegistryParameter {
        name: "progress",
        required: false,
        description: "Enable execution progress output on stderr",
    },
    RegistryParameter {
        name: "no-progress",
        required: false,
        description: "Disable execution progress output on stderr",
    },
];

const LS_PARAMS: &[RegistryParameter] = &[
    BY_NAME_PARAMETER,
    RegistryParameter {
        name: "path",
        required: false,
        description: "Optional adrive://instance[/space[/folder]] target",
    },
    RegistryParameter {
        name: "instance",
        required: false,
        description: "List spaces under this instance when space is omitted",
    },
    RegistryParameter {
        name: "space",
        required: false,
        description: "List files under this space when provided",
    },
    RegistryParameter {
        name: "folder",
        required: false,
        description: "Folder prefix for file listing",
    },
    RegistryParameter {
        name: "max-keys",
        required: false,
        description: "Maximum entries to return from the current directory level",
    },
    RegistryParameter {
        name: "marker",
        required: false,
        description: "Pagination marker returned by a previous listing",
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
        description: "Comma-separated table/csv columns",
    },
    RegistryParameter {
        name: "manifest-path",
        required: false,
        description: "Optionally write listing manifest CSV base path",
    },
];

const FIND_PARAMS: &[RegistryParameter] = &[
    BY_NAME_PARAMETER,
    RegistryParameter {
        name: "path",
        required: false,
        description: "Optional adrive://instance/space/folder target",
    },
    RegistryParameter {
        name: "instance",
        required: false,
        description: "ADrive instance identifier",
    },
    RegistryParameter {
        name: "space",
        required: false,
        description: "ADrive space identifier",
    },
    RegistryParameter {
        name: "folder",
        required: false,
        description: "Folder path inside the space",
    },
    RegistryParameter {
        name: "name",
        required: false,
        description: "Name glob or substring",
    },
    RegistryParameter {
        name: "size",
        required: false,
        description: "Size predicate such as +100MB or -1GB",
    },
    RegistryParameter {
        name: "mtime",
        required: false,
        description: "Relative modified time predicate such as -7d",
    },
    RegistryParameter {
        name: "manifest-path",
        required: false,
        description: "Optionally write matched-file manifest CSV base path",
    },
    RegistryParameter {
        name: "list-echo",
        required: false,
        description: "Enable traversal echo output",
    },
    RegistryParameter {
        name: "no-list-echo",
        required: false,
        description: "Disable traversal echo output",
    },
    RegistryParameter {
        name: "progress",
        required: false,
        description: "Legacy alias to enable traversal echo when list echo flags are absent",
    },
    RegistryParameter {
        name: "no-progress",
        required: false,
        description: "Legacy alias to disable traversal echo when list echo flags are absent",
    },
];

const API_PARAMS: &[RegistryParameter] = &[
    RegistryParameter {
        name: "group",
        required: true,
        description: "IDS API group",
    },
    RegistryParameter {
        name: "action",
        required: true,
        description: "IDS API action",
    },
    RegistryParameter {
        name: "request",
        required: false,
        description: "JSON request body or file:// path",
    },
    RegistryParameter {
        name: "force",
        required: false,
        description: "Reserved for future ADrive raw API execution; currently unimplemented",
    },
];

const COMPLETION_PARAMS: &[RegistryParameter] = &[RegistryParameter {
    name: "shell",
    required: true,
    description: "Shell name: bash, zsh, fish, or powershell",
}];

const SERVE_PARAMS: &[RegistryParameter] = &[
    RegistryParameter {
        name: "mcp",
        required: false,
        description: "Enable the long-running MCP runtime",
    },
    RegistryParameter {
        name: "transport",
        required: false,
        description: "MCP transport: stdio or sse",
    },
    RegistryParameter {
        name: "port",
        required: false,
        description: "SSE port; runtime binds 127.0.0.1:<port>",
    },
];

pub const CAPABILITIES: &[CapabilityRow] = &[
    CapabilityRow {
        command: "ve-adrive cp",
        domain: "cp",
        group: "High-Level",
        layer: "high-level",
        description: "Copy local files, ADrive files, or folders",
        risk_level: "medium",
        destructive: false,
        supports_force: true,
        supports_dry_run: true,
        api_actions: &[
            "get_instance",
            "get_space",
            "put_file",
            "get_file",
            "copy_file",
            "initiate_multipart_upload",
            "upload_part",
            "complete_multipart_upload",
            "abort_multipart_upload",
        ],
        parameters: CP_PARAMS,
        examples: &["ve-adrive-cli cp ./a.txt adrive://inst/space/docs/a.txt"],
    },
    CapabilityRow {
        command: "ve-adrive mv",
        domain: "mv",
        group: "High-Level",
        layer: "high-level",
        description: "Move files or folders by same-space rename or copy plus source delete",
        risk_level: "critical",
        destructive: true,
        supports_force: true,
        supports_dry_run: true,
        api_actions: &[
            "get_instance",
            "get_space",
            "rename_file",
            "rename_folder",
            "copy_file",
            "delete_file",
        ],
        parameters: MV_PARAMS,
        examples: &[
            "ve-adrive-cli mv adrive://inst/space/a.txt adrive://inst/space/b.txt --force --confirm adrive://inst/space/a.txt",
        ],
    },
    CapabilityRow {
        command: "ve-adrive sync",
        domain: "sync",
        group: "High-Level",
        layer: "high-level",
        description: "Synchronize source and destination incrementally",
        risk_level: "critical",
        destructive: true,
        supports_force: true,
        supports_dry_run: true,
        api_actions: &[
            "get_instance",
            "get_space",
            "list_files",
            "put_file",
            "get_file",
            "delete_file",
        ],
        parameters: SYNC_PARAMS,
        examples: &[
            "ve-adrive-cli sync ./dir adrive://inst/space/dir --delete --force --confirm adrive://inst/space/dir",
        ],
    },
    CapabilityRow {
        command: "ve-adrive crt",
        domain: "crt",
        group: "High-Level",
        layer: "high-level",
        description: "Create an instance or space",
        risk_level: "medium",
        destructive: false,
        supports_force: false,
        supports_dry_run: true,
        api_actions: &["get_instance", "create_instance", "create_space"],
        parameters: CREATE_PARAMS,
        examples: &[
            "ve-adrive-cli crt adrive://inst-name",
            "ve-adrive-cli crt adrive://inst-id/space-name",
        ],
    },
    CapabilityRow {
        command: "ve-adrive del",
        domain: "del",
        group: "High-Level",
        layer: "high-level",
        description: "Delete an instance or space",
        risk_level: "critical",
        destructive: true,
        supports_force: true,
        supports_dry_run: true,
        api_actions: &["get_instance", "get_space", "delete_instance", "delete_space"],
        parameters: DELETE_PARAMS,
        examples: &[
            "ve-adrive-cli del adrive://inst-id --force --confirm adrive://inst-id",
            "ve-adrive-cli del --instance inst-id --space space-id --force --confirm adrive://inst-id/space-id",
        ],
    },
    CapabilityRow {
        command: "ve-adrive rm",
        domain: "rm",
        group: "High-Level",
        layer: "high-level",
        description: "Delete a file, folder, or recursively clear a space",
        risk_level: "critical",
        destructive: true,
        supports_force: true,
        supports_dry_run: true,
        api_actions: &[
            "get_instance",
            "get_space",
            "delete_file",
            "delete_folder",
            "abort_multipart_upload",
        ],
        parameters: RM_PARAMS,
        examples: &[
            "ve-adrive-cli rm adrive://inst/space/docs/a.txt --force --confirm adrive://inst/space/docs/a.txt",
            "ve-adrive-cli rm adrive://inst/space/docs/ --recursive --recursive-delete-mode direct --force --confirm adrive://inst/space/docs/",
            "ve-adrive-cli rm adrive://inst/space --recursive --include-uploads --force --confirm adrive://inst/space",
        ],
    },
    CapabilityRow {
        command: "ve-adrive ls",
        domain: "ls",
        group: "High-Level",
        layer: "high-level",
        description: "List instances, spaces, or files by target depth",
        risk_level: "low",
        destructive: false,
        supports_force: false,
        supports_dry_run: true,
        api_actions: &[
            "get_instance",
            "get_space",
            "list_instances",
            "list_spaces",
            "list_files",
        ],
        parameters: LS_PARAMS,
        examples: &["ve-adrive-cli ls adrive://inst/space/docs/"],
    },
    CapabilityRow {
        command: "ve-adrive stat",
        domain: "stat",
        group: "High-Level",
        layer: "high-level",
        description: "Show file or folder metadata",
        risk_level: "low",
        destructive: false,
        supports_force: false,
        supports_dry_run: true,
        api_actions: &["get_instance", "get_space", "head_file"],
        parameters: TARGET_PARAMS,
        examples: &["ve-adrive-cli stat adrive://inst/space/docs/a.txt"],
    },
    CapabilityRow {
        command: "ve-adrive du",
        domain: "du",
        group: "High-Level",
        layer: "high-level",
        description: "Calculate file size statistics for a folder",
        risk_level: "low",
        destructive: false,
        supports_force: false,
        supports_dry_run: true,
        api_actions: &["get_instance", "get_space", "list_files"],
        parameters: DU_PARAMS,
        examples: &["ve-adrive-cli du --instance inst --space space --folder docs --cost"],
    },
    CapabilityRow {
        command: "ve-adrive find",
        domain: "find",
        group: "High-Level",
        layer: "high-level",
        description: "Find files by name, size, or mtime",
        risk_level: "low",
        destructive: false,
        supports_force: false,
        supports_dry_run: true,
        api_actions: &["get_instance", "get_space", "list_files"],
        parameters: FIND_PARAMS,
        examples: &["ve-adrive-cli find adrive://inst/space/docs --name '*.txt'"],
    },
    CapabilityRow {
        command: "ve-adrive cat",
        domain: "cat",
        group: "High-Level",
        layer: "high-level",
        description: "Stream file content",
        risk_level: "low",
        destructive: false,
        supports_force: false,
        supports_dry_run: true,
        api_actions: &["get_instance", "get_space", "get_file"],
        parameters: TARGET_PARAMS,
        examples: &["ve-adrive-cli cat adrive://inst/space/docs/a.txt"],
    },
    CapabilityRow {
        command: "ve-adrive put",
        domain: "put",
        group: "High-Level",
        layer: "high-level",
        description: "Upload stdin to a file",
        risk_level: "medium",
        destructive: false,
        supports_force: false,
        supports_dry_run: true,
        api_actions: &[
            "get_instance",
            "get_space",
            "put_file",
            "initiate_multipart_upload",
            "upload_part",
            "complete_multipart_upload",
            "abort_multipart_upload",
        ],
        parameters: PUT_PARAMS,
        examples: &[
            "ve-adrive-cli cat adrive://inst/space/docs/a.txt | gzip | ve-adrive-cli put adrive://inst/space/docs/a.txt.gz",
        ],
    },
    CapabilityRow {
        command: "ve-adrive mkdir",
        domain: "mkdir",
        group: "High-Level",
        layer: "high-level",
        description: "Create a folder",
        risk_level: "medium",
        destructive: false,
        supports_force: false,
        supports_dry_run: true,
        api_actions: &["get_instance", "get_space", "create_folder"],
        parameters: MKDIR_PARAMS,
        examples: &[
            "ve-adrive-cli mkdir adrive://inst/space/docs/new/",
            "ve-adrive-cli mkdir adrive://inst/space/docs/new/deep/ --parents",
        ],
    },
    CapabilityRow {
        command: "ve-adrive capabilities",
        domain: "capabilities",
        group: "Capabilities / Utilities",
        layer: "utility",
        description: "Discover CLI capabilities",
        risk_level: "low",
        destructive: false,
        supports_force: false,
        supports_dry_run: true,
        api_actions: &[],
        parameters: &[],
        examples: &["ve-adrive-cli capabilities --view full"],
    },
    CapabilityRow {
        command: "ve-adrive api",
        domain: "api",
        group: "Capabilities / Utilities",
        layer: "utility",
        description: "Inspect API metadata; execution is unimplemented",
        risk_level: "medium",
        destructive: false,
        supports_force: false,
        supports_dry_run: true,
        api_actions: &["raw_passthrough_plan"],
        parameters: API_PARAMS,
        examples: &[
            "ve-adrive-cli api file list --describe",
            "ve-adrive-cli api instance create --dry-run --request '{\"name\":\"demo\"}'",
        ],
    },
    CapabilityRow {
        command: "ve-adrive config",
        domain: "config",
        group: "Capabilities / Utilities",
        layer: "utility",
        description: "Manage ADrive CLI configuration",
        risk_level: "medium",
        destructive: false,
        supports_force: false,
        supports_dry_run: true,
        api_actions: &[],
        parameters: &[],
        examples: &["ve-adrive-cli config show"],
    },
    CapabilityRow {
        command: "ve-adrive completion",
        domain: "completion",
        group: "Capabilities / Utilities",
        layer: "utility",
        description:
            "Generate shell completion scripts and installation snippets for ve-adrive-cli / ve-adrive",
        risk_level: "low",
        destructive: false,
        supports_force: false,
        supports_dry_run: false,
        api_actions: &[],
        parameters: COMPLETION_PARAMS,
        examples: &[
            "ve-adrive-cli completion bash",
            "ve-adrive-cli completion bash --output json | jq -r '.data.script' > ~/.ve-adrive-completion.bash",
        ],
    },
    CapabilityRow {
        command: "ve-adrive serve",
        domain: "serve",
        group: "Capabilities / Utilities",
        layer: "utility",
        description: "Start registry-backed MCP server over stdio or local HTTP/SSE",
        risk_level: "low",
        destructive: false,
        supports_force: false,
        supports_dry_run: true,
        api_actions: &["mcp_tools_list", "mcp_tools_call"],
        parameters: SERVE_PARAMS,
        examples: &[
            "ve-adrive-cli serve --mcp",
            "ve-adrive-cli serve --mcp --transport sse --port 9090",
            "ve-adrive-cli serve --mcp --transport stdio --dry-run",
        ],
    },
    CapabilityRow {
        command: "ve-adrive skill",
        domain: "skill",
        group: "Capabilities / Utilities",
        layer: "utility",
        description: "List ADrive skill metadata or export Markdown SKILL.md files for external Agents and adapters",
        risk_level: "medium",
        destructive: false,
        supports_force: false,
        supports_dry_run: true,
        api_actions: &[],
        parameters: &[],
        examples: &[
            "ve-adrive-cli skill list",
            "ve-adrive-cli skill export --dir ./ve-adrive-skills",
            "ve-adrive-cli skill export --language zh --dir ./ve-adrive-skills-zh",
        ],
    },
    CapabilityRow {
        command: "ve-adrive doctor",
        domain: "doctor",
        group: "Capabilities / Utilities",
        layer: "utility",
        description: "Environment diagnostics",
        risk_level: "low",
        destructive: false,
        supports_force: false,
        supports_dry_run: false,
        api_actions: &[],
        parameters: &[],
        examples: &["ve-adrive-cli doctor --check principles"],
    },
];

pub fn capabilities() -> &'static [CapabilityRow] {
    CAPABILITIES
}

pub fn command_domains() -> Vec<&'static str> {
    let mut domains = capabilities()
        .iter()
        .map(|row| row.domain)
        .collect::<Vec<_>>();
    domains.sort_unstable();
    domains.dedup();
    domains
}

/// Map a command to its business skill domain, mirroring the TOS taxonomy.
///
/// ADrive currently exposes only High-Level commands plus utilities, so there
/// are three domains:
///   - `adrive-transfer` high-level data movement (cp/mv/sync/ls/...)
///   - `adrive-shared`   cross-cutting tooling (config/api/serve/skill/...)
///   - `adrive-admin`    operational helpers (doctor/capabilities)
///
/// Classification is derived from the capability's `layer` (and command root
/// within the utility layer) so new commands are grouped automatically.
pub fn business_domain(command: &str) -> &'static str {
    let row = find_capability(command);
    let layer = row.map(|row| row.layer).unwrap_or("");
    if layer == "high-level" {
        return "adrive-transfer";
    }
    let root = command.split_whitespace().nth(1).unwrap_or(command);
    match root {
        "doctor" | "capabilities" => "adrive-admin",
        _ => "adrive-shared",
    }
}

pub fn business_domains() -> Vec<&'static str> {
    let mut domains = capabilities()
        .iter()
        .map(|row| business_domain(row.command))
        .collect::<Vec<_>>();
    domains.sort_unstable();
    domains.dedup();
    domains
}

pub fn find_capability(command: &str) -> Option<&'static CapabilityRow> {
    capabilities().iter().find(|row| row.command == command)
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
    fn registry_uses_public_ve_adrive_command_keys() {
        assert!(capabilities()
            .iter()
            .all(|row| row.command.starts_with("ve-adrive")));
        assert!(capabilities()
            .iter()
            .all(|row| !row.command.starts_with("adrive ")));
        assert!(find_capability("ve-adrive cp").is_some());
        assert!(find_capability("adrive cp").is_none());
    }

    #[test]
    fn test_find_and_stat_use_their_own_parameter_sets() {
        let find_params = parameter_names("ve-adrive find");
        assert!(find_params.contains(&"name"));
        assert!(find_params.contains(&"size"));
        assert!(find_params.contains(&"mtime"));
        assert!(find_params.contains(&"manifest-path"));
        assert!(!find_params.contains(&"no-manifest"));
        assert!(!find_params.contains(&"report-failures-only"));

        let stat_params = parameter_names("ve-adrive stat");
        assert!(!stat_params.contains(&"name"));
        assert!(!stat_params.contains(&"size"));
        assert!(!stat_params.contains(&"mtime"));
        assert!(!stat_params.contains(&"manifest-path"));
        assert!(!stat_params.contains(&"no-manifest"));
        assert!(!stat_params.contains(&"report-failures-only"));
    }

    #[test]
    fn transfer_registry_exposes_batch_controls() {
        for command in ["ve-adrive cp", "ve-adrive mv", "ve-adrive sync"] {
            let parameters = parameter_names(command);
            for parameter in [
                "checkpoint-threshold",
                "batch-concurrency",
                "list-concurrency",
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
        }

        let sync_params = parameter_names("ve-adrive sync");
        assert!(sync_params.contains(&"size-only"));
        assert!(sync_params.contains(&"exact-timestamps"));

        let ls_params = parameter_names("ve-adrive ls");
        assert!(ls_params.contains(&"human-readable"));
        assert!(ls_params.contains(&"sort"));
        assert!(ls_params.contains(&"columns"));
    }
}
