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

use clap::{Command, Subcommand};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use tos_core::agent::describe::{
    CommandDescription, CommandLayer, CommandParameter, ParameterLocation, RelatedCommands,
    RiskLevel,
};

use crate::cli::TosCommand;

const TOS_EXAMPLE_PREFIX_ENV: &str = "VE_STORAGE_UNI_TOS_EXAMPLE_PREFIX";

pub const SUPPORTED_OUTPUT_FORMATS: &[&str] = &["json", "xml", "yaml", "csv", "table", "markdown"];

#[derive(Debug, Serialize)]
pub struct CommandGroupEntry {
    pub name: &'static str,
    pub command: &'static str,
    pub layer: CommandLayer,
    pub category: &'static str,
    pub description: &'static str,
    pub supports_help: bool,
    pub supports_describe: bool,
    pub implemented: bool,
}

#[derive(Debug, Serialize)]
pub struct CapabilityEntry {
    pub command: &'static str,
    pub group: &'static str,
    pub layer: CommandLayer,
    pub description: &'static str,
    pub risk_level: RiskLevel,
    pub apis: &'static [&'static str],
    pub endpoint_kind: Option<&'static str>,
    pub method: Option<&'static str>,
    pub supports_describe: bool,
    pub supports_dry_run: bool,
    pub supports_force: bool,
    pub supports_pipe: bool,
    pub supports_output_formats: &'static [&'static str],
    pub parameters: &'static [RegistryParameter],
    pub body_contract: Option<&'static str>,
    pub consistency_guards: &'static [&'static str],
    pub examples: &'static [&'static str],
    pub related_commands: &'static [&'static str],
}

#[derive(Debug, Clone, Serialize)]
pub struct RegistryParameter {
    pub name: &'static str,
    pub location: ParameterLocation,
    pub required: bool,
    pub description: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommandTreeEntry {
    pub name: String,
    pub command: String,
    pub layer: Option<String>,
    pub category: Option<String>,
    pub description: String,
    pub supports_help: bool,
    pub supports_describe: bool,
    pub implemented: bool,
    pub parameters: Vec<CommandParameterEntry>,
    pub subcommands: Vec<CommandTreeEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommandParameterEntry {
    pub name: String,
    pub required: bool,
    pub description: String,
    pub long: Option<String>,
    pub short: Option<char>,
    pub positional: bool,
    pub takes_value: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RegistryCapabilityParameter {
    pub name: String,
    pub location: String,
    pub required: bool,
    pub description: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RegistryCapabilityRow {
    pub command: String,
    pub group: String,
    pub layer: String,
    pub description: String,
    pub risk_level: String,
    pub destructive: bool,
    pub apis: Vec<String>,
    pub endpoint_rule: Option<String>,
    pub method: Option<String>,
    pub supports_describe: bool,
    pub supports_dry_run: bool,
    pub supports_force: bool,
    pub supports_pipe: bool,
    pub supports_output_formats: &'static [&'static str],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Vec<RegistryCapabilityParameter>>,
    pub body_contract: Option<String>,
    pub consistency_guards: &'static [&'static str],
    pub examples: Vec<String>,
    pub related_commands: Vec<String>,
}

pub const COMMAND_GROUPS: &[CommandGroupEntry] = &[
    group(
        "cp",
        "ve-tos cp",
        CommandLayer::HighLevel,
        "high_level",
        "Copy local files, TOS objects, or prefixes",
        true,
    ),
    group(
        "mv",
        "ve-tos mv",
        CommandLayer::HighLevel,
        "high_level",
        "Move files or objects by copy plus source delete",
        true,
    ),
    group(
        "sync",
        "ve-tos sync",
        CommandLayer::HighLevel,
        "high_level",
        "Synchronize source and destination incrementally",
        true,
    ),
    group(
        "mb",
        "ve-tos mb",
        CommandLayer::HighLevel,
        "high_level",
        "Create a bucket",
        true,
    ),
    group(
        "rb",
        "ve-tos rb",
        CommandLayer::HighLevel,
        "high_level",
        "Remove a bucket",
        true,
    ),
    group(
        "mkdir",
        "ve-tos mkdir",
        CommandLayer::HighLevel,
        "high_level",
        "Create a folder",
        true,
    ),
    group(
        "rm",
        "ve-tos rm",
        CommandLayer::HighLevel,
        "high_level",
        "Delete objects or prefixes",
        true,
    ),
    group(
        "ls",
        "ve-tos ls",
        CommandLayer::HighLevel,
        "high_level",
        "List buckets or objects",
        true,
    ),
    group(
        "stat",
        "ve-tos stat",
        CommandLayer::HighLevel,
        "high_level",
        "Show bucket or object metadata",
        true,
    ),
    group(
        "du",
        "ve-tos du",
        CommandLayer::HighLevel,
        "high_level",
        "Calculate object size statistics for a prefix",
        true,
    ),
    group(
        "find",
        "ve-tos find",
        CommandLayer::HighLevel,
        "high_level",
        "Find objects by name, size, mtime, or storage class",
        true,
    ),
    group(
        "cat",
        "ve-tos cat",
        CommandLayer::HighLevel,
        "high_level",
        "Stream object content",
        true,
    ),
    group(
        "put",
        "ve-tos put",
        CommandLayer::HighLevel,
        "high_level",
        "Upload stdin to an object",
        true,
    ),
    group(
        "presign",
        "ve-tos presign",
        CommandLayer::HighLevel,
        "high_level",
        "Generate presigned URLs",
        true,
    ),
    group(
        "restore",
        "ve-tos restore",
        CommandLayer::HighLevel,
        "high_level",
        "Restore archived objects",
        true,
    ),
    group(
        "bucket",
        "ve-tos bucket",
        CommandLayer::LowLevel,
        "core",
        "Bucket Core APIs",
        true,
    ),
    group(
        "object",
        "ve-tos object",
        CommandLayer::LowLevel,
        "core",
        "Object Core APIs",
        true,
    ),
    group(
        "multipart",
        "ve-tos multipart",
        CommandLayer::LowLevel,
        "core",
        "Multipart Core APIs",
        true,
    ),
    group(
        "turbo",
        "ve-tos turbo",
        CommandLayer::LowLevel,
        "core",
        "Turbo append upload APIs",
        true,
    ),
    group(
        "quota",
        "ve-tos quota",
        CommandLayer::LowLevel,
        "bucket_config",
        "Bucket storage quota",
        true,
    ),
    group(
        "policy",
        "ve-tos policy",
        CommandLayer::LowLevel,
        "bucket_config",
        "Bucket policy management",
        true,
    ),
    group(
        "lifecycle",
        "ve-tos lifecycle",
        CommandLayer::LowLevel,
        "bucket_config",
        "Lifecycle rule management",
        true,
    ),
    group(
        "storageclass",
        "ve-tos storageclass",
        CommandLayer::LowLevel,
        "bucket_config",
        "Bucket default storage class",
        true,
    ),
    group(
        "cors",
        "ve-tos cors",
        CommandLayer::LowLevel,
        "bucket_config",
        "Bucket CORS configuration",
        true,
    ),
    group(
        "versioning",
        "ve-tos versioning",
        CommandLayer::LowLevel,
        "bucket_config",
        "Bucket versioning configuration",
        true,
    ),
    group(
        "replication",
        "ve-tos replication",
        CommandLayer::LowLevel,
        "bucket_config",
        "Cross-region replication",
        true,
    ),
    group(
        "encryption",
        "ve-tos encryption",
        CommandLayer::LowLevel,
        "bucket_config",
        "Bucket encryption configuration",
        true,
    ),
    group(
        "custom-domain",
        "ve-tos custom-domain",
        CommandLayer::LowLevel,
        "bucket_config",
        "Custom domain binding",
        true,
    ),
    group(
        "notification",
        "ve-tos notification",
        CommandLayer::LowLevel,
        "bucket_config",
        "Event notification configuration",
        true,
    ),
    group(
        "website",
        "ve-tos website",
        CommandLayer::LowLevel,
        "bucket_config",
        "Static website hosting",
        true,
    ),
    group(
        "mirror",
        "ve-tos mirror",
        CommandLayer::LowLevel,
        "bucket_config",
        "Mirror back-to-source rules",
        true,
    ),
    group(
        "inventory",
        "ve-tos inventory",
        CommandLayer::LowLevel,
        "bucket_config",
        "Bucket inventory configuration",
        true,
    ),
    group(
        "tagging",
        "ve-tos tagging",
        CommandLayer::LowLevel,
        "bucket_config",
        "Bucket tagging management",
        true,
    ),
    group(
        "acl",
        "ve-tos acl",
        CommandLayer::LowLevel,
        "bucket_config",
        "Bucket ACL management",
        true,
    ),
    group(
        "rename",
        "ve-tos rename",
        CommandLayer::LowLevel,
        "bucket_config",
        "Bucket RenameObject configuration",
        true,
    ),
    group(
        "real-time-log",
        "ve-tos real-time-log",
        CommandLayer::LowLevel,
        "bucket_config",
        "Real-time log analysis",
        true,
    ),
    group(
        "access-monitor",
        "ve-tos access-monitor",
        CommandLayer::LowLevel,
        "bucket_config",
        "Access monitoring configuration",
        true,
    ),
    group(
        "worm",
        "ve-tos worm",
        CommandLayer::LowLevel,
        "bucket_config",
        "WORM / object lock configuration",
        true,
    ),
    group(
        "trash",
        "ve-tos trash",
        CommandLayer::LowLevel,
        "bucket_config",
        "Bucket trash configuration",
        true,
    ),
    group(
        "payment",
        "ve-tos payment",
        CommandLayer::LowLevel,
        "bucket_config",
        "Requester pays configuration",
        true,
    ),
    group(
        "logging",
        "ve-tos logging",
        CommandLayer::LowLevel,
        "bucket_config",
        "Access log storage configuration",
        true,
    ),
    group(
        "intelligent-tiering",
        "ve-tos intelligent-tiering",
        CommandLayer::LowLevel,
        "bucket_config",
        "Intelligent tiering configuration",
        true,
    ),
    group(
        "transfer-acceleration",
        "ve-tos transfer-acceleration",
        CommandLayer::LowLevel,
        "bucket_config",
        "Transfer acceleration configuration",
        true,
    ),
    group(
        "cdn-notification",
        "ve-tos cdn-notification",
        CommandLayer::LowLevel,
        "bucket_config",
        "CDN notification configuration",
        true,
    ),
    group(
        "https-config",
        "ve-tos https-config",
        CommandLayer::LowLevel,
        "bucket_config",
        "HTTPS/TLS configuration",
        true,
    ),
    group(
        "pay-by-traffic",
        "ve-tos pay-by-traffic",
        CommandLayer::LowLevel,
        "bucket_config",
        "Pay-by-traffic configuration",
        true,
    ),
    group(
        "max-age",
        "ve-tos max-age",
        CommandLayer::LowLevel,
        "bucket_config",
        "Max-age cache configuration",
        true,
    ),
    group(
        "redundancy-transition",
        "ve-tos redundancy-transition",
        CommandLayer::LowLevel,
        "bucket_config",
        "Data redundancy transition",
        true,
    ),
    group(
        "data-process",
        "ve-tos data-process",
        CommandLayer::LowLevel,
        "advanced",
        "Advanced data processing APIs",
        true,
    ),
    group(
        "object-set",
        "ve-tos object-set",
        CommandLayer::LowLevel,
        "advanced",
        "Advanced object set APIs",
        true,
    ),
    group(
        "accelerator",
        "ve-tos accelerator",
        CommandLayer::LowLevel,
        "advanced",
        "Advanced accelerator control APIs",
        true,
    ),
    group(
        "mrap",
        "ve-tos mrap",
        CommandLayer::LowLevel,
        "advanced",
        "Multi-region access point APIs",
        true,
    ),
    group(
        "ap",
        "ve-tos ap",
        CommandLayer::LowLevel,
        "advanced",
        "Access point APIs",
        true,
    ),
    group(
        "cap",
        "ve-tos cap",
        CommandLayer::LowLevel,
        "advanced",
        "Converged access point APIs",
        true,
    ),
    group(
        "dataset",
        "ve-tos dataset",
        CommandLayer::LowLevel,
        "advanced",
        "Intelligent retrieval dataset APIs",
        true,
    ),
    group(
        "control",
        "ve-tos control",
        CommandLayer::LowLevel,
        "advanced",
        "Advanced control APIs",
        true,
    ),
    group(
        "capabilities",
        "ve-tos capabilities",
        CommandLayer::Meta,
        "utilities",
        "Discover CLI capabilities",
        true,
    ),
    group(
        "api",
        "ve-tos api",
        CommandLayer::Meta,
        "utilities",
        "Inspect API metadata",
        true,
    ),
    group(
        "config",
        "ve-tos config",
        CommandLayer::Meta,
        "utilities",
        "Configuration management",
        true,
    ),
    group(
        "completion",
        "ve-tos completion",
        CommandLayer::Meta,
        "utilities",
        "Generate shell completion",
        true,
    ),
    group(
        "serve",
        "ve-tos serve",
        CommandLayer::Meta,
        "utilities",
        "Start MCP server",
        true,
    ),
    group(
        "skill",
        "ve-tos skill",
        CommandLayer::Meta,
        "utilities",
        "Manage/export skill metadata",
        true,
    ),
    group(
        "doctor",
        "ve-tos doctor",
        CommandLayer::Meta,
        "utilities",
        "Environment diagnostics",
        true,
    ),
];

pub const CAPABILITIES: &[CapabilityEntry] = &[
    CapabilityEntry {
        command: "ve-tos cp",
        group: "cp",
        layer: CommandLayer::HighLevel,
        description: "Copy local files, objects, or prefixes between local and TOS.",
        risk_level: RiskLevel::Medium,
        apis: &["HeadObject/ListObjects", "GetObject", "PutObject", "CopyObject"],
        endpoint_kind: Some("DataPlane"),
        method: None,
        supports_describe: true,
        supports_dry_run: true,
        supports_force: true,
        supports_pipe: false,
        supports_output_formats: SUPPORTED_OUTPUT_FORMATS,
        parameters: TRANSFER_PARAMETERS,
        body_contract: None,
        // [Review Fix #s3 / #X1] Guard wording is now aligned with the post-M1/M3
        // implementation: streaming via tokio::io::copy, atomic .tos-partial-<pid>
        // persistence, dual SHA256+CRC64 prehashing for upload, and native x-tos-*
        // copy headers. Source ETag (x-tos-copy-source-if-match) and destination
        // ETag (if-match) are kept as separate guards because they target different
        // sides of CopyObject.
        consistency_guards: &[
            "download: GetObject streams via tokio::io::copy into .tos-partial-<pid> then renames atomically; honors If-Match/version_id",
            "upload: file body is stream-hashed (SHA256 + CRC64) once, sent via Body::wrap_stream, and verified against the response x-tos-hash-crc64ecma",
            "copy: CopyObject sends native x-tos-copy-source-if-match for the source ETag; if-match guards the destination ETag",
        ],
        examples: &["ve-tos cp ./file.txt tos://bucket/file.txt", "ve-tos cp tos://bucket/prefix ./dir --recursive"],
        related_commands: &["ve-tos object upload", "ve-tos object download", "ve-tos object copy"],
    },
    CapabilityEntry {
        command: "ve-tos mv",
        group: "mv",
        layer: CommandLayer::HighLevel,
        description: "Move files or objects by copy plus source delete.",
        risk_level: RiskLevel::Critical,
        apis: &["HeadObject/ListObjects", "GetObject", "PutObject", "DeleteObject"],
        endpoint_kind: Some("DataPlane"),
        method: None,
        supports_describe: true,
        supports_dry_run: true,
        supports_force: true,
        supports_pipe: false,
        supports_output_formats: SUPPORTED_OUTPUT_FORMATS,
        parameters: TRANSFER_PARAMETERS,
        body_contract: None,
        consistency_guards: &[
            "copy phase reuses cp guards: streaming + atomic persist + native x-tos-copy-source-if-match for source ETag (separate from destination if-match)",
            "source delete is issued only after the destination write succeeds",
            "destination is identified by the Cp-discovered ETag; delete propagates that ETag when available",
        ],
        examples: &["ve-tos mv tos://bucket/a tos://bucket/b --force --confirm tos://bucket/a"],
        related_commands: &["ve-tos cp", "ve-tos rm"],
    },
    CapabilityEntry {
        command: "ve-tos sync",
        group: "sync",
        layer: CommandLayer::HighLevel,
        description: "Synchronize source and destination incrementally.",
        risk_level: RiskLevel::Critical,
        apis: &["ListObjects", "HeadObject", "GetObject", "PutObject", "DeleteObject"],
        endpoint_kind: Some("DataPlane"),
        method: None,
        supports_describe: true,
        supports_dry_run: true,
        supports_force: true,
        supports_pipe: false,
        supports_output_formats: SUPPORTED_OUTPUT_FORMATS,
        parameters: SYNC_PARAMETERS,
        body_contract: None,
        consistency_guards: &[
            "manifest diff partitions entries into skip/copy/delete based on size + ETag (and mtime when available)",
            "transfer phase reuses cp guards: streaming I/O, dual SHA256+CRC64 prehash, and atomic .tos-partial-<pid> persistence",
            "delete phase requires --force and propagates the discovered ETag to make deletes target the snapshotted version",
            "dry-run computes scanned_count/preview_truncated; impact preview is capped to MAX_PREVIEW_OBJECTS",
        ],
        examples: &[
            "ve-tos sync ./dir tos://bucket/prefix",
            "ve-tos sync tos://bucket/src tos://bucket/dst --delete --force --confirm tos://bucket/dst",
        ],
        related_commands: &["ve-tos cp", "ve-tos rm"],
    },
    CapabilityEntry {
        command: "ve-tos mb",
        group: "mb",
        layer: CommandLayer::HighLevel,
        description: "Create a bucket with optional storage class, ACL, and redundancy settings.",
        risk_level: RiskLevel::Low,
        apis: &["CreateBucket", "PutBucketAcl"],
        endpoint_kind: Some("DataPlane"),
        method: Some("PUT"),
        supports_describe: true,
        supports_dry_run: true,
        supports_force: false,
        supports_pipe: false,
        supports_output_formats: SUPPORTED_OUTPUT_FORMATS,
        parameters: MB_PARAMETERS,
        body_contract: None,
        consistency_guards: &["create bucket is idempotency-aware at bucket name granularity"],
        examples: &["ve-tos mb tos://bucket --storage-class STANDARD"],
        related_commands: &["ve-tos bucket create", "ve-tos acl put"],
    },
    CapabilityEntry {
        command: "ve-tos rb",
        group: "rb",
        layer: CommandLayer::HighLevel,
        description: "Remove an empty bucket; use ve-tos rm --recursive first when cleanup is required.",
        risk_level: RiskLevel::Critical,
        apis: &["DeleteBucket"],
        endpoint_kind: Some("DataPlane"),
        method: Some("DELETE"),
        supports_describe: true,
        supports_dry_run: true,
        supports_force: true,
        supports_pipe: false,
        supports_output_formats: SUPPORTED_OUTPUT_FORMATS,
        parameters: RB_PARAMETERS,
        body_contract: None,
        consistency_guards: &["empty bucket phase records success and failure per object"],
        examples: &["ve-tos rb tos://bucket --force --confirm tos://bucket"],
        related_commands: &["ve-tos bucket delete", "ve-tos rm"],
    },
    CapabilityEntry {
        command: "ve-tos mkdir",
        group: "mkdir",
        layer: CommandLayer::HighLevel,
        description: "Create a folder.",
        risk_level: RiskLevel::Medium,
        apis: &["PutObject"],
        endpoint_kind: Some("DataPlane"),
        method: Some("PUT"),
        supports_describe: true,
        supports_dry_run: true,
        supports_force: false,
        supports_pipe: false,
        supports_output_formats: SUPPORTED_OUTPUT_FORMATS,
        parameters: MKDIR_PARAMETERS,
        body_contract: Some("Creates a zero-byte object whose key is normalized to end with '/'."),
        consistency_guards: &["folder targets are normalized to a trailing slash before PutObject"],
        examples: &[
            "ve-tos mkdir tos://bucket/folder/",
            "ve-tos mkdir tos://bucket/folder/subfolder/ --parents",
            "ve-tos mkdir --bucket bucket --key folder/",
        ],
        related_commands: &["ve-tos object upload", "ve-tos ls", "ve-tos rm"],
    },
    CapabilityEntry {
        command: "ve-tos rm",
        group: "rm",
        layer: CommandLayer::HighLevel,
        description: "Delete an object or prefix.",
        risk_level: RiskLevel::Critical,
        apis: &["HeadBucket", "HeadObject/ListObjects", "DeleteObject"],
        endpoint_kind: Some("DataPlane"),
        method: Some("DELETE"),
        supports_describe: true,
        supports_dry_run: true,
        supports_force: true,
        supports_pipe: false,
        supports_output_formats: SUPPORTED_OUTPUT_FORMATS,
        parameters: OBJECT_BATCH_PARAMETERS,
        body_contract: None,
        consistency_guards: &[
            "delete operations are planned first and require --force",
            "HNS recursive deletes can run bottom-up or use service-side direct recursion",
        ],
        examples: &[
            "ve-tos rm tos://bucket/key --force --confirm tos://bucket/key",
            "ve-tos rm tos://bucket/prefix --recursive --force --confirm tos://bucket/prefix",
            "ve-tos rm tos://bucket/prefix --recursive --recursive-delete-mode direct --force --confirm tos://bucket/prefix",
        ],
        related_commands: &["ve-tos object delete"],
    },
    CapabilityEntry {
        command: "ve-tos ls",
        group: "ls",
        layer: CommandLayer::HighLevel,
        description: "List buckets or objects.",
        risk_level: RiskLevel::Low,
        apis: &["ListBuckets", "ListObjects"],
        endpoint_kind: Some("DataPlane"),
        method: Some("GET"),
        supports_describe: true,
        supports_dry_run: true,
        supports_force: false,
        supports_pipe: true,
        supports_output_formats: SUPPORTED_OUTPUT_FORMATS,
        parameters: LISTING_PARAMETERS,
        body_contract: None,
        consistency_guards: &["listing is read-only and emits deterministic pagination output"],
        examples: &["ve-tos ls", "ve-tos ls tos://bucket/prefix --max-keys 100"],
        related_commands: &["ve-tos bucket list", "ve-tos object list"],
    },
    CapabilityEntry {
        command: "ve-tos stat",
        group: "stat",
        layer: CommandLayer::HighLevel,
        description: "Show bucket or object metadata.",
        risk_level: RiskLevel::Low,
        apis: &["HeadBucket", "HeadObject"],
        endpoint_kind: Some("DataPlane"),
        method: Some("HEAD"),
        supports_describe: true,
        supports_dry_run: true,
        supports_force: false,
        supports_pipe: true,
        supports_output_formats: SUPPORTED_OUTPUT_FORMATS,
        parameters: STAT_PARAMETERS,
        body_contract: None,
        consistency_guards: &["metadata read can pin an object version when version_id is provided"],
        examples: &["ve-tos stat tos://bucket/key"],
        related_commands: &["ve-tos bucket head", "ve-tos object head"],
    },
    CapabilityEntry {
        command: "ve-tos du",
        group: "du",
        layer: CommandLayer::HighLevel,
        description: "Calculate object size statistics for a prefix.",
        risk_level: RiskLevel::Low,
        apis: &["ListObjects"],
        endpoint_kind: Some("DataPlane"),
        method: Some("GET"),
        supports_describe: true,
        supports_dry_run: true,
        supports_force: false,
        supports_pipe: true,
        supports_output_formats: SUPPORTED_OUTPUT_FORMATS,
        parameters: DU_PARAMETERS,
        body_contract: None,
        consistency_guards: &["read-only traversal records deterministic summary output"],
        examples: &["ve-tos du tos://bucket/prefix --human-readable --cost"],
        related_commands: &["ve-tos ls"],
    },
    CapabilityEntry {
        command: "ve-tos find",
        group: "find",
        layer: CommandLayer::HighLevel,
        description: "Find objects by name, size, mtime, or storage class.",
        risk_level: RiskLevel::Low,
        apis: &["ListObjects"],
        endpoint_kind: Some("DataPlane"),
        method: Some("GET"),
        supports_describe: true,
        supports_dry_run: true,
        supports_force: false,
        supports_pipe: true,
        supports_output_formats: SUPPORTED_OUTPUT_FORMATS,
        parameters: FIND_PARAMETERS,
        body_contract: None,
        consistency_guards: &["read-only traversal applies filters after deterministic listing"],
        examples: &["ve-tos find tos://bucket/prefix --name '*.log' --size +10m"],
        related_commands: &["ve-tos ls", "ve-tos du"],
    },
    CapabilityEntry {
        command: "ve-tos cat",
        group: "cat",
        layer: CommandLayer::HighLevel,
        description: "Stream object content to stdout.",
        risk_level: RiskLevel::Low,
        // [Review Fix #X2] cat is best-effort streaming and never issues a HEAD
        // round-trip (see consistency_guards below); apis must reflect that to
        // keep the output truthful.
        apis: &["GetObject"],
        endpoint_kind: Some("DataPlane"),
        method: Some("GET"),
        supports_describe: true,
        supports_dry_run: true,
        supports_force: false,
        supports_pipe: true,
        supports_output_formats: SUPPORTED_OUTPUT_FORMATS,
        parameters: CAT_PARAMETERS,
        body_contract: None,
        consistency_guards: &[
            // [Review Fix #M2] cat is best-effort streaming; honors range and
            // version_id only. It does NOT issue If-Match (which would require
            // a HEAD round-trip and slow down piping), so the guard text must
            // accurately reflect this behavior.
            "streams raw object body to stdout; honors --range and --version-id without extra HEAD",
        ],
        examples: &["ve-tos cat tos://bucket/key --range bytes=0-1023"],
        related_commands: &["ve-tos object download"],
    },
    CapabilityEntry {
        command: "ve-tos put",
        group: "put",
        layer: CommandLayer::HighLevel,
        description: "Upload stdin to an object; upload starts/completes after stdin EOF.",
        risk_level: RiskLevel::Medium,
        apis: &[
            "PutObject",
            "CreateMultipartUpload",
            "UploadPart",
            "CompleteMultipartUpload",
        ],
        endpoint_kind: Some("DataPlane"),
        method: Some("PUT"),
        supports_describe: true,
        supports_dry_run: true,
        supports_force: false,
        supports_pipe: true,
        supports_output_formats: SUPPORTED_OUTPUT_FORMATS,
        parameters: PUT_PARAMETERS,
        body_contract: Some("Reads stdin until EOF; interactive terminals submit with Ctrl+D on Unix/macOS or Ctrl+Z then Enter on Windows. Ctrl+C cancels instead of uploading. Input below --multipart-threshold uses PutObject, larger input is uploaded with multipart upload parts."),
        consistency_guards: &[
            "interactive stdin is submitted with EOF, not Ctrl+C",
            "stdin is read in bounded chunks and never fully buffered for multipart-sized input",
            "each uploaded part carries CRC64 and the completed object CRC64 is checked when returned",
        ],
        examples: &["ve-tos cat tos://src/key | gzip | ve-tos put tos://dst/key.gz"],
        related_commands: &["ve-tos cat", "ve-tos object upload", "ve-tos multipart upload"],
    },
    CapabilityEntry {
        command: "ve-tos presign",
        group: "presign",
        layer: CommandLayer::HighLevel,
        description: "Generate a presigned URL for object access.",
        risk_level: RiskLevel::Medium,
        apis: &["SignV4"],
        endpoint_kind: Some("DataPlane"),
        method: None,
        supports_describe: true,
        supports_dry_run: true,
        supports_force: false,
        supports_pipe: true,
        supports_output_formats: SUPPORTED_OUTPUT_FORMATS,
        parameters: PRESIGN_PARAMETERS,
        body_contract: None,
        consistency_guards: &["presigned URL scope is constrained by method and expiration"],
        examples: &["ve-tos presign tos://bucket/key --expires 3600 --method GET"],
        related_commands: &["ve-tos object head"],
    },
    CapabilityEntry {
        command: "ve-tos restore",
        group: "restore",
        layer: CommandLayer::HighLevel,
        description: "Restore archived objects, including recursive and manifest-driven batches.",
        risk_level: RiskLevel::High,
        apis: &["ListObjects", "RestoreObject"],
        endpoint_kind: Some("DataPlane"),
        method: Some("POST"),
        supports_describe: true,
        supports_dry_run: true,
        supports_force: true,
        supports_pipe: false,
        supports_output_formats: SUPPORTED_OUTPUT_FORMATS,
        parameters: RESTORE_PARAMETERS,
        body_contract: Some("Restore job options are generated from flags; batch source may come from --manifest."),
        consistency_guards: &[
            "restore requests record per-object success and failure for retry",
            "batch dry-run reports scanned_count and preview_truncated when listing exceeds MAX_PREVIEW_OBJECTS",
            "destructive form (--force) is required only for --recursive/manifest batches; single-object restore is non-destructive",
        ],
        examples: &["ve-tos restore tos://bucket/key", "ve-tos restore tos://bucket/prefix --recursive --force"],
        related_commands: &["ve-tos object restore"],
    },
    CapabilityEntry {
        command: "ve-tos api",
        group: "api",
        layer: CommandLayer::Meta,
        description: "Execute or plan arbitrary signed TOS API requests through a JSON request contract.",
        risk_level: RiskLevel::High,
        apis: &["RawSignedRequest"],
        endpoint_kind: None,
        method: None,
        supports_describe: true,
        supports_dry_run: true,
        supports_force: true,
        supports_pipe: false,
        supports_output_formats: SUPPORTED_OUTPUT_FORMATS,
        parameters: API_PARAMETERS,
        body_contract: Some(
            "--request JSON fields: method, endpoint_rule (alias endpoint_kind), bucket, key, path, query, headers, body",
        ),
        consistency_guards: &[
            "mutating raw methods require --force",
            "signer-managed headers cannot be overridden",
            "raw inline response bodies are capped to prevent OOM",
        ],
        examples: &[
            "ve-tos api bucket lifecycle --request '{\"method\":\"GET\",\"endpoint_rule\":\"bucket\",\"bucket\":\"demo\",\"query\":{\"lifecycle\":\"\"}}' --dry-run",
            "ve-tos api object put --request '{\"method\":\"PUT\",\"endpoint_rule\":\"object\",\"bucket\":\"demo\",\"key\":\"hello.json\",\"headers\":{\"content-type\":\"application/json\"},\"body\":{\"hello\":\"world\"}}' --dry-run",
            "ve-tos api raw put --request file://request.json --force --output json",
        ],
        related_commands: &["ve-tos capabilities --view tree", "ve-tos doctor"],
    },
    CapabilityEntry {
        command: "ve-tos config init",
        group: "config",
        layer: CommandLayer::Meta,
        description: "Initialize TOS CLI configuration with a template layered profile.",
        risk_level: RiskLevel::Low,
        apis: &[],
        endpoint_kind: None,
        method: None,
        supports_describe: true,
        supports_dry_run: true,
        supports_force: false,
        supports_pipe: false,
        supports_output_formats: SUPPORTED_OUTPUT_FORMATS,
        parameters: CONFIG_INIT_PARAMETERS,
        body_contract: None,
        consistency_guards: &["writes only the selected local config profile"],
        examples: &["ve-tos config init", "ve-tos config init --profile staging"],
        related_commands: &["ve-tos config show", "ve-tos doctor"],
    },
    CapabilityEntry {
        command: "ve-tos config show",
        group: "config",
        layer: CommandLayer::Meta,
        description: "Show effective configuration with source annotations and redacted secrets.",
        risk_level: RiskLevel::Low,
        apis: &[],
        endpoint_kind: None,
        method: None,
        supports_describe: true,
        supports_dry_run: false,
        supports_force: false,
        supports_pipe: true,
        supports_output_formats: SUPPORTED_OUTPUT_FORMATS,
        parameters: SKILL_LIST_PARAMETERS,
        body_contract: None,
        consistency_guards: &["secrets are redacted before output"],
        examples: &["ve-tos config show --output json"],
        related_commands: &["ve-tos config init", "ve-tos doctor"],
    },
    CapabilityEntry {
        command: "ve-tos config set",
        group: "config",
        layer: CommandLayer::Meta,
        description: "Set a configuration value in the shared profile or TOS override profile.",
        risk_level: RiskLevel::Low,
        apis: &[],
        endpoint_kind: None,
        method: None,
        supports_describe: true,
        supports_dry_run: true,
        supports_force: false,
        supports_pipe: false,
        supports_output_formats: SUPPORTED_OUTPUT_FORMATS,
        parameters: CONFIG_SET_PARAMETERS,
        body_contract: None,
        consistency_guards: &["credential values are stored encrypted and redacted on show"],
        examples: &[
            "ve-tos config set region cn-beijing",
            "ve-tos config set endpoint https://tos-cn-boe.volces.com --profile dev",
        ],
        related_commands: &["ve-tos config show", "ve-tos doctor"],
    },
    CapabilityEntry {
        command: "ve-tos completion",
        group: "completion",
        layer: CommandLayer::Meta,
        description: "Generate shell completion scripts and installation snippets for ve-tos-cli / ve-tos.",
        risk_level: RiskLevel::Low,
        apis: &[],
        endpoint_kind: None,
        method: None,
        supports_describe: true,
        supports_dry_run: false,
        supports_force: false,
        supports_pipe: true,
        supports_output_formats: SUPPORTED_OUTPUT_FORMATS,
        parameters: COMPLETION_PARAMETERS,
        body_contract: None,
        consistency_guards: &[
            "completion scripts are generated from the registry-backed command tree",
            "the command returns a structured Envelope; use --output json and extract data.script for installation",
        ],
        examples: &[
            "ve-tos completion bash --output json",
            "ve-tos completion bash --output json | jq -r '.data.script' > ~/.ve-tos-completion.bash",
            "ve-tos completion zsh --output json | jq -r '.data.script' > ~/.zfunc/_ve-tos",
            "ve-tos completion fish --output json | jq -r '.data.script' > ~/.config/fish/completions/ve-tos.fish",
            "ve-tos completion powershell --output json | jq -r '.data.script' >> $PROFILE",
        ],
        related_commands: &["ve-tos capabilities --view full", "ve-tos doctor --check completion"],
    },
    CapabilityEntry {
        command: "ve-tos serve",
        group: "serve",
        layer: CommandLayer::Meta,
        description: "Start an MCP server backed by the same in-process skill registry used by skill list/export.",
        risk_level: RiskLevel::Low,
        apis: &[],
        endpoint_kind: None,
        method: None,
        supports_describe: true,
        supports_dry_run: true,
        supports_force: false,
        supports_pipe: false,
        supports_output_formats: SUPPORTED_OUTPUT_FORMATS,
        parameters: SERVE_PARAMETERS,
        body_contract: None,
        consistency_guards: &[
            "stdio is the default MCP transport and does not open a TCP listener",
            "sse binds the rmcp HTTP/SSE transport to 127.0.0.1:<port>",
            "--dry-run and --describe report planned_not_started without launching the long-running server",
        ],
        examples: &[
            "ve-tos serve --mcp",
            "ve-tos serve --mcp --transport sse --port 9090",
            "ve-tos serve --mcp --dry-run --output json",
        ],
        related_commands: &["ve-tos skill list", "ve-tos capabilities --view full"],
    },
    CapabilityEntry {
        command: "ve-tos skill list",
        group: "skill",
        layer: CommandLayer::Meta,
        description: "List TOS skill v1 metadata used for MCP tool advertisement and external Agent catalogs.",
        risk_level: RiskLevel::Low,
        apis: &[],
        endpoint_kind: None,
        method: None,
        supports_describe: true,
        supports_dry_run: false,
        supports_force: false,
        supports_pipe: true,
        supports_output_formats: SUPPORTED_OUTPUT_FORMATS,
        parameters: &[],
        body_contract: None,
        consistency_guards: &[
            "metadata is derived from the curated capability registry plus the clap command tree",
            "serve uses the same in-process definitions; it does not read exported Markdown skill files",
        ],
        examples: &[
            "ve-tos skill list --output json",
            "ve-tos skill list --language zh --output json",
        ],
        related_commands: &["ve-tos skill export", "ve-tos serve --mcp"],
    },
    CapabilityEntry {
        command: "ve-tos skill export",
        group: "skill",
        layer: CommandLayer::Meta,
        description: "Export TOS Markdown SKILL.md files for external agents, documentation, prompts, or adapter tooling.",
        risk_level: RiskLevel::Low,
        apis: &[],
        endpoint_kind: None,
        method: None,
        supports_describe: true,
        supports_dry_run: true,
        supports_force: false,
        supports_pipe: false,
        supports_output_formats: SUPPORTED_OUTPUT_FORMATS,
        parameters: SKILL_EXPORT_PARAMETERS,
        body_contract: None,
        consistency_guards: &[
            "export writes dir/SKILL.md plus dir/{domain}/{skill_name}/SKILL.md and refuses to overwrite existing files",
            "--dry-run returns planned target paths and conflict flags without creating directories or files",
            "exported Markdown skill files are portable catalogs; serve rebuilds MCP tools from the live registry instead of reading the export directory",
        ],
        examples: &[
            "ve-tos skill export --dry-run --output json",
            "ve-tos skill export --name cp --dir ./ve-tos-skills",
            "ve-tos skill export --name \"ve-tos bucket create\" --dir ./ve-tos-skills",
            "ve-tos skill export --language zh --dir ./ve-tos-skills-zh",
        ],
        related_commands: &["ve-tos skill list", "ve-tos serve --mcp"],
    },
    // [Review Fix #s1] Curated leaf rows for the action-level commands that high-level
    // flows compose with. Keeps risk/guard/example metadata authoritative instead of
    // falling back to clap-tree derivation.
    CapabilityEntry {
        command: "ve-tos object upload",
        group: "object",
        layer: CommandLayer::LowLevel,
        description: "PutObject for a single object body up to 5GB.",
        risk_level: RiskLevel::Medium,
        apis: &["PutObject"],
        endpoint_kind: Some("DataPlane"),
        method: Some("PUT"),
        supports_describe: true,
        supports_dry_run: true,
        supports_force: false,
        supports_pipe: true,
        supports_output_formats: SUPPORTED_OUTPUT_FORMATS,
        parameters: OBJECT_UPLOAD_PARAMETERS,
        body_contract: Some("--body accepts file paths, file://, '-' (stdin), or inline strings; file inputs are stream-uploaded."),
        consistency_guards: &[
            "file --body is stream-hashed (SHA256 + CRC64) once before signing and sent via Body::wrap_stream",
            "stdin/inline payloads stay buffered for V4 signing and capped at the safe inline limit",
            "response x-tos-hash-crc64ecma is verified when the client computed CRC64",
        ],
        examples: &[
            "ve-tos object upload tos://bucket/key --body ./payload.bin",
            "echo hello | ve-tos object upload tos://bucket/key --body -",
        ],
        related_commands: &["ve-tos cp", "ve-tos multipart upload"],
    },
    CapabilityEntry {
        command: "ve-tos object download",
        group: "object",
        layer: CommandLayer::LowLevel,
        description: "GetObject streamed to a destination file or stdout.",
        risk_level: RiskLevel::Low,
        apis: &["GetObject"],
        endpoint_kind: Some("DataPlane"),
        method: Some("GET"),
        supports_describe: true,
        supports_dry_run: true,
        supports_force: false,
        supports_pipe: true,
        supports_output_formats: SUPPORTED_OUTPUT_FORMATS,
        parameters: OBJECT_DOWNLOAD_PARAMETERS,
        body_contract: None,
        consistency_guards: &[
            "response body is streamed via tokio::io::copy into .tos-partial-<pid> then renamed atomically",
            "binary stdout is base64-encoded inside the Envelope; --output - streams raw bytes",
            "honors --range, --version-id, and --if-match without buffering the full object",
        ],
        examples: &[
            "ve-tos object download tos://bucket/key --output ./local.bin",
            "ve-tos object download tos://bucket/key --range bytes=0-1023 --output -",
        ],
        related_commands: &["ve-tos cp", "ve-tos cat", "ve-tos multipart download"],
    },
    CapabilityEntry {
        command: "ve-tos object copy",
        group: "object",
        layer: CommandLayer::LowLevel,
        description: "Server-side CopyObject between TOS keys (same or cross bucket).",
        risk_level: RiskLevel::Medium,
        apis: &["CopyObject"],
        endpoint_kind: Some("DataPlane"),
        method: Some("PUT"),
        supports_describe: true,
        supports_dry_run: true,
        supports_force: false,
        supports_pipe: false,
        supports_output_formats: SUPPORTED_OUTPUT_FORMATS,
        parameters: OBJECT_COPY_PARAMETERS,
        body_contract: None,
        consistency_guards: &[
            "x-tos-copy-source carries the URL-encoded source path and version_id",
            "--if-match maps to the destination ETag guard (if-match header); the source ETag guard (x-tos-copy-source-if-match) is only injected by high-level cp/mv when the discovered source ETag is known",
            "metadata-directive defaults to COPY; REPLACE requires the full metadata set; REPLACE_NEW overrides only newly supplied metadata fields",
        ],
        examples: &["ve-tos object copy tos://src/key tos://dst/key --if-match \"abc\""],
        related_commands: &["ve-tos cp", "ve-tos mv"],
    },
    CapabilityEntry {
        command: "ve-tos object delete",
        group: "object",
        layer: CommandLayer::LowLevel,
        description: "DeleteObject for a single key (optionally a specific version).",
        risk_level: RiskLevel::Critical,
        apis: &["DeleteObject"],
        endpoint_kind: Some("DataPlane"),
        method: Some("DELETE"),
        supports_describe: true,
        supports_dry_run: true,
        supports_force: true,
        supports_pipe: false,
        supports_output_formats: SUPPORTED_OUTPUT_FORMATS,
        parameters: OBJECT_DELETE_PARAMETERS,
        body_contract: None,
        consistency_guards: &[
            "destructive call gated by --force unless --dry-run is in effect",
            "version_id, when supplied, scopes the delete to a single version",
        ],
        examples: &["ve-tos object delete tos://bucket/key --force --confirm tos://bucket/key"],
        related_commands: &["ve-tos rm", "ve-tos object batch-delete"],
    },
    CapabilityEntry {
        command: "ve-tos bucket create",
        group: "bucket",
        layer: CommandLayer::LowLevel,
        description: "CreateBucket with optional region, storage class, and ACL settings.",
        risk_level: RiskLevel::Low,
        apis: &["CreateBucket"],
        endpoint_kind: Some("DataPlane"),
        method: Some("PUT"),
        supports_describe: true,
        supports_dry_run: true,
        supports_force: false,
        supports_pipe: false,
        supports_output_formats: SUPPORTED_OUTPUT_FORMATS,
        parameters: BUCKET_CREATE_PARAMETERS,
        body_contract: None,
        consistency_guards: &["create is idempotent at bucket-name granularity; existing-bucket errors are surfaced verbatim"],
        examples: &["ve-tos bucket create tos://bucket --storage-class STANDARD"],
        related_commands: &["ve-tos mb"],
    },
    CapabilityEntry {
        command: "ve-tos bucket delete",
        group: "bucket",
        layer: CommandLayer::LowLevel,
        description: "DeleteBucket; bucket must be empty before this succeeds.",
        risk_level: RiskLevel::Critical,
        apis: &["DeleteBucket"],
        endpoint_kind: Some("DataPlane"),
        method: Some("DELETE"),
        supports_describe: true,
        supports_dry_run: true,
        supports_force: true,
        supports_pipe: false,
        supports_output_formats: SUPPORTED_OUTPUT_FORMATS,
        parameters: BUCKET_DELETE_PARAMETERS,
        body_contract: None,
        consistency_guards: &[
            "destructive call gated by --force unless --dry-run is in effect",
            "non-empty buckets must be drained with ve-tos rm --recursive before bucket deletion",
        ],
        examples: &["ve-tos bucket delete tos://bucket --force --confirm tos://bucket"],
        related_commands: &["ve-tos rb"],
    },
];

pub fn command_groups() -> &'static [CommandGroupEntry] {
    COMMAND_GROUPS
}

pub fn capabilities() -> &'static [CapabilityEntry] {
    CAPABILITIES
}

pub fn find_group(name: &str) -> Option<&'static CommandGroupEntry> {
    let name = canonical_group_name(name);
    COMMAND_GROUPS
        .iter()
        .find(|entry| entry.name == name || entry.command == name)
}

pub fn find_capability(command: &str) -> Option<&'static CapabilityEntry> {
    let command = normalize_command(command);
    let group = canonical_group_name(command.as_str());
    CAPABILITIES
        .iter()
        .find(|entry| entry.command == command || entry.group == group)
}

pub fn canonical_group_name(name: &str) -> &str {
    match name {
        // [Review Fix #M5] Canonical spelling is now `storageclass`. Map the
        // historical typo `storgeclass` so legacy invocations still resolve to
        // the correct registry entry.
        "storgeclass" => "storageclass",
        "ve-tos storgeclass" => "ve-tos storageclass",
        other => other,
    }
}

pub fn is_known_group_or_category(name: &str) -> bool {
    let name = canonical_group_name(name);
    COMMAND_GROUPS
        .iter()
        .any(|entry| entry.name == name || entry.category == name || entry.command == name)
}

/// Map a fully-qualified command (e.g. `ve-tos cors put`) to its business skill
/// domain (e.g. `tos-bucket-config`).
///
/// Domains group related commands so Agents can reason about the surface at a
/// coarser, intent-oriented granularity than the per-command root:
///   - `tos-transfer`      high-level data movement (cp/mv/sync/ls/...)
///   - `tos-bucket`        bucket/object/multipart core read-write APIs
///   - `tos-bucket-config` bucket-scoped configuration (cors/lifecycle/...)
///   - `tos-control`       control-plane & advanced APIs (control/mrap/...)
///   - `tos-shared`        cross-cutting utilities (config/api/serve/...)
///   - `tos-admin`         operational helpers (doctor/capabilities)
///
/// Derivation goes command-root → category → domain so newly added commands are
/// classified automatically as long as their `COMMAND_GROUPS` category is set.
pub fn business_domain(command: &str) -> &'static str {
    let root = command.split_whitespace().nth(1).unwrap_or(command);
    let root = canonical_group_name(root);
    let category = find_group(root).map(|entry| entry.category).unwrap_or("");
    match category {
        "high_level" => "tos-transfer",
        "core" => "tos-bucket",
        "bucket_config" => "tos-bucket-config",
        "advanced" | "control" => "tos-control",
        "utilities" => match root {
            // doctor/capabilities are operational; everything else is shared tooling.
            "doctor" | "capabilities" => "tos-admin",
            _ => "tos-shared",
        },
        // Unknown/unclassified commands default to the shared utility bucket so
        // the P6 coverage check never sees an empty domain.
        _ => "tos-shared",
    }
}

/// [Review Fix #21] Registry is the single source for capability rows,
/// including rows derived from the clap command tree. Callers may filter and
/// rank rows, but risk/endpoint/method/body metadata must not be re-inferred
/// in presentation layers.
pub fn capability_rows(
    caps: &[&'static CapabilityEntry],
    commands: &[CommandTreeEntry],
    keep_parameters: bool,
) -> Vec<RegistryCapabilityRow> {
    let mut rows: Vec<RegistryCapabilityRow> = caps
        .iter()
        .map(|entry| curated_capability_row(entry, keep_parameters))
        .collect();
    let curated_commands = caps
        .iter()
        .map(|entry| entry.command)
        .collect::<std::collections::BTreeSet<_>>();
    rows.extend(
        commands
            .iter()
            .filter(|entry| entry.subcommands.is_empty())
            .filter(|entry| !curated_commands.contains(entry.command.as_str()))
            .filter_map(|entry| command_tree_capability_row(entry, keep_parameters)),
    );
    rows
}

pub fn capability_row_for_command(
    command: &str,
    keep_parameters: bool,
) -> Option<RegistryCapabilityRow> {
    if let Some(entry) = find_capability(command) {
        return Some(curated_capability_row(entry, keep_parameters));
    }
    find_command_tree_entry(command).and_then(|entry| {
        if entry.subcommands.is_empty() {
            command_tree_capability_row(&entry, keep_parameters)
        } else {
            None
        }
    })
}

/// Build a parameter-independent `--describe` payload from registry metadata.
///
/// This is used by the top-level parser recovery path for commands such as
/// `ve-tos cp --describe`, where clap would normally reject the invocation before
/// the high-level handler can run because SOURCE/DESTINATION are absent.
pub fn describe_command_metadata(command: &str) -> Option<CommandDescription> {
    let row = capability_row_for_command(command, true)?;
    let apis = (!row.apis.is_empty()).then_some(row.apis.clone());
    // [Review Fix #RebaseRegistry] Rebase kept the helper but dropped this call;
    // route describe output through the full high-level metadata builder.
    let routing = describe_scenario_routing(&row);

    Some(
        CommandDescription {
            command: row.command.clone(),
            layer: layer_from_name(&row.layer),
            api: apis.as_ref().map(|items| items.join(" + ")),
            description: row.description.clone(),
            risk_level: risk_from_name(&row.risk_level),
            supports_dry_run: row.supports_dry_run,
            supports_pipe: row.supports_pipe,
            parameters: row.parameters.map(|params| {
                let has_body_contract = row.body_contract.is_some();
                params
                    .into_iter()
                    .map(|param| registry_describe_parameter(param, has_body_contract))
                    .collect()
            }),
            scenario_routing: Some(routing),
            related_commands: Some(RelatedCommands {
                high_level: None,
                low_level: (!row.related_commands.is_empty()).then_some(row.related_commands),
            }),
            low_level_apis: apis.clone(),
            wraps_apis: apis,
            output_filter_examples: Some(tos_filter_examples(&row.command)),
            shell_quoting_tips: Some(vec![
                "Quote paths and object keys that contain spaces or shell metacharacters."
                    .to_string(),
                "JMESPath literals inside --query use backticks; keep the whole expression quoted."
                    .to_string(),
            ]),
        }
        .mirror_apis(),
    )
}

fn registry_describe_parameter(
    param: RegistryCapabilityParameter,
    has_body_contract: bool,
) -> CommandParameter {
    let is_config_body = has_body_contract && param.name == "config";
    let schema_type = if is_config_body {
        "object"
    } else {
        registry_parameter_schema_type(&param.name)
    };
    CommandParameter {
        // [Review Fix #5] Registry fallback describe must use the same body
        // parameter convention as handler describe output.
        name: if is_config_body {
            "config(body)".to_string()
        } else {
            param.name
        },
        location: if is_config_body {
            ParameterLocation::Body
        } else {
            parameter_location_from_name(&param.location)
        },
        required: param.required,
        description: param.description,
        // [Review Fix #RebaseSchema] Preserve the describe schema contract that
        // agents and tests use for pre-invocation validation.
        schema: Some(json!({ "type": schema_type })),
        ..Default::default()
    }
}

fn registry_parameter_schema_type(name: &str) -> &'static str {
    match name {
        "recursive"
        | "checkpoint"
        | "force"
        | "destroy"
        | "progress"
        | "no-progress"
        | "list-echo"
        | "no-list-echo"
        | "no-manifest"
        | "report-failures-only"
        | "delete"
        | "size-only"
        | "exact-timestamps"
        | "include-parent"
        | "parents"
        | "all-versions"
        | "include-uploads"
        | "no-clobber"
        | "human-readable"
        | "cost"
        | "mcp"
        | "bucket-object-lock-enabled" => "boolean",
        "max-depth"
        | "max-keys"
        | "top-k"
        | "days"
        | "expires"
        | "port"
        | "batch-concurrency"
        | "list-concurrency"
        | "multipart-concurrency" => "integer",
        _ => "string",
    }
}

fn describe_scenario_routing(row: &RegistryCapabilityRow) -> HashMap<String, String> {
    let mut routing = HashMap::new();
    if let Some(endpoint_rule) = &row.endpoint_rule {
        routing.insert("endpoint_rule".to_string(), endpoint_rule.clone());
        routing.insert(
            "endpoint_kind".to_string(),
            match endpoint_rule.as_str() {
                "control" | "ControlPlane" | "Control Plane" => "ControlPlane",
                _ => "DataPlane",
            }
            .to_string(),
        );
    }
    if let Some(method) = &row.method {
        routing.insert("method".to_string(), method.clone());
    }
    if let Some(body_contract) = &row.body_contract {
        routing.insert("body_contract".to_string(), body_contract.clone());
    }
    routing.insert(
        "dry_run".to_string(),
        "returns a deterministic plan without mutating local files or TOS resources".to_string(),
    );
    routing.insert(
        "target_resolution".to_string(),
        "accept tos://bucket[/key] URI or command-specific --bucket/--key flags where supported"
            .to_string(),
    );
    routing.insert(
        "output".to_string(),
        "success and failure paths use Envelope plus --query and multi-format rendering"
            .to_string(),
    );
    routing.insert(
        "progress".to_string(),
        "execution stderr; auto-enabled on TTY, disabled by --no-progress or --quiet, forced by --progress".to_string(),
    );
    routing.insert(
        "list_echo".to_string(),
        "listing stderr; auto-enabled on TTY, disabled by --no-list-echo or --quiet, forced by --list-echo"
            .to_string(),
    );
    routing.insert(
        "checkpoint".to_string(),
        "stable task fingerprint plus atomic lock when the command supports checkpoint state"
            .to_string(),
    );
    routing.insert(
        "low_level_boundary".to_string(),
        "TOS High-Level commands wrap TOS OpenAPI actions directly".to_string(),
    );
    match row.command.as_str() {
        "ve-tos ls" => {
            routing.insert(
                "target_matrix".to_string(),
                "no target -> ListBuckets; bucket or prefix target -> ListObjects".to_string(),
            );
            routing.insert(
                "output_shapes".to_string(),
                "bucket listing mirrors ve-tos bucket list; JSON object listing uses raw data.objects/data.common_prefixes; table/csv render a synthesized typed row view"
                    .to_string(),
            );
        }
        "ve-tos bucket create" => {
            routing.insert(
                "bucket_target".to_string(),
                "accept either uri=tos://bucket or bucket_name=<bucket> (CLI --bucket <bucket>); provide exactly one"
                    .to_string(),
            );
        }
        "ve-tos completion" => {
            routing.insert(
                "install_flow".to_string(),
                "the command returns an Envelope; install by extracting data.script, then source bash output, add ~/.zfunc to zsh fpath and run compinit, write fish output under ~/.config/fish/completions, or append PowerShell output to $PROFILE"
                    .to_string(),
            );
            routing.insert(
                "registered_command_names".to_string(),
                "generated scripts register ve-tos-cli and ve-tos".to_string(),
            );
        }
        "ve-tos serve" => {
            routing.insert(
                "transport_matrix".to_string(),
                "stdio uses stdin/stdout and opens no TCP listener; sse starts a local rmcp HTTP/SSE listener on 127.0.0.1:<port>"
                    .to_string(),
            );
            routing.insert(
                "tool_source".to_string(),
                "MCP tools are rebuilt from the in-process skill registry; exported Markdown skill files are not read by serve"
                    .to_string(),
            );
            routing.insert(
                "call_semantics".to_string(),
                "tools/call plans by default; include execute=true to run the underlying CLI command"
                    .to_string(),
            );
        }
        "ve-tos skill list" | "ve-tos skill export" => {
            routing.insert(
                "format".to_string(),
                "Markdown SKILL.md pack with root index plus per-domain command skills".to_string(),
            );
            routing.insert(
                "consumers".to_string(),
                "external Agent catalogs, prompt context, documentation generators, adapters, and MCP tool advertisement"
                    .to_string(),
            );
            routing.insert(
                "serve_relationship".to_string(),
                "serve uses the same live registry data but does not read the exported Markdown skill directory"
                    .to_string(),
            );
        }
        "ve-tos cp" => {
            routing.insert(
                "transfer_matrix".to_string(),
                "local->TOS uses PutObject; TOS->local uses GetObject; TOS->TOS uses CopyObject; --recursive expands prefixes with ListObjects"
                    .to_string(),
            );
        }
        "ve-tos mv" => {
            routing.insert(
                "transfer_matrix".to_string(),
                "copy phase follows cp behavior; source delete uses DeleteObject after destination confirmation"
                    .to_string(),
            );
        }
        "ve-tos sync" => {
            routing.insert(
                "sync_matrix".to_string(),
                "lists both sides, transfers changed objects, and deletes destination extras only when --delete is set"
                    .to_string(),
            );
        }
        "ve-tos rm" => {
            routing.insert(
                "target_scope".to_string(),
                "rm accepts object or prefix targets only; use ve-tos rb for bucket deletion"
                    .to_string(),
            );
            routing.insert(
                "destructive_guard".to_string(),
                "critical delete paths require --force and, in non-interactive shells, exact --confirm <target>"
                    .to_string(),
            );
            routing.insert(
                "recursive_delete".to_string(),
                "recursive prefix deletes list objects first; HNS targets can use bottom-up or direct mode"
                    .to_string(),
            );
        }
        "ve-tos rb" => {
            routing.insert(
                "recursive_delete".to_string(),
                "bucket deletion only calls DeleteBucket; object cleanup is handled by ve-tos rm"
                    .to_string(),
            );
        }
        "ve-tos restore" => {
            routing.insert(
                "batch".to_string(),
                "single object by default; --recursive or --manifest expands multiple restore requests"
                    .to_string(),
            );
        }
        _ => {}
    }
    routing
}
pub fn find_api_capability(group: &str, action: &str) -> Option<&'static CapabilityEntry> {
    let group = canonical_group_name(group);
    let action_command = format!("ve-tos {group} {action}");
    if let Some(capability) = capabilities()
        .iter()
        .find(|entry| entry.command == action_command)
    {
        return Some(capability);
    }
    if matches!(action, "describe" | "metadata" | "info") {
        if let Some(capability) = capabilities().iter().find(|entry| entry.group == group) {
            return Some(capability);
        }
    }
    None
}

/// [P0 #1] Effective capability snapshot used by the dispatcher guard. We
/// project either a curated `CapabilityEntry` or a leaf-inferred row into a
/// single shape so destructive low-level actions cannot bypass the registry
/// just because they were never hand-listed in `CAPABILITIES`.
#[derive(Debug, Clone)]
pub struct EffectiveCapability {
    pub command: String,
    pub risk_level: RiskLevel,
    pub supports_force: bool,
    pub source: &'static str,
}

/// Resolve any `ve-tos <group> <action>` command path into an `EffectiveCapability`
/// regardless of whether it has a hand-curated `CapabilityEntry`. Falls back to
/// action-name heuristics that mirror the high-level risk classifier.
pub fn resolve_effective_capability(command: &str) -> Option<EffectiveCapability> {
    if let Some(entry) = find_capability(command) {
        return Some(EffectiveCapability {
            command: entry.command.to_string(),
            risk_level: entry.risk_level,
            supports_force: entry.supports_force,
            source: "registry",
        });
    }
    let normalized = normalize_command(command);
    let mut parts = normalized.split_whitespace();
    let _tool = parts.next()?;
    let group = parts.next()?;
    let action = parts.next().unwrap_or(group);
    let risk = infer_leaf_risk(group, action);
    let supports_force = matches!(risk, RiskLevel::High | RiskLevel::Critical);
    Some(EffectiveCapability {
        command: normalized,
        risk_level: risk,
        supports_force,
        source: "inferred",
    })
}

fn infer_leaf_risk(group: &str, action: &str) -> RiskLevel {
    let verb = action.split('-').next().unwrap_or(action);
    if group == "rb" && action == "rb" {
        return RiskLevel::Critical;
    }
    if matches!(verb, "destroy" | "purge") {
        return RiskLevel::Critical;
    }
    if action == "batch-delete" || matches!(verb, "delete" | "remove" | "rm" | "drop") {
        return RiskLevel::Critical;
    }
    if matches!(verb, "abort" | "disable") {
        return RiskLevel::High;
    }
    if matches!(group, "rb" | "rm") {
        return RiskLevel::Critical;
    }
    // [Review Fix #6] `mv` deletes the source after copying, so the leaf is
    // destructive even if its action name does not literally match `delete`.
    // `sync` can delete destination extras when --delete is used; the curated
    // high-level row marks it Critical while this leaf-only inference remains
    // High for non-delete generic shapes.
    if matches!(group, "mv" | "sync") {
        return RiskLevel::High;
    }
    if matches!(
        verb,
        "create"
            | "put"
            | "set"
            | "set-meta"
            | "set-acl"
            | "set-tagging"
            | "set-time"
            | "set-expires"
            | "set-retention"
            | "upload"
            | "form-upload"
            | "copy"
            | "modify"
            | "rename"
            | "append"
            | "seal-append"
            | "fetch"
            | "create-fetch-task"
            | "open"
            | "close"
            | "complete"
    ) {
        return RiskLevel::Medium;
    }
    if matches!(
        group,
        "cp" | "mv" | "sync" | "mb" | "restore" | "api" | "config"
    ) {
        return RiskLevel::Medium;
    }
    RiskLevel::Low
}

/// [Review Fix #6] Registry-driven guard：在 dispatcher 入口统一执行"破坏性命令必须显式 --force"
/// 的检查，避免每个 handler 各自重复实现。
///
/// - 当 `cmd_path` 在 registry 中标记为 `supports_force` 且 `risk_level >= High` 时，
///   真实执行路径若 `force == false` 即拒绝。
/// - 注册表里未声明 `supports_force` 的命令直接放行，由 handler 自己判断（如纯只读命令）。
/// - 不会改变现有 handler 中 `ensure_force_for_destructive` 的兜底行为，仅作为前置统一拦截。
///
/// [P0 #1] 当命令在 `CAPABILITIES` 中无手写条目时，回退到 leaf-推断风险表，
/// 保证 271 个 low-level action 不会绕过统一守门。
pub fn enforce_registry_guards(
    cmd_path: &str,
    force: bool,
    _is_tty: bool,
) -> Result<(), GuardViolation> {
    let Some(effective) = resolve_effective_capability(cmd_path) else {
        return Ok(());
    };
    if !effective.supports_force {
        return Ok(());
    }
    if !matches!(effective.risk_level, RiskLevel::High | RiskLevel::Critical) {
        return Ok(());
    }
    // [Review Fix #1] TTY presence is not an explicit confirmation for destructive execution.
    if force {
        return Ok(());
    }
    Err(GuardViolation {
        command: leak_command_label(effective.command),
        risk_level: effective.risk_level,
        reason: "destructive command requires --force before execution",
        fix_hint: "rerun with --force after reviewing impact",
    })
}

/// Leak an owned command label into a `'static` slot for the existing
/// `GuardViolation` shape. Bound by the small set of distinct command paths
/// the binary will ever execute, so memory growth is negligible.
fn leak_command_label(command: String) -> &'static str {
    Box::leak(command.into_boxed_str())
}

/// [Review Fix #9] Enumerate every command that the dispatcher would gate
/// behind `--force`. Useful for `ve-tos doctor` / Agent planners that need to
/// pre-screen risky commands before invocation. The list is derived by walking
/// the full leaf command tree and projecting each entry through
/// `resolve_effective_capability`, then filtering by guard semantics.
pub fn force_required_commands() -> Vec<EffectiveCapability> {
    let mut acc = Vec::new();
    for entry in leaf_command_tree() {
        if let Some(effective) = resolve_effective_capability(&entry.command) {
            if effective.supports_force
                && matches!(effective.risk_level, RiskLevel::High | RiskLevel::Critical)
            {
                acc.push(effective);
            }
        }
    }
    // Stable order: by command path so doctor output is deterministic across
    // runs and golden-file tests stay well-defined.
    acc.sort_by(|a, b| a.command.cmp(&b.command));
    acc.dedup_by(|a, b| a.command == b.command);
    acc
}

/// [Review Fix #6] Guard 拒绝时返回的结构化诊断；调用方负责映射到 `CliError::ValidationError`，
/// 保持错误码确定性。
#[derive(Debug, Clone)]
pub struct GuardViolation {
    pub command: &'static str,
    pub risk_level: RiskLevel,
    pub reason: &'static str,
    pub fix_hint: &'static str,
}

impl std::fmt::Display for GuardViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "command '{}' (risk={:?}) {}; {}",
            self.command, self.risk_level, self.reason, self.fix_hint
        )
    }
}

pub fn command_tree() -> Vec<CommandTreeEntry> {
    // [Review Fix #3] Derive action-level metadata from clap to avoid a second hand-written API list.
    let command = TosCommand::augment_subcommands(Command::new("ve-tos"));
    command
        .get_subcommands()
        .map(|subcommand| command_tree_entry(subcommand, "ve-tos"))
        .collect()
}

pub fn flattened_command_tree() -> Vec<CommandTreeEntry> {
    let mut entries = Vec::new();
    for entry in command_tree() {
        push_flattened_command(entry, &mut entries);
    }
    entries
}

pub fn leaf_command_tree() -> Vec<CommandTreeEntry> {
    flattened_command_tree()
        .into_iter()
        .filter(|entry| entry.subcommands.is_empty())
        .collect()
}

pub fn find_command_tree_entry(command: &str) -> Option<CommandTreeEntry> {
    let normalized = normalize_command(command);
    command_tree()
        .into_iter()
        .find_map(|entry| find_command_tree_entry_in(entry, &normalized))
}

pub fn describe_tos_group() -> Value {
    let groups = COMMAND_GROUPS
        .iter()
        .map(|entry| {
            json!({
                "name": entry.name,
                "command": entry.command,
                "layer": &entry.layer,
                "category": entry.category,
                "description": entry.description,
                "supports_help": entry.supports_help,
                "supports_describe": entry.supports_describe,
                "implemented": entry.implemented,
            })
        })
        .collect::<Vec<_>>();

    json!({
        "command": "ve-tos",
        "kind": "tool",
        "layer": "tool",
        "description": "TOS Object Storage CLI",
        "supports_help": true,
        "supports_describe": true,
        "groups": groups,
    })
}

pub fn grouped_help_text(global_options: &str) -> String {
    // [Review Fix #4] Render grouped help from registry so `ve-tos` help and describe output cannot drift.
    let command_prefix = tos_example_prefix();
    let mut output = String::from("TOS Object Storage CLI — Agent-Native\n\nUsage:\n");
    if command_prefix == "ve-tos-cli" {
        output.push_str(
            "  ve-tos-cli <command> [options]\n  ve-storage-uni-cli ve-tos <command> [options]\n\n",
        );
    } else {
        output.push_str(
            "  ve-storage-uni-cli ve-tos <command> [options]\n  ve-tos-cli <command> [options]\n\n",
        );
    }
    for (title, category) in [
        ("High-Level Commands", "high_level"),
        ("Low-Level API — Core", "core"),
        ("Low-Level API — Bucket Configuration", "bucket_config"),
        ("Low-Level API — Advanced", "advanced"),
        ("Capabilities / Utilities", "utilities"),
    ] {
        output.push_str(title);
        output.push_str(":\n");
        for entry in COMMAND_GROUPS
            .iter()
            .filter(|entry| entry.category == category)
        {
            output.push_str(&format!("  {:<26} {}\n", entry.name, entry.description));
        }
        output.push('\n');
    }
    output.push_str(
        "TOS Target Syntax:\n  URI:     tos://<bucket>/<key>\n  Flags:   --bucket <NAME> --key <KEY> / --prefix <PREFIX>\n\n",
    );
    output.push_str(global_options);
    output.push_str(&format!(
        "\nExamples:\n  {command_prefix} mb tos://mybucket\n  {command_prefix} mkdir tos://mybucket/folder/\n  {command_prefix} ls tos://mybucket/\n  {command_prefix} cp ./a.txt tos://mybucket/docs/a.txt\n  {command_prefix} cat --bucket mybucket --key docs/a.txt\n  {command_prefix} rm tos://mybucket/docs/a.txt --force --confirm tos://mybucket/docs/a.txt\n  {command_prefix} rb tos://mybucket --force --confirm tos://mybucket\n\n"
    ));
    output.push_str(
        "General:\n  -h, --help                 Print help\n  -V, --version              Print version\n\n",
    );
    output.push_str(
        "Language:\n  --language <en|zh>      Help output language, e.g. --help --language zh\n\n",
    );
    output.push_str(&format!(
        "Run '{command_prefix} <command> --help' for details on a specific command.\n"
    ));
    output.push_str(&format!(
        "Run '{command_prefix} capabilities --view groups' for machine-readable command listing.\n",
    ));
    output.push_str(&format!(
        "Run '{command_prefix} doctor' for environment diagnostics.\n"
    ));
    output
}

fn command_tree_entry(command: &Command, parent: &str) -> CommandTreeEntry {
    let name = command.get_name().to_string();
    let full_command = format!("{parent} {name}");
    let group = (parent == "ve-tos").then(|| find_group(&name)).flatten();
    let description = command
        .get_about()
        .map(|about| about.to_string())
        .or_else(|| group.map(|entry| entry.description.to_string()))
        .unwrap_or_default();
    CommandTreeEntry {
        name,
        command: full_command.clone(),
        layer: group.map(|entry| layer_name(&entry.layer).to_string()),
        category: group.map(|entry| entry.category.to_string()),
        description,
        supports_help: true,
        supports_describe: group.map(|entry| entry.supports_describe).unwrap_or(true),
        implemented: group.map(|entry| entry.implemented).unwrap_or(true),
        parameters: command
            .get_arguments()
            .map(|arg| CommandParameterEntry {
                name: arg.get_id().to_string(),
                required: arg.is_required_set(),
                description: arg
                    .get_help()
                    .map(|help| help.to_string())
                    .unwrap_or_default(),
                long: arg.get_long().map(|long| long.to_string()),
                short: arg.get_short(),
                positional: arg.get_index().is_some()
                    || (arg.get_long().is_none() && arg.get_short().is_none()),
                takes_value: arg.get_action().takes_values(),
            })
            .collect(),
        subcommands: command
            .get_subcommands()
            .map(|subcommand| command_tree_entry(subcommand, &full_command))
            .collect(),
    }
}

fn find_command_tree_entry_in(
    entry: CommandTreeEntry,
    normalized_command: &str,
) -> Option<CommandTreeEntry> {
    if normalize_command(&entry.command) == normalized_command {
        return Some(entry);
    }
    entry
        .subcommands
        .into_iter()
        .find_map(|child| find_command_tree_entry_in(child, normalized_command))
}

fn push_flattened_command(entry: CommandTreeEntry, entries: &mut Vec<CommandTreeEntry>) {
    entries.push(entry.clone());
    for child in entry.subcommands {
        push_flattened_command(child, entries);
    }
}

fn normalize_command(command: &str) -> String {
    let mut parts = command.split_whitespace().collect::<Vec<_>>();
    // [Review Fix #M5] Map the legacy typo to the canonical spelling so that
    // capability lookups succeed regardless of which spelling the caller uses.
    if parts.get(0) == Some(&"ve-tos") && parts.get(1) == Some(&"storgeclass") {
        parts[1] = "storageclass";
    }
    parts.join(" ")
}

fn curated_capability_row(entry: &CapabilityEntry, keep_parameters: bool) -> RegistryCapabilityRow {
    RegistryCapabilityRow {
        command: entry.command.to_string(),
        group: entry.group.to_string(),
        layer: layer_name(&entry.layer).to_string(),
        description: entry.description.to_string(),
        risk_level: risk_name(&entry.risk_level).to_string(),
        destructive: matches!(entry.risk_level, RiskLevel::High | RiskLevel::Critical),
        apis: entry.apis.iter().map(|api| (*api).to_string()).collect(),
        endpoint_rule: entry.endpoint_kind.map(str::to_string),
        method: entry.method.map(str::to_string),
        supports_describe: entry.supports_describe,
        supports_dry_run: entry.supports_dry_run,
        supports_force: entry.supports_force,
        supports_pipe: entry.supports_pipe,
        supports_output_formats: entry.supports_output_formats,
        parameters: keep_parameters.then(|| {
            entry
                .parameters
                .iter()
                .map(|parameter| RegistryCapabilityParameter {
                    name: parameter.name.to_string(),
                    location: format!("{:?}", parameter.location).to_lowercase(),
                    required: parameter.required,
                    description: parameter.description.to_string(),
                })
                .collect()
        }),
        body_contract: entry.body_contract.map(str::to_string),
        consistency_guards: entry.consistency_guards,
        examples: entry
            .examples
            .iter()
            .map(|example| public_tos_example(example))
            .collect(),
        related_commands: entry
            .related_commands
            .iter()
            .map(|command| (*command).to_string())
            .collect(),
    }
}

fn command_tree_capability_row(
    entry: &CommandTreeEntry,
    keep_parameters: bool,
) -> Option<RegistryCapabilityRow> {
    let group = command_root_group(&entry.command)?;
    let group_entry = find_group(group)?;
    let risk = infer_command_risk(&entry.command);
    let apis = inferred_api_names(&entry.command);
    let parameters = keep_parameters.then(|| {
        entry
            .parameters
            .iter()
            .map(|parameter| RegistryCapabilityParameter {
                name: parameter
                    .long
                    .clone()
                    .unwrap_or_else(|| parameter.name.clone()),
                location: if parameter.positional {
                    "path".to_string()
                } else {
                    "flag".to_string()
                },
                required: parameter.required,
                description: parameter.description.clone(),
            })
            .collect()
    });
    let supports_force = entry
        .parameters
        .iter()
        .any(|parameter| parameter.name == "force" || parameter.long.as_deref() == Some("force"));
    Some(RegistryCapabilityRow {
        command: entry.command.clone(),
        group: group.to_string(),
        layer: layer_name(&group_entry.layer).to_string(),
        description: entry.description.clone(),
        risk_level: risk.to_string(),
        destructive: matches!(risk, "high" | "critical"),
        apis,
        endpoint_rule: inferred_endpoint_rule(&entry.command, group_entry),
        method: inferred_method(&entry.command),
        supports_describe: entry.supports_describe,
        supports_dry_run: true,
        supports_force,
        supports_pipe: command_supports_pipe(&entry.command),
        supports_output_formats: SUPPORTED_OUTPUT_FORMATS,
        parameters,
        body_contract: inferred_body_contract(entry),
        consistency_guards: inferred_consistency_guards(&risk),
        examples: vec![format!("{} --describe", public_tos_command(&entry.command))],
        related_commands: vec![
            "ve-tos capabilities --view full".to_string(),
            format!("{} --dry-run", entry.command),
        ],
    })
}

fn tos_example_prefix() -> String {
    std::env::var(TOS_EXAMPLE_PREFIX_ENV).unwrap_or_else(|_| "ve-tos".to_string())
}

pub fn public_tos_command(command: &str) -> String {
    let prefix = tos_example_prefix();
    command
        // [Review Fix #27] `tos` is now a separate top-level command, so ve-tos
        // only normalizes its own public prefixes.
        .strip_prefix("ve-tos ")
        .or_else(|| command.strip_prefix("ve-tos-cli "))
        .or_else(|| command.strip_prefix("ve-storage-uni-cli ve-tos "))
        .map(|suffix| format!("{prefix} {suffix}"))
        .unwrap_or_else(|| command.to_string())
}

fn tos_filter_examples(command: &str) -> Vec<String> {
    let public_command = public_tos_command(command);
    let mut examples = vec![
        format!("{public_command} ... --output json | jq '.data'"),
        format!("{public_command} ... --query 'data'"),
    ];
    match command {
        "ve-tos cp" | "ve-tos mv" => {
            examples.push(format!(
                "{public_command} ... --query 'data.transfers[?status==`failed`].key'"
            ));
            examples.push(format!(
                "{public_command} ... --query 'data.summary.total_bytes'"
            ));
        }
        "ve-tos sync" => {
            examples.push(format!("{public_command} ... --query 'data.summary'"));
            examples.push(format!(
                "{public_command} ... --query 'data.changes[?action==`upload`].key'"
            ));
        }
        "ve-tos rm" | "ve-tos rb" => {
            examples.push(format!(
                "{public_command} ... --dry-run --query 'data.impact'"
            ));
        }
        "ve-tos ls" => {
            examples.push(format!(
                "{public_command} tos://bucket/prefix --query 'data.objects[*].key'"
            ));
            examples.push(format!(
                "{public_command} tos://bucket --query 'data.common_prefixes[*]'"
            ));
        }
        "ve-tos cat" => {
            examples.push(format!(
                "{public_command} tos://bucket/key --output json --query 'data.content'"
            ));
        }
        _ => {}
    }
    examples
}

pub fn public_tos_example(example: &str) -> String {
    let prefix = tos_example_prefix();
    let with_public_pipeline = example
        .replace(" | ve-tos ", &format!(" | {prefix} "))
        .replace("ve-tos-cli ", &format!("{prefix} "))
        .replace("ve-storage-uni-cli ve-tos ", &format!("{prefix} "));
    public_tos_command(&with_public_pipeline)
}

fn command_root_group(command: &str) -> Option<&str> {
    command.split_whitespace().nth(1)
}

fn infer_command_risk(command: &str) -> &'static str {
    let mut parts = command.split_whitespace();
    let _tool = parts.next();
    let root = parts.next().unwrap_or_default();
    let action = parts.last().unwrap_or(root);
    let verb = action.split('-').next().unwrap_or(action);
    if matches!(root, "rb" | "rm")
        || matches!(verb, "delete" | "destroy" | "remove" | "rm")
        || action == "batch-delete"
    {
        return "critical";
    }
    if matches!(verb, "abort") {
        return "high";
    }
    if action == "set-retention" || matches!(verb, "disable") {
        return "high";
    }
    if matches!(
        root,
        "cp" | "mv" | "sync" | "mb" | "restore" | "api" | "config"
    ) || matches!(
        verb,
        "create"
            | "set"
            | "put"
            | "upload"
            | "form-upload"
            | "append"
            | "copy"
            | "complete"
            | "restore"
            | "rename"
            | "modify"
            | "open"
            | "close"
            | "link"
            | "fetch"
            | "create-symlink"
            | "create-fetch-task"
    ) {
        return "medium";
    }
    "low"
}

fn inferred_api_names(command: &str) -> Vec<String> {
    let declared = source_declared_api_names(command);
    if !declared.is_empty() {
        return declared;
    }

    let parts = command.split_whitespace().collect::<Vec<_>>();
    if parts.len() < 3 {
        return Vec::new();
    }
    let action = parts[2..].join("-");
    let root = parts[1];
    synthesized_api_names(root, &action)
}

fn inferred_endpoint_rule(command: &str, group: &CommandGroupEntry) -> Option<String> {
    if let Some(endpoint) = source_declared_endpoint_rule(command) {
        return Some(endpoint);
    }
    match group.category {
        "advanced"
            if matches!(
                group.name,
                "control" | "mrap" | "accelerator" | "cap" | "ap" | "dataset"
            ) =>
        {
            Some("control".to_string())
        }
        "high_level" | "core" | "bucket_config" | "advanced" => Some("data".to_string()),
        _ => None,
    }
}

fn inferred_method(command: &str) -> Option<String> {
    if let Some(method) = source_declared_method(command) {
        return Some(method);
    }
    let action = command.split_whitespace().last().unwrap_or_default();
    let method = if matches!(
        action,
        "list" | "head" | "get" | "stat" | "info" | "location"
    ) {
        "GET"
    } else if matches!(
        action,
        "delete" | "batch-delete" | "abort" | "delete-tagging"
    ) {
        "DELETE"
    } else if matches!(action, "upload" | "set" | "copy" | "complete" | "create") {
        "PUT"
    } else {
        return None;
    };
    Some(method.to_string())
}

fn source_declared_api_names(command: &str) -> Vec<String> {
    let mut apis = BTreeSet::new();
    for source in API_METADATA_SOURCES {
        for api in scan_source_for_api_names(source, command) {
            apis.insert(api);
        }
    }
    apis.into_iter().collect()
}

fn source_declared_method(command: &str) -> Option<String> {
    for source in API_METADATA_SOURCES {
        if let Some(method) = scan_source_for_method(source, command) {
            return Some(method);
        }
    }
    None
}

fn source_declared_endpoint_rule(command: &str) -> Option<String> {
    let source = include_str!("handler/advanced.rs");
    let needle = format!("\"{command}\"");
    let idx = source.find(&needle)?;
    let before = &source[..idx];
    let dp = before.rfind("dp(");
    let cp = before.rfind("cp(");
    match (dp, cp) {
        (Some(d), Some(c)) if d > c => Some("data".to_string()),
        (Some(_), None) => Some("data".to_string()),
        (Some(_), Some(_)) => Some("control".to_string()),
        (None, Some(_)) => Some("control".to_string()),
        (None, None) => None,
    }
}

const API_METADATA_SOURCES: &[&str] = &[
    include_str!("handler/bucket.rs"),
    include_str!("handler/object.rs"),
    include_str!("handler/multipart.rs"),
    include_str!("handler/turbo.rs"),
    include_str!("handler/bucket_config.rs"),
    include_str!("handler/advanced.rs"),
];

fn scan_source_for_api_names(source: &str, command: &str) -> Vec<String> {
    let needle = format!("\"{command}\"");
    let mut rest = source;
    let mut out = Vec::new();
    while let Some(idx) = rest.find(&needle) {
        let after = &rest[idx + needle.len()..];
        if let Some((api, consumed)) = next_quoted_string(after) {
            if looks_like_api_name(api) {
                out.push(api.to_string());
            }
            rest = &after[consumed..];
        } else {
            break;
        }
    }
    out
}

fn scan_source_for_method(source: &str, command: &str) -> Option<String> {
    let needle = format!("\"{command}\"");
    let idx = source.find(&needle)?;
    let window = source[idx..].get(..512).unwrap_or(&source[idx..]);
    [
        ("Method::GET", "GET"),
        ("Method::HEAD", "HEAD"),
        ("Method::PUT", "PUT"),
        ("Method::POST", "POST"),
        ("Method::DELETE", "DELETE"),
        ("Method::PATCH", "PATCH"),
    ]
    .into_iter()
    .filter_map(|(needle, method)| window.find(needle).map(|pos| (pos, method)))
    .min_by_key(|(pos, _)| *pos)
    .map(|(_, method)| method.to_string())
}

fn next_quoted_string(input: &str) -> Option<(&str, usize)> {
    let start = input.find('"')? + 1;
    let tail = &input[start..];
    let end = tail.find('"')?;
    Some((&tail[..end], start + end + 1))
}

fn looks_like_api_name(value: &str) -> bool {
    let Some(first) = value.chars().next() else {
        return false;
    };
    first.is_ascii_uppercase()
        && value.len() >= 3
        && !value.contains(' ')
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '/')
}

fn synthesized_api_names(root: &str, action: &str) -> Vec<String> {
    let parts = action.split('-').collect::<Vec<_>>();
    let verb = parts.first().copied().unwrap_or_default();
    let subject_parts = if parts.len() > 1 {
        &parts[1..]
    } else {
        std::slice::from_ref(&root)
    };
    let verb = match verb {
        "list" => "List",
        "get" | "head" | "stat" | "info" | "location" => "Get",
        "set" | "put" | "upload" | "complete" | "restore" | "open" | "close" => "Put",
        "create" | "copy" | "append" | "fetch" | "link" | "rename" | "modify" => "Create",
        "delete" | "remove" | "rm" | "abort" | "disable" | "destroy" => "Delete",
        other if !other.is_empty() => return vec![pascal_case_api(other, subject_parts)],
        _ => return Vec::new(),
    };
    vec![pascal_case_api(verb, subject_parts)]
}

fn pascal_case_api(verb: &str, subject_parts: &[&str]) -> String {
    let mut out = pascal_segment(verb);
    for part in subject_parts {
        out.push_str(&pascal_segment(part));
    }
    out
}

fn pascal_segment(value: &str) -> String {
    value
        .split(|ch: char| ch == '-' || ch == '_' || ch == '/')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            let Some(first) = chars.next() else {
                return String::new();
            };
            let mut out = String::new();
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
            out
        })
        .collect::<String>()
}

fn command_supports_pipe(command: &str) -> bool {
    matches!(
        command,
        "ve-tos cp" | "ve-tos cat" | "ve-tos object upload" | "ve-tos object download"
    )
}

fn inferred_body_contract(entry: &CommandTreeEntry) -> Option<String> {
    let has_body = entry.parameters.iter().any(|parameter| {
        matches!(
            parameter.name.as_str(),
            "body" | "config" | "request" | "source"
        )
    });
    has_body.then(|| {
        "Request body is supplied by the documented body/config/source flag; use --dry-run before execution.".to_string()
    })
}

fn inferred_consistency_guards(risk: &str) -> &'static [&'static str] {
    match risk {
        "high" | "critical" => &[
            "destructive execution must be reviewed with --dry-run first",
            "real execution requires explicit confirmation flags when supported",
        ],
        "medium" => &["mutating execution is previewable with --dry-run"],
        _ => &["read-only or metadata-only command"],
    }
}

fn layer_name(layer: &CommandLayer) -> &'static str {
    match layer {
        CommandLayer::HighLevel => "high_level",
        CommandLayer::LowLevel => "low_level",
        CommandLayer::Meta => "meta",
    }
}

fn layer_from_name(layer: &str) -> CommandLayer {
    match layer {
        "low_level" => CommandLayer::LowLevel,
        "meta" => CommandLayer::Meta,
        _ => CommandLayer::HighLevel,
    }
}

fn risk_name(risk: &RiskLevel) -> &'static str {
    match risk {
        RiskLevel::Low => "low",
        RiskLevel::Medium => "medium",
        RiskLevel::High => "high",
        RiskLevel::Critical => "critical",
    }
}

fn risk_from_name(risk: &str) -> RiskLevel {
    match risk {
        "medium" => RiskLevel::Medium,
        "high" => RiskLevel::High,
        "critical" => RiskLevel::Critical,
        _ => RiskLevel::Low,
    }
}

fn parameter_location_from_name(location: &str) -> ParameterLocation {
    match location {
        "path" => ParameterLocation::Path,
        "query" => ParameterLocation::Query,
        "header" => ParameterLocation::Header,
        "body" => ParameterLocation::Body,
        _ => ParameterLocation::Flag,
    }
}

const fn group(
    name: &'static str,
    command: &'static str,
    layer: CommandLayer,
    category: &'static str,
    description: &'static str,
    implemented: bool,
) -> CommandGroupEntry {
    CommandGroupEntry {
        name,
        command,
        layer,
        category,
        description,
        supports_help: true,
        supports_describe: implemented,
        implemented,
    }
}

const TRANSFER_PARAMETERS: &[RegistryParameter] = &[
    param(
        "source",
        ParameterLocation::Path,
        true,
        "Source local path or tos:// URI",
    ),
    param(
        "destination",
        ParameterLocation::Path,
        true,
        "Destination local path or tos:// URI",
    ),
    param(
        "recursive",
        ParameterLocation::Flag,
        false,
        "Enable recursive transfer",
    ),
    param(
        "include-parent",
        ParameterLocation::Flag,
        false,
        "Include the source directory/prefix name under the destination prefix",
    ),
    param("include", ParameterLocation::Flag, false, "Include pattern"),
    param("exclude", ParameterLocation::Flag, false, "Exclude pattern"),
    param(
        "checkpoint",
        ParameterLocation::Flag,
        false,
        "Enable resumable checkpoint",
    ),
    param(
        "checkpoint-dir",
        ParameterLocation::Flag,
        false,
        "Checkpoint directory",
    ),
    param(
        "batch-concurrency",
        ParameterLocation::Flag,
        false,
        "Maximum files/items running concurrently in batch execution",
    ),
    param(
        "list-concurrency",
        ParameterLocation::Flag,
        false,
        "Maximum prefixes listed concurrently when recursive listing uses delimiter=\"/\"",
    ),
    param(
        "recursive-list-mode",
        ParameterLocation::Flag,
        false,
        "Recursive listing mode: tos always uses hierarchical delimiter=\"/\"; ve-tos auto uses bucket shape, with flat/hierarchical overrides",
    ),
    param(
        "storage-class",
        ParameterLocation::Header,
        false,
        "Storage class for ve-tos uploads and TOS-to-TOS copies; ByteTOS uploads reject this override",
    ),
    param(
        "acl",
        ParameterLocation::Header,
        false,
        "Target object ACL for TOS uploads/copies",
    ),
    param(
        "meta",
        ParameterLocation::Header,
        false,
        "Custom object metadata as key=value#key2=value2; writes x-tos-meta-* headers",
    ),
    param(
        "report-path",
        ParameterLocation::Flag,
        false,
        "Batch report path",
    ),
    param(
        "report-failures-only",
        ParameterLocation::Flag,
        false,
        "Write only failed items to the batch report",
    ),
    param(
        "manifest-path",
        ParameterLocation::Flag,
        false,
        "Planned transfer manifest path",
    ),
    param(
        "no-manifest",
        ParameterLocation::Flag,
        false,
        "Disable planned manifest output",
    ),
    param(
        "list-echo",
        ParameterLocation::Flag,
        false,
        "Enable listing-phase echo output",
    ),
    param(
        "no-list-echo",
        ParameterLocation::Flag,
        false,
        "Disable listing-phase echo output",
    ),
    param(
        "progress",
        ParameterLocation::Flag,
        false,
        "Enable execution progress output",
    ),
    param(
        "no-progress",
        ParameterLocation::Flag,
        false,
        "Disable execution progress output",
    ),
    param(
        "force",
        ParameterLocation::Flag,
        false,
        "Allow overwrite or destructive behavior",
    ),
];

const SYNC_PARAMETERS: &[RegistryParameter] = &[
    param(
        "source",
        ParameterLocation::Path,
        true,
        "Source local path or tos:// URI",
    ),
    param(
        "destination",
        ParameterLocation::Path,
        true,
        "Destination local path or tos:// URI",
    ),
    param(
        "delete",
        ParameterLocation::Flag,
        false,
        "Delete destination extras",
    ),
    param(
        "force",
        ParameterLocation::Flag,
        false,
        "Required when --delete is set",
    ),
    param(
        "size-only",
        ParameterLocation::Flag,
        false,
        "Compare size only",
    ),
    param(
        "exact-timestamps",
        ParameterLocation::Flag,
        false,
        "Require exact timestamp match",
    ),
    param(
        "include-parent",
        ParameterLocation::Flag,
        false,
        "Include the source directory/prefix name under the destination prefix",
    ),
    param("include", ParameterLocation::Flag, false, "Include pattern"),
    param("exclude", ParameterLocation::Flag, false, "Exclude pattern"),
    param(
        "checkpoint-dir",
        ParameterLocation::Flag,
        false,
        "Checkpoint directory",
    ),
    param(
        "batch-concurrency",
        ParameterLocation::Flag,
        false,
        "Maximum files/items running concurrently in batch execution",
    ),
    param(
        "list-concurrency",
        ParameterLocation::Flag,
        false,
        "Maximum prefixes listed concurrently when recursive listing uses delimiter=\"/\"",
    ),
    param(
        "recursive-list-mode",
        ParameterLocation::Flag,
        false,
        "Recursive listing mode: tos always uses hierarchical delimiter=\"/\"; ve-tos auto uses bucket shape, with flat/hierarchical overrides",
    ),
    param(
        "storage-class",
        ParameterLocation::Header,
        false,
        "Storage class for ve-tos uploads and TOS-to-TOS copies; ByteTOS uploads reject this override",
    ),
    param(
        "acl",
        ParameterLocation::Header,
        false,
        "Target object ACL for TOS uploads/copies",
    ),
    param(
        "meta",
        ParameterLocation::Header,
        false,
        "Custom object metadata as key=value#key2=value2; writes x-tos-meta-* headers",
    ),
    param(
        "report-path",
        ParameterLocation::Flag,
        false,
        "Batch report path",
    ),
    param(
        "report-failures-only",
        ParameterLocation::Flag,
        false,
        "Write only failed items to the batch report",
    ),
    param(
        "manifest-path",
        ParameterLocation::Flag,
        false,
        "Planned transfer manifest path",
    ),
    param(
        "no-manifest",
        ParameterLocation::Flag,
        false,
        "Disable planned manifest output",
    ),
    param(
        "list-echo",
        ParameterLocation::Flag,
        false,
        "Enable listing-phase echo output",
    ),
    param(
        "no-list-echo",
        ParameterLocation::Flag,
        false,
        "Disable listing-phase echo output",
    ),
    param(
        "progress",
        ParameterLocation::Flag,
        false,
        "Enable execution progress output",
    ),
    param(
        "no-progress",
        ParameterLocation::Flag,
        false,
        "Disable execution progress output",
    ),
];

const MB_PARAMETERS: &[RegistryParameter] = &[
    param(
        "bucket",
        ParameterLocation::Path,
        true,
        "Bucket name or tos://bucket",
    ),
    param(
        "region",
        ParameterLocation::Flag,
        false,
        "Region override for this request",
    ),
    param(
        "storage-class",
        ParameterLocation::Flag,
        false,
        "Bucket storage class",
    ),
    param("acl", ParameterLocation::Flag, false, "Bucket ACL"),
    param(
        "az-redundancy",
        ParameterLocation::Flag,
        false,
        "AZ redundancy mode",
    ),
    param(
        "bucket-type",
        ParameterLocation::Flag,
        false,
        "Bucket type. Allowed: fns, hns",
    ),
    param(
        "bucket-object-lock-enabled",
        ParameterLocation::Flag,
        false,
        "Enable bucket object lock",
    ),
];

const RB_PARAMETERS: &[RegistryParameter] = &[
    param(
        "bucket",
        ParameterLocation::Path,
        true,
        "Bucket name or tos://bucket",
    ),
    param(
        "force",
        ParameterLocation::Flag,
        false,
        "Confirm bucket deletion",
    ),
];

const MKDIR_PARAMETERS: &[RegistryParameter] = &[
    param(
        "path",
        ParameterLocation::Path,
        true,
        "tos://bucket/folder/",
    ),
    param(
        "bucket",
        ParameterLocation::Path,
        false,
        "Bucket name when path is omitted",
    ),
    param(
        "key",
        ParameterLocation::Path,
        false,
        "Folder key when --bucket is used",
    ),
    param(
        "parents",
        ParameterLocation::Flag,
        false,
        "Create parent folder markers as needed",
    ),
    param(
        "content-type",
        ParameterLocation::Header,
        true,
        "Fixed application/x-directory for created folder markers",
    ),
];

const OBJECT_BATCH_PARAMETERS: &[RegistryParameter] = &[
    param(
        "path",
        ParameterLocation::Path,
        true,
        "tos://bucket/key or prefix",
    ),
    param(
        "recursive",
        ParameterLocation::Flag,
        false,
        "Apply operation recursively",
    ),
    param(
        "recursive-delete-mode",
        ParameterLocation::Flag,
        false,
        "HNS-only recursive delete strategy: bottom-up or direct",
    ),
    param(
        "batch-concurrency",
        ParameterLocation::Flag,
        false,
        "Maximum files/items running concurrently in batch execution",
    ),
    param(
        "list-concurrency",
        ParameterLocation::Flag,
        false,
        "Maximum prefixes listed concurrently when recursive listing uses delimiter=\"/\"",
    ),
    param(
        "recursive-list-mode",
        ParameterLocation::Flag,
        false,
        "Recursive listing mode: tos always uses hierarchical delimiter=\"/\"; ve-tos auto uses bucket shape, with flat/hierarchical overrides",
    ),
    param("include", ParameterLocation::Flag, false, "Include pattern"),
    param("exclude", ParameterLocation::Flag, false, "Exclude pattern"),
    param(
        "report-path",
        ParameterLocation::Flag,
        false,
        "Batch report path",
    ),
    param(
        "report-failures-only",
        ParameterLocation::Flag,
        false,
        "Write only failed items to the batch report",
    ),
    param(
        "manifest-path",
        ParameterLocation::Flag,
        false,
        "Planned delete manifest path",
    ),
    param(
        "no-manifest",
        ParameterLocation::Flag,
        false,
        "Disable planned manifest output",
    ),
    param(
        "list-echo",
        ParameterLocation::Flag,
        false,
        "Enable listing-phase echo output",
    ),
    param(
        "no-list-echo",
        ParameterLocation::Flag,
        false,
        "Disable listing-phase echo output",
    ),
    param(
        "progress",
        ParameterLocation::Flag,
        false,
        "Enable execution progress output",
    ),
    param(
        "no-progress",
        ParameterLocation::Flag,
        false,
        "Disable execution progress output",
    ),
    param(
        "force",
        ParameterLocation::Flag,
        false,
        "Required for destructive batch operations",
    ),
];

const LISTING_PARAMETERS: &[RegistryParameter] = &[
    param(
        "path",
        ParameterLocation::Path,
        false,
        "tos://bucket or tos://bucket/prefix",
    ),
    param(
        "max-keys",
        ParameterLocation::Query,
        false,
        "Maximum buckets, objects, or prefixes to return from the current level",
    ),
    param(
        "continuation-token",
        ParameterLocation::Query,
        false,
        "Continuation token returned by a previous listing",
    ),
    param(
        "human-readable",
        ParameterLocation::Flag,
        false,
        "Render human-readable sizes",
    ),
    param("sort", ParameterLocation::Flag, false, "Sort field"),
    param(
        "columns",
        ParameterLocation::Flag,
        false,
        "Comma-separated table/csv columns",
    ),
    param(
        "manifest-path",
        ParameterLocation::Flag,
        false,
        "Optional listing manifest path",
    ),
];

const DU_PARAMETERS: &[RegistryParameter] = &[
    param(
        "path",
        ParameterLocation::Path,
        false,
        "tos://bucket or tos://bucket/prefix",
    ),
    param(
        "human-readable",
        ParameterLocation::Flag,
        false,
        "Render human-readable total size",
    ),
    param(
        "max-depth",
        ParameterLocation::Flag,
        false,
        "Maximum directory aggregation depth",
    ),
    param(
        "top-k",
        ParameterLocation::Flag,
        false,
        "Number of largest and oldest object samples to keep in verbose diagnostics; 0 disables samples",
    ),
    param(
        "cost",
        ParameterLocation::Flag,
        false,
        "Include estimated monthly storage cost by storage class",
    ),
    param(
        "storage-price",
        ParameterLocation::Flag,
        false,
        "Override storage price as CLASS=PRICE in CNY/GB/month",
    ),
    param(
        "manifest-path",
        ParameterLocation::Flag,
        false,
        "Optional traversed object manifest path",
    ),
    param(
        "list-concurrency",
        ParameterLocation::Flag,
        false,
        "Maximum prefixes listed concurrently when the bucket is listed hierarchically",
    ),
    param(
        "list-echo",
        ParameterLocation::Flag,
        false,
        "Enable traversal echo output",
    ),
    param(
        "no-list-echo",
        ParameterLocation::Flag,
        false,
        "Disable traversal echo output",
    ),
    param(
        "progress",
        ParameterLocation::Flag,
        false,
        "Legacy alias to enable traversal echo when list echo flags are absent",
    ),
    param(
        "no-progress",
        ParameterLocation::Flag,
        false,
        "Legacy alias to disable traversal echo when list echo flags are absent",
    ),
];

const STAT_PARAMETERS: &[RegistryParameter] = &[
    param(
        "path",
        ParameterLocation::Path,
        true,
        "tos://bucket or tos://bucket/key",
    ),
    param(
        "version-id",
        ParameterLocation::Query,
        false,
        "Object version ID",
    ),
];

const FIND_PARAMETERS: &[RegistryParameter] = &[
    param("path", ParameterLocation::Path, true, "tos://bucket/prefix"),
    param("name", ParameterLocation::Flag, false, "Name glob"),
    param("size", ParameterLocation::Flag, false, "Size predicate"),
    param(
        "mtime",
        ParameterLocation::Flag,
        false,
        "Modified time predicate",
    ),
    param(
        "storage-class",
        ParameterLocation::Flag,
        false,
        "Storage class filter",
    ),
    param(
        "manifest-path",
        ParameterLocation::Flag,
        false,
        "Optional matched object manifest path",
    ),
    param(
        "list-echo",
        ParameterLocation::Flag,
        false,
        "Enable traversal echo output",
    ),
    param(
        "no-list-echo",
        ParameterLocation::Flag,
        false,
        "Disable traversal echo output",
    ),
    param(
        "progress",
        ParameterLocation::Flag,
        false,
        "Legacy alias to enable traversal echo when list echo flags are absent",
    ),
    param(
        "no-progress",
        ParameterLocation::Flag,
        false,
        "Legacy alias to disable traversal echo when list echo flags are absent",
    ),
];

const CAT_PARAMETERS: &[RegistryParameter] = &[
    param("path", ParameterLocation::Path, true, "tos://bucket/key"),
    param("range", ParameterLocation::Header, false, "HTTP range"),
    param(
        "version-id",
        ParameterLocation::Query,
        false,
        "Object version ID",
    ),
];

const PUT_PARAMETERS: &[RegistryParameter] = &[
    param("path", ParameterLocation::Path, true, "tos://bucket/key"),
    param(
        "content-type",
        ParameterLocation::Header,
        false,
        "Content-Type for uploaded object",
    ),
    param(
        "storage-class",
        ParameterLocation::Header,
        false,
        "Storage class for ve-tos stdin uploads; ByteTOS tos put rejects this override",
    ),
    param(
        "acl",
        ParameterLocation::Header,
        false,
        "Target object ACL",
    ),
    param(
        "meta",
        ParameterLocation::Header,
        false,
        "Custom object metadata as key=value#key2=value2; writes x-tos-meta-* headers",
    ),
    param(
        "multipart-threshold",
        ParameterLocation::Flag,
        false,
        "Stdin size threshold for multipart upload; data is uploaded after stdin EOF; defaults to shared checkpoint_threshold",
    ),
    param(
        "no-clobber",
        ParameterLocation::Flag,
        false,
        "Do not overwrite an existing object",
    ),
    param(
        "progress",
        ParameterLocation::Flag,
        false,
        "Enable execution progress output",
    ),
    param(
        "no-progress",
        ParameterLocation::Flag,
        false,
        "Disable execution progress output",
    ),
];

const PRESIGN_PARAMETERS: &[RegistryParameter] = &[
    param("path", ParameterLocation::Path, true, "tos://bucket/key"),
    param(
        "expires",
        ParameterLocation::Query,
        false,
        "URL expiration seconds",
    ),
    param("method", ParameterLocation::Query, false, "HTTP method"),
];

const RESTORE_PARAMETERS: &[RegistryParameter] = &[
    param(
        "path",
        ParameterLocation::Path,
        true,
        "tos://bucket/key or prefix",
    ),
    param(
        "recursive",
        ParameterLocation::Flag,
        false,
        "Restore prefix recursively",
    ),
    param(
        "manifest",
        ParameterLocation::Flag,
        false,
        "Manifest file path",
    ),
    param("days", ParameterLocation::Body, false, "Restore days"),
    param("tier", ParameterLocation::Body, false, "Restore tier"),
    param("include", ParameterLocation::Flag, false, "Include pattern"),
    param("exclude", ParameterLocation::Flag, false, "Exclude pattern"),
    param(
        "force",
        ParameterLocation::Flag,
        false,
        "Required for recursive/manifest restore",
    ),
    param(
        "report-path",
        ParameterLocation::Flag,
        false,
        "Batch report path",
    ),
    param(
        "report-failures-only",
        ParameterLocation::Flag,
        false,
        "Write only failed items to the batch report",
    ),
    param(
        "batch-concurrency",
        ParameterLocation::Flag,
        false,
        "Maximum files/items running concurrently in batch restore execution",
    ),
    param(
        "list-concurrency",
        ParameterLocation::Flag,
        false,
        "Maximum prefixes listed concurrently when recursive listing uses delimiter=\"/\"",
    ),
    param(
        "recursive-list-mode",
        ParameterLocation::Flag,
        false,
        "Recursive listing mode: tos always uses hierarchical delimiter=\"/\"; ve-tos auto uses bucket shape, with flat/hierarchical overrides",
    ),
    param(
        "manifest-path",
        ParameterLocation::Flag,
        false,
        "Planned restore manifest path",
    ),
    param(
        "no-manifest",
        ParameterLocation::Flag,
        false,
        "Disable planned restore manifest output",
    ),
    param(
        "list-echo",
        ParameterLocation::Flag,
        false,
        "Enable listing-phase echo output",
    ),
    param(
        "no-list-echo",
        ParameterLocation::Flag,
        false,
        "Disable listing-phase echo output",
    ),
    param(
        "progress",
        ParameterLocation::Flag,
        false,
        "Enable execution progress output",
    ),
    param(
        "no-progress",
        ParameterLocation::Flag,
        false,
        "Disable execution progress output",
    ),
];

const API_PARAMETERS: &[RegistryParameter] = &[
    param(
        "group",
        ParameterLocation::Path,
        true,
        "Registry group or raw namespace",
    ),
    param(
        "action",
        ParameterLocation::Path,
        true,
        "Registry action or raw operation name",
    ),
    param(
        "request",
        ParameterLocation::Body,
        false,
        "JSON raw request contract or file://path",
    ),
    param(
        "describe",
        ParameterLocation::Flag,
        false,
        "Return metadata or execution plan without sending the request",
    ),
    param(
        "force",
        ParameterLocation::Flag,
        false,
        "Required for POST, PUT, PATCH, DELETE, and other mutating raw requests",
    ),
];

const CONFIG_INIT_PARAMETERS: &[RegistryParameter] = &[param(
    "profile",
    ParameterLocation::Flag,
    false,
    "Profile name, default is default",
)];

const CONFIG_SET_PARAMETERS: &[RegistryParameter] = &[
    param("key", ParameterLocation::Path, true, "Config key"),
    param("value", ParameterLocation::Path, true, "Config value"),
];

const COMPLETION_PARAMETERS: &[RegistryParameter] = &[param(
    "shell",
    ParameterLocation::Path,
    true,
    "Shell to generate: bash, zsh, fish, powershell, or pwsh. Install by extracting data.script from --output json.",
)];

const SERVE_PARAMETERS: &[RegistryParameter] = &[
    param(
        "mcp",
        ParameterLocation::Flag,
        false,
        "Enable the MCP server. Without --mcp, serve only reports registry-backed planning metadata.",
    ),
    param(
        "transport",
        ParameterLocation::Flag,
        false,
        "MCP transport: stdio (default, no TCP listener) or sse (local HTTP/SSE listener).",
    ),
    param(
        "port",
        ParameterLocation::Flag,
        false,
        "Local TCP port for --transport sse; ignored for stdio.",
    ),
];

const SKILL_LIST_PARAMETERS: &[RegistryParameter] = &[param(
    "language",
    ParameterLocation::Flag,
    false,
    "Documentation language for generated skill metadata: en (default) or zh.",
)];

const SKILL_EXPORT_PARAMETERS: &[RegistryParameter] = &[
    param(
        "name",
        ParameterLocation::Flag,
        false,
        "Optional skill name, domain, command, or command suffix filter.",
    ),
    param(
        "dir",
        ParameterLocation::Flag,
        false,
        "Output directory. Files are written as dir/SKILL.md and dir/{domain}/{skill_name}/SKILL.md.",
    ),
    param(
        "language",
        ParameterLocation::Flag,
        false,
        "Documentation language for generated SKILL.md files: en (default) or zh.",
    ),
];

// [Review Fix #s1] Curated parameter sets for the leaf commands that high-level
// flows (cp/mv/sync/rb/restore/cat) compose with. Without these the action-level
// rows fall back to clap-tree derivation and lose risk/guard metadata.
const OBJECT_UPLOAD_PARAMETERS: &[RegistryParameter] = &[
    param(
        "uri",
        ParameterLocation::Path,
        false,
        "tos://bucket/key (or use --bucket + --key)",
    ),
    param(
        "bucket",
        ParameterLocation::Flag,
        false,
        "Bucket name when uri is omitted",
    ),
    param(
        "key",
        ParameterLocation::Flag,
        false,
        "Object key when uri is omitted",
    ),
    param(
        "body",
        ParameterLocation::Body,
        true,
        "File path, file:// URL, '-' for stdin, or inline string",
    ),
    param(
        "content-type",
        ParameterLocation::Header,
        false,
        "Override Content-Type",
    ),
    param(
        "storage-class",
        ParameterLocation::Header,
        false,
        "Storage class for ve-tos uploads; ByteTOS PutObject upload rejects this override",
    ),
];

const OBJECT_DOWNLOAD_PARAMETERS: &[RegistryParameter] = &[
    param(
        "uri",
        ParameterLocation::Path,
        false,
        "tos://bucket/key (or use --bucket + --key)",
    ),
    param(
        "bucket",
        ParameterLocation::Flag,
        false,
        "Bucket name when uri is omitted",
    ),
    param(
        "key",
        ParameterLocation::Flag,
        false,
        "Object key when uri is omitted",
    ),
    param(
        "output",
        ParameterLocation::Flag,
        false,
        "Destination file path; '-' streams to stdout",
    ),
    param(
        "range",
        ParameterLocation::Header,
        false,
        "HTTP range, e.g. bytes=0-1023",
    ),
    param(
        "version-id",
        ParameterLocation::Query,
        false,
        "Object version ID",
    ),
    param(
        "if-match",
        ParameterLocation::Header,
        false,
        "Conditional ETag guard",
    ),
];

const OBJECT_COPY_PARAMETERS: &[RegistryParameter] = &[
    param("source", ParameterLocation::Path, true, "Source tos://bucket/key"),
    param("destination", ParameterLocation::Path, true, "Destination tos://bucket/key"),
    param(
        "metadata-directive",
        ParameterLocation::Header,
        false,
        "COPY, REPLACE, or REPLACE_NEW; maps to x-tos-metadata-directive",
    ),
    param("storage-class", ParameterLocation::Header, false, "Target storage class"),
    param("if-match", ParameterLocation::Header, false, "Destination ETag guard (mapped to the if-match header). Source ETag guarding is performed by high-level cp/mv via x-tos-copy-source-if-match when the source ETag is known."),
];

const OBJECT_DELETE_PARAMETERS: &[RegistryParameter] = &[
    param(
        "uri",
        ParameterLocation::Path,
        false,
        "tos://bucket/key (or use --bucket + --key)",
    ),
    param(
        "bucket",
        ParameterLocation::Flag,
        false,
        "Bucket name when uri is omitted",
    ),
    param(
        "key",
        ParameterLocation::Flag,
        false,
        "Object key when uri is omitted",
    ),
    param(
        "version-id",
        ParameterLocation::Query,
        false,
        "Object version ID",
    ),
    param(
        "force",
        ParameterLocation::Flag,
        false,
        "Required for destructive single-object delete outside dry-run",
    ),
];

const BUCKET_CREATE_PARAMETERS: &[RegistryParameter] = &[
    param(
        "uri",
        ParameterLocation::Path,
        false,
        "Bucket URI (tos://bucket). Alternative to --bucket / bucket_name.",
    ),
    param(
        "bucket_name",
        ParameterLocation::Flag,
        false,
        "Bucket name passed as --bucket. Alternative to positional uri.",
    ),
    param("region", ParameterLocation::Flag, false, "Override region"),
    param(
        "storage-class",
        ParameterLocation::Header,
        false,
        "Default storage class",
    ),
    param("acl", ParameterLocation::Header, false, "Canned ACL"),
    param(
        "bucket-type",
        ParameterLocation::Header,
        false,
        "Bucket type. Allowed: fns, hns",
    ),
];

const BUCKET_DELETE_PARAMETERS: &[RegistryParameter] = &[
    param(
        "uri",
        ParameterLocation::Path,
        false,
        "Bucket URI (tos://bucket). Alternative to --bucket / bucket_name.",
    ),
    param(
        "bucket_name",
        ParameterLocation::Flag,
        false,
        "Bucket name passed as --bucket. Alternative to positional uri.",
    ),
    param(
        "force",
        ParameterLocation::Flag,
        false,
        "Required outside dry-run for the destructive DeleteBucket call",
    ),
];

const fn param(
    name: &'static str,
    location: ParameterLocation,
    required: bool,
    description: &'static str,
) -> RegistryParameter {
    RegistryParameter {
        name,
        location,
        required,
        description,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_registry_covers_top_level_groups() {
        let groups = command_groups()
            .iter()
            .map(|entry| entry.name)
            .collect::<HashSet<_>>();
        for expected in [
            "cp",
            "mv",
            "sync",
            "bucket",
            "object",
            "multipart",
            "data-process",
            "control",
            "capabilities",
            "api",
            "config",
            "completion",
            "serve",
            "skill",
            "doctor",
        ] {
            assert!(groups.contains(expected), "missing group {expected}");
        }
    }

    #[test]
    fn test_registry_covers_high_level_and_config_capabilities() {
        let commands = capabilities()
            .iter()
            .map(|entry| entry.command)
            .collect::<HashSet<_>>();
        for expected in [
            "ve-tos cp",
            "ve-tos mv",
            "ve-tos sync",
            "ve-tos mb",
            "ve-tos rb",
            "ve-tos rm",
            "ve-tos ls",
            "ve-tos stat",
            "ve-tos du",
            "ve-tos find",
            "ve-tos cat",
            "ve-tos put",
            "ve-tos presign",
            "ve-tos restore",
            "ve-tos api",
            "ve-tos config init",
            "ve-tos config show",
            "ve-tos config set",
        ] {
            assert!(commands.contains(expected), "missing capability {expected}");
        }
    }

    #[test]
    fn test_registry_has_no_duplicate_group_names() {
        let mut seen = HashSet::new();
        for entry in command_groups() {
            assert!(seen.insert(entry.name), "duplicate group {}", entry.name);
        }
    }

    #[test]
    fn test_destructive_high_level_capabilities_require_force() {
        for command in ["ve-tos mv", "ve-tos rb", "ve-tos rm", "ve-tos restore"] {
            let entry = find_capability(command).expect("capability");
            assert!(entry.supports_force, "{command} must expose force support");
        }
    }

    /// [Review Fix #6] 不变性：所有 High/Critical 风险且声明 `supports_force` 的命令，
    /// 在真实执行且无 `--force` 下必须被 `enforce_registry_guards` 拒绝。
    #[test]
    fn test_enforce_registry_guards_rejects_high_risk_without_force() {
        for command in ["ve-tos mv", "ve-tos rb", "ve-tos rm", "ve-tos restore"] {
            let result = enforce_registry_guards(command, false, false);
            assert!(
                result.is_err(),
                "{command} should be rejected without --force"
            );
            let violation = result.unwrap_err();
            assert_eq!(violation.command, command);
            assert!(violation.to_string().contains("--force"));
        }
    }

    /// [Review Fix #6] Guard 不应误伤：低风险命令（ls/stat/du/find/cat/presign）即使 supports_force=false 也直接放行。
    #[test]
    fn test_enforce_registry_guards_allows_low_risk_commands() {
        for command in [
            "ve-tos ls",
            "ve-tos stat",
            "ve-tos du",
            "ve-tos find",
            "ve-tos cat",
        ] {
            assert!(
                enforce_registry_guards(command, false, false).is_ok(),
                "low-risk command {command} should pass guard"
            );
        }
    }

    /// [Review Fix #1] Guard must not treat TTY as explicit confirmation.
    #[test]
    fn test_enforce_registry_guards_rejects_high_risk_in_tty_without_force() {
        for command in ["ve-tos mv", "ve-tos rb", "ve-tos rm"] {
            assert!(
                enforce_registry_guards(command, false, true).is_err(),
                "{command} should still require --force in TTY mode"
            );
        }
    }

    /// [Review Fix #6] 显式 `--force` 后 Guard 必须放行。
    #[test]
    fn test_enforce_registry_guards_allows_with_force() {
        for command in ["ve-tos mv", "ve-tos rb", "ve-tos rm", "ve-tos restore"] {
            assert!(
                enforce_registry_guards(command, true, false).is_ok(),
                "{command} with --force should pass guard"
            );
        }
    }

    /// [Review Fix #6] 不变性：registry 与 schema 必须自洽 — 任何 High/Critical 风险的 high-level
    /// 命令都必须显式声明 `supports_force`，以保证 Guard 能拦截。
    #[test]
    fn test_high_risk_high_level_capabilities_declare_force() {
        for entry in capabilities() {
            if !matches!(entry.layer, CommandLayer::HighLevel) {
                continue;
            }
            if matches!(entry.risk_level, RiskLevel::High | RiskLevel::Critical) {
                assert!(
                    entry.supports_force,
                    "{} declared {:?} risk but does not support --force",
                    entry.command, entry.risk_level
                );
            }
        }
    }

    /// [Review Fix #6] 不变性：每个 high-level 命令必须列出非空的 `apis`，否则 Agent
    /// 拿到的 --describe 缺失底层依赖信息。
    #[test]
    fn test_high_level_capabilities_list_low_level_apis() {
        for entry in capabilities() {
            if matches!(entry.layer, CommandLayer::HighLevel) {
                assert!(
                    !entry.apis.is_empty(),
                    "{} is high-level but apis array is empty",
                    entry.command
                );
            }
        }
    }

    /// [Review Fix #6] 不变性：CapabilityEntry 不允许重复登记同一命令（避免 find_capability 行为漂移）。
    #[test]
    fn test_registry_capabilities_have_unique_commands() {
        let mut seen = HashSet::new();
        for entry in capabilities() {
            assert!(
                seen.insert(entry.command),
                "duplicate capability entry for {}",
                entry.command
            );
        }
    }

    #[test]
    fn test_describe_tos_group_is_derived_from_registry() {
        let description = describe_tos_group();
        let groups = description["groups"].as_array().expect("groups array");
        assert_eq!(groups.len(), command_groups().len());
        for entry in command_groups() {
            assert!(
                groups.iter().any(|group| group["name"] == entry.name
                    && group["description"] == entry.description
                    && group["category"] == entry.category),
                "missing registry group {}",
                entry.name
            );
        }
    }

    #[test]
    fn test_command_tree_covers_low_level_actions() {
        let flattened = flattened_command_tree();
        let commands = flattened
            .iter()
            .map(|entry| entry.command.as_str())
            .collect::<HashSet<_>>();
        for expected in [
            "ve-tos bucket create",
            "ve-tos object upload",
            "ve-tos multipart list-parts",
            "ve-tos turbo append",
            "ve-tos lifecycle set",
            "ve-tos data-process list-jobs",
        ] {
            assert!(
                commands.contains(expected),
                "missing command tree entry {expected}"
            );
        }
    }

    /// [Review Fix #6] Cross-validate that the curated `CAPABILITIES` table
    /// agrees with what `infer_leaf_risk` would produce for the same group/
    /// action pair. The two need not match exactly (curated rows can be
    /// intentionally stricter than the action-name heuristic), but the
    /// inferred risk must never *under*-classify a curated High/Critical
    /// command as Low — that would let the leaf-fallback guard wave through
    /// commands that the curated registry already considers destructive.
    #[test]
    fn test_curated_and_inferred_risk_agree_on_force_requirement() {
        for entry in CAPABILITIES.iter() {
            let normalized = normalize_command(entry.command);
            let mut parts = normalized.split_whitespace();
            let _tool = parts.next();
            let group = match parts.next() {
                Some(g) => g,
                None => continue,
            };
            let action = parts.next().unwrap_or(group);
            let inferred = infer_leaf_risk(group, action);
            let curated_is_severe =
                matches!(entry.risk_level, RiskLevel::High | RiskLevel::Critical);
            if curated_is_severe {
                assert!(
                    !matches!(inferred, RiskLevel::Low),
                    "leaf inference under-classifies {}: curated={:?}, inferred={:?}",
                    entry.command,
                    entry.risk_level,
                    inferred
                );
            }
        }
    }

    /// [Review Fix #9] Confirm that every leaf the dispatcher would gate is
    /// reachable from `force_required_commands()`, so doctor reports stay in
    /// lockstep with `enforce_registry_guards`.
    #[test]
    fn test_force_required_commands_are_consistent_with_guard() {
        let listed = force_required_commands();
        for effective in &listed {
            // Calling enforce_registry_guards with force=false must reject
            // every entry surfaced as force-required.
            let result = enforce_registry_guards(&effective.command, false, false);
            assert!(
                result.is_err(),
                "force_required_commands listed {} but guard accepted force=false",
                effective.command
            );
        }
        // And the converse: the well-known critical bucket-removal entrypoint
        // must show up in the list.
        assert!(
            listed.iter().any(|entry| entry.command == "ve-tos rb"),
            "force_required_commands must include `ve-tos rb`"
        );
    }

    #[test]
    fn public_tos_command_does_not_translate_legacy_tos_prefix() {
        assert_eq!(public_tos_command("ve-tos cp"), "ve-tos cp");
        assert_eq!(public_tos_command("tos cp"), "tos cp");
    }
}
