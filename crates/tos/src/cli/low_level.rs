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

use clap::{Args, Subcommand};
use tos_core::agent::error::CliError;

// =============================================================================
// Shared bucket target (positional URI or --bucket flag)
// =============================================================================

fn parse_bucket_uri_value(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    let Some(rest) = trimmed.strip_prefix("tos://") else {
        return Err(format!(
            "invalid bucket URI '{}': positional targets must use tos://bucket; use --bucket <bucket> for flag style",
            raw
        ));
    };
    let mut parts = rest.splitn(2, '/');
    let bucket = parts.next().unwrap_or("");
    if bucket.is_empty() {
        return Err(format!(
            "invalid bucket URI '{}': missing bucket name in tos://bucket",
            raw
        ));
    }
    if let Some(suffix) = parts.next() {
        if !suffix.is_empty() {
            return Err(format!(
                "invalid bucket URI '{}': expected tos://bucket; pass object/prefix parameters separately",
                raw
            ));
        }
    }
    Ok(bucket.to_string())
}

fn parse_bucket_flag_value(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("bucket name cannot be empty".to_string());
    }
    if trimmed.starts_with("tos://") || trimmed.contains('/') {
        return Err(format!(
            // [Review Fix #TOS-BucketTarget-Message] Bucket positional targets are
            // intentionally limited to tos://bucket; object/prefix data travels
            // through the dedicated path, key, or prefix parameters.
            "invalid bucket flag '{}': --bucket expects a bucket name only; use positional tos://bucket for URI style",
            raw
        ));
    }
    Ok(trimmed.to_string())
}

fn parse_object_list_uri_value(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    let Some(rest) = trimmed.strip_prefix("tos://") else {
        return Err(format!(
            "invalid object list URI '{}': positional targets must use tos://bucket[/prefix]; use --bucket <bucket> --prefix <prefix> for flag style",
            raw
        ));
    };
    let bucket = rest.split('/').next().unwrap_or("");
    if bucket.is_empty() {
        return Err(format!(
            "invalid object list URI '{}': missing bucket name in tos://bucket[/prefix]",
            raw
        ));
    }
    Ok(trimmed.to_string())
}

/// A unified bucket selector: accepts a positional `tos://bucket` URI or a
/// `--bucket <bucket>` flag.
///
/// Resolution rules:
/// - If both positional and flag are provided and differ, return an error.
/// - Otherwise return whichever is present.
/// - If neither is present, return a validation error.
#[derive(Debug, Args, Clone, Default)]
pub struct BucketTarget {
    /// Bucket URI (tos://bucket)
    #[arg(value_name = "URI", value_parser = parse_bucket_uri_value)]
    pub bucket_pos: Option<String>,

    /// Bucket name (flag style)
    #[arg(long = "bucket", value_name = "BUCKET", value_parser = parse_bucket_flag_value)]
    pub bucket_flag: Option<String>,
}

impl BucketTarget {
    /// Convenience constructor (used in tests and programmatic invocations).
    pub fn from_name(name: impl Into<String>) -> Self {
        Self {
            bucket_pos: Some(name.into()),
            bucket_flag: None,
        }
    }

    /// Resolve the effective bucket name. Returns a `String` for ergonomic use
    /// at call sites that previously held a `String` field.
    pub fn require(&self) -> Result<String, CliError> {
        match (self.bucket_pos.as_deref(), self.bucket_flag.as_deref()) {
            (Some(p), Some(f)) if p != f => Err(CliError::ValidationError(format!(
                "conflicting bucket: positional '{}' vs --bucket '{}'",
                p, f
            ))),
            (Some(value), _) | (_, Some(value)) => Ok(value.to_string()),
            (None, None) => Err(CliError::ValidationError(
                "bucket name is required (provide tos://bucket or --bucket NAME)".to_string(),
            )),
        }
    }
}

// =============================================================================
// Shared Arg Types for Bucket Config Commands
// =============================================================================

/// Common args for bucket config get/delete operations (positional or --bucket)
#[derive(Debug, Args)]
pub struct BucketArg {
    #[command(flatten)]
    pub bucket: BucketTarget,
}

/// Common args for bucket config set operations (bucket + --config)
#[derive(Debug, Args)]
pub struct BucketConfigSetArg {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Configuration (JSON or file://path)
    #[arg(long)]
    pub config: String,
}

/// Generic args for advanced feature commands (skeleton)
#[derive(Debug, Args)]
pub struct GenericArgs {
    /// Resource identifier (bucket name, access point name, etc.)
    #[arg(long)]
    pub name: Option<String>,
    /// Bucket name
    #[arg(long)]
    pub bucket: Option<String>,
    /// Resource ID
    #[arg(long)]
    pub id: Option<String>,
    /// Image style name (styleName query)
    #[arg(long)]
    pub style_name: Option<String>,
    /// Job ID (jobID path/query)
    #[arg(long)]
    pub job_id: Option<String>,
    /// Data processing job type (job_type query)
    #[arg(long)]
    pub job_type: Option<String>,
    /// MRAP alias
    #[arg(long)]
    pub alias: Option<String>,
    /// Accelerator ID or name used in path parameters
    #[arg(long)]
    pub accelerator: Option<String>,
    /// Accelerator ID used by accelerator APIs
    #[arg(long)]
    pub accelerator_id: Option<String>,
    /// Bucket name path parameter for control-plane binding APIs
    #[arg(long)]
    pub bucket_name: Option<String>,
    /// Custom endpoint domain
    #[arg(long)]
    pub domain: Option<String>,
    /// Availability zone
    #[arg(long)]
    pub az: Option<String>,
    /// Region query parameter
    #[arg(long)]
    pub region: Option<String>,
    /// Resource TRN path parameter
    #[arg(long)]
    pub resource_trn: Option<String>,
    /// Tag keys query parameter (comma-separated or JSON array)
    #[arg(long)]
    pub tag_keys: Option<String>,
    /// Data-process template tag query parameter
    #[arg(long)]
    pub tag: Option<String>,
    /// Object set name encoded as query key
    #[arg(long)]
    pub object_set_name: Option<String>,
    /// Object key path parameter
    #[arg(long)]
    pub object: Option<String>,
    /// Configuration (JSON or file://path)
    #[arg(long)]
    pub config: Option<String>,
    /// Content-MD5 request header for JSON body
    #[arg(long)]
    pub content_md5: Option<String>,
    /// Extra query parameter, repeatable, in k=v form
    #[arg(long = "query")]
    pub query: Vec<String>,
    /// Extra request header, repeatable, in k=v form
    #[arg(long = "header")]
    pub header: Vec<String>,
    /// Confirm destructive Advanced operation before execution
    #[arg(long)]
    pub force: bool,
}

// =============================================================================
// Core: Bucket (7 actions)
// =============================================================================

#[derive(Debug, Args)]
#[command(
    about = "Bucket core APIs",
    long_about = "Low-Level API — Core: bucket operations.",
    after_help = "Examples:\n  ve-tos-cli bucket create --bucket mybucket --region cn-beijing\n  ve-tos-cli bucket list\n  ve-tos-cli bucket head --bucket mybucket\n  ve-tos-cli bucket delete --bucket mybucket"
)]
pub struct BucketCommand {
    #[command(subcommand)]
    pub action: Option<BucketAction>,
}

#[derive(Debug, Subcommand)]
pub enum BucketAction {
    /// Create a new bucket
    Create(BucketCreateArgs),
    /// Get bucket metadata (HeadBucket)
    Head(BucketHeadArgs),
    /// Delete a bucket
    Delete(BucketDeleteArgs),
    /// List all buckets
    List(BucketListArgs),
    /// Get bucket statistics
    Stat(BucketStatArgs),
    /// Get bucket detailed information
    Info(BucketInfoArgs),
    /// Get bucket location
    Location(BucketLocationArgs),
}

#[derive(Debug, Args)]
pub struct BucketCreateArgs {
    /// Bucket URI (tos://bucket)
    #[arg(value_name = "URI", value_parser = parse_bucket_uri_value, conflicts_with = "bucket_name")]
    pub uri: Option<String>,
    /// Bucket name (flag style)
    #[arg(long = "bucket", value_name = "BUCKET", value_parser = parse_bucket_flag_value)]
    pub bucket_name: Option<String>,
    /// Region override for this request
    #[arg(long)]
    pub region: Option<String>,
    /// Storage class. Allowed: STANDARD, IA, ARCHIVE_FR, INTELLIGENT_TIERING, COLD_ARCHIVE, ARCHIVE, DEEP_COLD_ARCHIVE
    #[arg(long, default_value = "STANDARD")]
    pub storage_class: String,
    /// Bucket type header (x-tos-bucket-type; allowed: fns, hns)
    #[arg(long)]
    pub bucket_type: Option<String>,
    /// Project name header (x-tos-project-name)
    #[arg(long)]
    pub project_name: Option<String>,
    /// Enable bucket object lock (x-tos-bucket-object-lock-enabled=true)
    #[arg(long)]
    pub bucket_object_lock_enabled: bool,
    /// Bucket ACL (x-tos-acl). Allowed: private, public-read, public-read-write, authenticated-read, bucket-owner-read, bucket-owner-full-control
    #[arg(long)]
    pub acl: Option<String>,
    /// Grant full control (x-tos-grant-full-control)
    #[arg(long)]
    pub grant_full_control: Option<String>,
    /// Grant read (x-tos-grant-read)
    #[arg(long)]
    pub grant_read: Option<String>,
    /// Grant read without list (x-tos-grant-read-non-list)
    #[arg(long)]
    pub grant_read_non_list: Option<String>,
    /// Grant read ACP (x-tos-grant-read-acp)
    #[arg(long)]
    pub grant_read_acp: Option<String>,
    /// Grant write (x-tos-grant-write)
    #[arg(long)]
    pub grant_write: Option<String>,
    /// Grant write ACP (x-tos-grant-write-acp)
    #[arg(long)]
    pub grant_write_acp: Option<String>,
    /// AZ redundancy (x-tos-az-redundancy). Allowed: single-az, multi-az
    #[arg(long)]
    pub az_redundancy: Option<String>,
    /// Object tags (x-tos-tagging; key1=value1&key2=value2)
    #[arg(long)]
    pub tagging: Option<String>,
}

#[derive(Debug, Args)]
pub struct BucketHeadArgs {
    /// Bucket URI (tos://bucket)
    #[arg(value_name = "URI", value_parser = parse_bucket_uri_value, conflicts_with = "bucket_name")]
    pub uri: Option<String>,
    /// Bucket name (flag style)
    #[arg(long = "bucket", value_name = "BUCKET", value_parser = parse_bucket_flag_value)]
    pub bucket_name: Option<String>,
}

#[derive(Debug, Args)]
pub struct BucketDeleteArgs {
    /// Bucket URI (tos://bucket)
    #[arg(value_name = "URI", value_parser = parse_bucket_uri_value, conflicts_with = "bucket_name")]
    pub uri: Option<String>,
    /// Bucket name (flag style)
    #[arg(long = "bucket", value_name = "BUCKET", value_parser = parse_bucket_flag_value)]
    pub bucket_name: Option<String>,
    /// Force delete bucket contents first (?force)
    #[arg(long)]
    pub force: bool,
    /// Destroy bucket permanently (?destroy)
    #[arg(long, conflicts_with = "force")]
    pub destroy: bool,
}

#[derive(Debug, Args)]
pub struct BucketListArgs {
    /// Filter by project name
    #[arg(long)]
    pub project_name: Option<String>,
    /// Filter by bucket type (x-tos-bucket-type; allowed: fns, hns)
    #[arg(long)]
    pub bucket_type: Option<String>,
}

#[derive(Debug, Args)]
pub struct BucketStatArgs {
    /// Bucket URI (tos://bucket)
    #[arg(value_name = "URI", value_parser = parse_bucket_uri_value, conflicts_with = "bucket_name")]
    pub uri: Option<String>,
    /// Bucket name (flag style)
    #[arg(long = "bucket", value_name = "BUCKET", value_parser = parse_bucket_flag_value)]
    pub bucket_name: Option<String>,
}

#[derive(Debug, Args)]
pub struct BucketInfoArgs {
    /// Bucket URI (tos://bucket)
    #[arg(value_name = "URI", value_parser = parse_bucket_uri_value, conflicts_with = "bucket_name")]
    pub uri: Option<String>,
    /// Bucket name (flag style)
    #[arg(long = "bucket", value_name = "BUCKET", value_parser = parse_bucket_flag_value)]
    pub bucket_name: Option<String>,
}

#[derive(Debug, Args)]
pub struct BucketLocationArgs {
    /// Bucket URI (tos://bucket)
    #[arg(value_name = "URI", value_parser = parse_bucket_uri_value, conflicts_with = "bucket_name")]
    pub uri: Option<String>,
    /// Bucket name (flag style)
    #[arg(long = "bucket", value_name = "BUCKET", value_parser = parse_bucket_flag_value)]
    pub bucket_name: Option<String>,
}

// =============================================================================
// Core: Object (32 actions)
// =============================================================================

#[derive(Debug, Args)]
#[command(
    about = "Object core APIs",
    long_about = "Low-Level API — Core: object operations.",
    after_help = "Examples:\n  ve-tos-cli object upload --bucket mybucket --key hello.txt --body file://hello.txt\n  ve-tos-cli object download --bucket mybucket --key hello.txt --body ./hello.txt\n  ve-tos-cli object head --bucket mybucket --key hello.txt\n  ve-tos-cli object delete tos://mybucket/hello.txt --force --confirm tos://mybucket/hello.txt\n  ve-tos-cli object list --bucket mybucket --prefix logs/\n  ve-tos-cli object copy tos://mybucket/src.txt tos://mybucket/dest.txt"
)]
pub struct ObjectCommand {
    #[command(subcommand)]
    pub action: Option<ObjectAction>,
}

#[derive(Debug, Subcommand)]
pub enum ObjectAction {
    /// Upload a single object (PutObject, <=5GB)
    Upload(ObjectUploadArgs),
    /// Download a single object (GetObject)
    Download(ObjectDownloadArgs),
    /// Upload object via form (PostObject)
    #[command(name = "form-upload")]
    FormUpload(ObjectFormUploadArgs),
    /// Copy an object (server-side CopyObject)
    Copy(ObjectCopyArgs),
    /// Delete a single object
    Delete(ObjectDeleteArgs),
    /// Batch delete objects (DeleteMultiObjects)
    #[command(name = "batch-delete")]
    BatchDelete(ObjectBatchDeleteArgs),
    /// List objects (ListObjectsV2)
    List(ObjectListArgs),
    /// List object versions
    #[command(name = "list-versions")]
    ListVersions(ObjectListVersionsArgs),
    /// Get object metadata (HeadObject)
    Head(ObjectHeadArgs),
    /// Get object stat information
    Stat(ObjectStatArgs),
    /// Set object metadata
    #[command(name = "set-meta")]
    SetMeta(ObjectSetMetaArgs),
    /// Set object time attributes
    #[command(name = "set-time")]
    SetTime(ObjectSetTimeArgs),
    /// Set object expiration time
    #[command(name = "set-expires")]
    SetExpires(ObjectSetExpiresArgs),
    /// Append data to an appendable object
    Append(ObjectAppendArgs),
    /// Seal an appendable object (make it immutable)
    #[command(name = "seal-append")]
    SealAppend(ObjectSealAppendArgs),
    /// Modify an object in-place
    Modify(ObjectModifyArgs),
    /// Rename an object
    Rename(ObjectRenameArgs),
    /// Restore an archived object
    Restore(ObjectRestoreArgs),
    /// Get object processing status
    Status(ObjectStatusArgs),
    /// Get object ACL
    #[command(name = "get-acl")]
    GetAcl(ObjectGetAclArgs),
    /// Set object ACL
    #[command(name = "set-acl")]
    SetAcl(ObjectSetAclArgs),
    /// Get object tagging
    #[command(name = "get-tagging")]
    GetTagging(ObjectGetTaggingArgs),
    /// Set object tagging
    #[command(name = "set-tagging")]
    SetTagging(ObjectSetTaggingArgs),
    /// Delete object tagging
    #[command(name = "delete-tagging")]
    DeleteTagging(ObjectDeleteTaggingArgs),
    /// Create a hard link to an object
    Link(ObjectLinkArgs),
    /// Get symlink target
    #[command(name = "get-symlink")]
    GetSymlink(ObjectGetSymlinkArgs),
    /// Create a symbolic link
    #[command(name = "create-symlink")]
    CreateSymlink(ObjectCreateSymlinkArgs),
    /// Get async fetch task status
    #[command(name = "get-fetch-task")]
    GetFetchTask(ObjectGetFetchTaskArgs),
    /// Create an async fetch task
    #[command(name = "create-fetch-task")]
    CreateFetchTask(ObjectCreateFetchTaskArgs),
    /// Fetch an external object synchronously
    Fetch(ObjectFetchArgs),
    /// Set object retention policy
    #[command(name = "set-retention")]
    SetRetention(ObjectSetRetentionArgs),
    /// Get object retention policy
    #[command(name = "get-retention")]
    GetRetention(ObjectGetRetentionArgs),
}

// --- Existing detailed args ---

#[derive(Debug, Args)]
pub struct ObjectUploadArgs {
    /// Object path (tos://bucket/key or --bucket + --key)
    pub uri: Option<String>,
    /// Bucket name
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key
    #[arg(long)]
    pub key: Option<String>,
    /// File to upload (or - for stdin)
    #[arg(long)]
    pub body: Option<String>,
    /// Content type
    #[arg(long)]
    pub content_type: Option<String>,
    /// Storage class for ve-tos uploads; ByteTOS tos object upload does not support creation-time override
    #[arg(long)]
    pub storage_class: Option<String>,
    /// Custom metadata (key1=val1&key2=val2)
    #[arg(long)]
    pub meta: Option<String>,
    /// Net speed test marker header (X-Tos-Net-Speed-Test)
    #[arg(long)]
    pub net_speed_test: Option<String>,
    /// ACL value (x-tos-acl)
    #[arg(long)]
    pub acl: Option<String>,
    /// Grant full control
    #[arg(long)]
    pub grant_full_control: Option<String>,
    /// Grant read
    #[arg(long)]
    pub grant_read: Option<String>,
    /// Grant read without list
    #[arg(long)]
    pub grant_read_non_list: Option<String>,
    /// Grant read ACP
    #[arg(long)]
    pub grant_read_acp: Option<String>,
    /// Grant write
    #[arg(long)]
    pub grant_write: Option<String>,
    /// Grant write ACP
    #[arg(long)]
    pub grant_write_acp: Option<String>,
    /// Object tagging (key1=value1&key2=value2)
    #[arg(long)]
    pub tagging: Option<String>,
    /// Object lock mode (x-object-lock-mode)
    #[arg(long)]
    pub object_lock_mode: Option<String>,
    /// Object lock retain until date (x-object-lock-retain-until-date)
    #[arg(long)]
    pub object_lock_retain_until_date: Option<String>,
    /// Prevent overwrite if object exists (if-none-match: *)
    #[arg(long)]
    pub if_none_match: Option<String>,
    /// Forbid overwrite (x-tos-forbid-overwrite)
    #[arg(long)]
    pub forbid_overwrite: bool,
    /// Content-MD5 for integrity check
    #[arg(long)]
    pub content_md5: Option<String>,
    /// Traffic limit in bps (x-traffic-limit)
    #[arg(long)]
    pub traffic_limit: Option<u64>,
    /// Persistent custom response headers (x-persistent-headers)
    #[arg(long)]
    pub persistent_headers: Option<String>,
    /// ETag pattern hint (x-etag-pattern)
    #[arg(long)]
    pub etag_pattern: Option<String>,
}

#[derive(Debug, Args)]
pub struct ObjectDownloadArgs {
    /// Object path (tos://bucket/key or --bucket + --key)
    pub uri: Option<String>,
    /// Bucket name
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key
    #[arg(long)]
    pub key: Option<String>,
    /// Output file (or - for stdout)
    #[arg(long)]
    pub body: Option<String>,
    /// Version ID
    #[arg(long)]
    pub version_id: Option<String>,
    /// Range header (bytes=start-end)
    #[arg(long)]
    pub range: Option<String>,
    /// If-Modified-Since header
    #[arg(long)]
    pub if_modified_since: Option<String>,
    /// If-Unmodified-Since header
    #[arg(long)]
    pub if_unmodified_since: Option<String>,
    /// If-Match (ETag condition)
    #[arg(long)]
    pub if_match: Option<String>,
    /// If-None-Match (ETag condition)
    #[arg(long)]
    pub if_none_match: Option<String>,
    /// Traffic limit in bps (x-traffic-limit)
    #[arg(long)]
    pub traffic_limit: Option<u64>,
    /// Override response Content-Type
    #[arg(long)]
    pub response_content_type: Option<String>,
    /// Override response Content-Disposition
    #[arg(long)]
    pub response_content_disposition: Option<String>,
    /// Override response Cache-Control
    #[arg(long)]
    pub response_cache_control: Option<String>,
    /// Override response Expires
    #[arg(long)]
    pub response_expires: Option<String>,
    /// Read specific part number of multipart upload
    #[arg(long)]
    pub part_number: Option<u32>,
    /// X-Replicated-From header
    #[arg(long)]
    pub replicated_from: Option<String>,
    /// X-From-Modular header
    #[arg(long)]
    pub from_modular: Option<String>,
}

#[derive(Debug, Args)]
pub struct ObjectFormUploadArgs {
    /// Object path (tos://bucket/key or --bucket + --key)
    pub uri: Option<String>,
    /// Bucket name
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key
    #[arg(long)]
    pub key: Option<String>,
    /// File to upload
    #[arg(long)]
    pub body: Option<String>,
    /// Content type
    #[arg(long)]
    pub content_type: Option<String>,
    /// Storage class
    #[arg(long)]
    pub storage_class: Option<String>,
    /// Custom metadata (key1=val1&key2=val2)
    #[arg(long)]
    pub meta: Option<String>,
}

#[derive(Debug, Args)]
pub struct ObjectCopyArgs {
    /// Source (tos://src-bucket/src-key)
    pub source: String,
    /// Destination (tos://dst-bucket/dst-key)
    pub destination: String,
    /// Source byte range
    #[arg(long)]
    pub range: Option<String>,
    /// Source modified-since condition
    #[arg(long)]
    pub copy_source_if_modified_since: Option<String>,
    /// Source unmodified-since condition
    #[arg(long)]
    pub copy_source_if_unmodified_since: Option<String>,
    /// ETag pattern hint
    #[arg(long)]
    pub etag_pattern: Option<String>,
    /// Metadata directive
    #[arg(long)]
    pub metadata_directive: Option<String>,
    /// Tagging directive
    #[arg(long)]
    pub tagging_directive: Option<String>,
    /// Unique tag (x-unique-tag)
    #[arg(long)]
    pub unique_tag: Option<String>,
    /// Copy source last modified (x-tos-copy-source-last-modified)
    #[arg(long)]
    pub copy_source_last_modified: Option<String>,
    /// Data ID (x-data-id)
    #[arg(long)]
    pub data_id: Option<String>,
    /// Fingerprint (x-finger-print)
    #[arg(long)]
    pub finger_print: Option<String>,
    /// Internal metadata directive (x-internal-metadata-directive)
    #[arg(long)]
    pub internal_metadata_directive: Option<String>,
    /// CRR source timestamp nsec (x-crr-source-timestamp-nsec)
    #[arg(long)]
    pub crr_source_timestamp_nsec: Option<String>,
    /// CRR proxy (x-crr-proxy)
    #[arg(long)]
    pub crr_proxy: Option<String>,
    /// CRR source bucket version status (x-crr-source-bucket-version-status)
    #[arg(long)]
    pub crr_source_bucket_version_status: Option<String>,
    /// Traffic limit (x-traffic-limit)
    #[arg(long)]
    pub traffic_limit: Option<u64>,
    /// Object lock mode (x-object-lock-mode)
    #[arg(long)]
    pub object_lock_mode: Option<String>,
    /// Object lock retain-until date (x-object-lock-retain-until-date)
    #[arg(long)]
    pub object_lock_retain_until_date: Option<String>,
    /// If-Unmodified-Since (x-if-unmodified-since)
    #[arg(long)]
    pub if_unmodified_since: Option<String>,
    /// If-None-Match
    #[arg(long)]
    pub if_none_match: Option<String>,
    /// If-Match
    #[arg(long)]
    pub if_match: Option<String>,
    /// Persistent headers list (x-persistent-headers)
    #[arg(long)]
    pub persistent_headers: Option<String>,
    /// Tagging (x-tagging; key1=value1&key2=value2)
    #[arg(long)]
    pub tagging: Option<String>,
    /// ACL value (x-tos-acl)
    #[arg(long)]
    pub acl: Option<String>,
    /// Grant full control
    #[arg(long)]
    pub grant_full_control: Option<String>,
    /// Grant read
    #[arg(long)]
    pub grant_read: Option<String>,
    /// Grant read without list
    #[arg(long)]
    pub grant_read_non_list: Option<String>,
    /// Grant read ACP
    #[arg(long)]
    pub grant_read_acp: Option<String>,
    /// Grant write
    #[arg(long)]
    pub grant_write: Option<String>,
    /// Grant write ACP
    #[arg(long)]
    pub grant_write_acp: Option<String>,
}

#[derive(Debug, Args)]
pub struct ObjectDeleteArgs {
    /// Object path (tos://bucket/key or --bucket + --key)
    pub uri: Option<String>,
    /// Bucket name
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key
    #[arg(long)]
    pub key: Option<String>,
    /// Version ID
    #[arg(long)]
    pub version_id: Option<String>,
    /// Confirm destructive delete before execution
    #[arg(long)]
    pub force: bool,
    /// X-From-Modular header
    #[arg(long)]
    pub from_modular: Option<String>,
    /// X-If-Match-Expires header
    #[arg(long)]
    pub if_match_expires: Option<String>,
    /// Last-Modified header
    #[arg(long)]
    pub last_modified: Option<String>,
    /// X-If-Match-CreateTime header
    #[arg(long)]
    pub if_match_create_time: Option<String>,
    /// If-Match header
    #[arg(long)]
    pub if_match: Option<String>,
    /// X-If-Match-Tags header
    #[arg(long)]
    pub if_match_tags: Option<String>,
    /// X-If-Match-AccessTime header
    #[arg(long)]
    pub if_match_access_time: Option<String>,
    /// x-lifecycle-directly-delete-versions header
    #[arg(long)]
    pub lifecycle_directly_delete_versions: bool,
    /// x-if-match-inode-id header
    #[arg(long)]
    pub if_match_inode_id: Option<String>,
    /// x-parent-inode-id header
    #[arg(long)]
    pub parent_inode_id: Option<String>,
    /// x-only-put-delete-marker header
    #[arg(long)]
    pub only_put_delete_marker: bool,
    /// X-Inner-Properties-TimeStamp header
    #[arg(long)]
    pub inner_properties_timestamp: Option<String>,
    /// X-Inner-Properties-TimeStampNsec header
    #[arg(long)]
    pub inner_properties_timestamp_nsec: Option<String>,
}

#[derive(Debug, Args)]
pub struct ObjectBatchDeleteArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Keys to delete (JSON array or comma-separated keys)
    #[arg(long)]
    pub keys: String,
    /// Confirm destructive delete before execution
    #[arg(long)]
    pub force: bool,
    /// Recursive delete flag (queryRecursive)
    #[arg(long)]
    pub recursive: bool,
    /// Skip trash flag (querySkipTrash)
    #[arg(long)]
    pub skip_trash: bool,
    /// Content-MD5 header
    #[arg(long)]
    pub content_md5: Option<String>,
}

#[derive(Debug, Args)]
pub struct ObjectListArgs {
    /// Object list URI (tos://bucket or tos://bucket/prefix/)
    #[arg(value_name = "URI", value_parser = parse_object_list_uri_value, conflicts_with_all = ["bucket", "prefix"])]
    pub uri: Option<String>,
    /// Bucket name
    #[arg(long, value_name = "BUCKET", value_parser = parse_bucket_flag_value)]
    pub bucket: Option<String>,
    /// Object prefix
    #[arg(long)]
    pub prefix: Option<String>,
    /// Delimiter
    #[arg(long)]
    pub delimiter: Option<String>,
    /// Maximum keys per response
    #[arg(long, default_value = "1000")]
    pub max_keys: u32,
    /// Continuation token
    #[arg(long)]
    pub continuation_token: Option<String>,
}

#[derive(Debug, Args)]
pub struct ObjectListVersionsArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Prefix filter
    #[arg(long)]
    pub prefix: Option<String>,
}

#[derive(Debug, Args)]
pub struct ObjectHeadArgs {
    /// Object path (tos://bucket/key or --bucket + --key)
    pub uri: Option<String>,
    /// Bucket name
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key
    #[arg(long)]
    pub key: Option<String>,
    /// Version ID
    #[arg(long)]
    pub version_id: Option<String>,
    /// If-Modified-Since header
    #[arg(long)]
    pub if_modified_since: Option<String>,
    /// If-Unmodified-Since header
    #[arg(long)]
    pub if_unmodified_since: Option<String>,
    /// If-Match (ETag condition)
    #[arg(long)]
    pub if_match: Option<String>,
    /// If-None-Match (ETag condition)
    #[arg(long)]
    pub if_none_match: Option<String>,
    /// Range header (bytes=start-end)
    #[arg(long)]
    pub range: Option<String>,
    /// X-Replicated-From header
    #[arg(long)]
    pub replicated_from: Option<String>,
    /// X-From-Modular header
    #[arg(long)]
    pub from_modular: Option<String>,
}

#[derive(Debug, Args)]
pub struct ObjectStatArgs {
    /// Object path (tos://bucket/key or --bucket + --key)
    pub uri: Option<String>,
    /// Bucket name
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key
    #[arg(long)]
    pub key: Option<String>,
}

#[derive(Debug, Args)]
pub struct ObjectSetMetaArgs {
    /// Object path (tos://bucket/key or --bucket + --key)
    pub uri: Option<String>,
    /// Bucket name
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key
    #[arg(long)]
    pub key: Option<String>,
    /// Metadata (JSON or key1=val1&key2=val2)
    #[arg(long)]
    pub meta: String,
    /// Version ID
    #[arg(long)]
    pub version_id: Option<String>,
    /// Unique tag (x-unique-tag)
    #[arg(long)]
    pub unique_tag: Option<String>,
    /// Content-Type header
    #[arg(long)]
    pub content_type: Option<String>,
}

#[derive(Debug, Args)]
pub struct ObjectSetTimeArgs {
    /// Object path (tos://bucket/key or --bucket + --key)
    pub uri: Option<String>,
    /// Bucket name
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key
    #[arg(long)]
    pub key: Option<String>,
    /// Timestamp (RFC3339 or Unix epoch)
    #[arg(long)]
    pub time: String,
    /// x-modify-timestamp header
    #[arg(long)]
    pub modify_timestamp: Option<String>,
    /// x-modify-timestamp-ns header
    #[arg(long)]
    pub modify_timestamp_ns: Option<String>,
}

#[derive(Debug, Args)]
pub struct ObjectSetExpiresArgs {
    /// Object path (tos://bucket/key or --bucket + --key)
    pub uri: Option<String>,
    /// Bucket name
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key
    #[arg(long)]
    pub key: Option<String>,
    /// Expiration time (RFC3339 or Unix epoch)
    #[arg(long)]
    pub expires: String,
    /// Version ID
    #[arg(long)]
    pub version_id: Option<String>,
}

#[derive(Debug, Args)]
pub struct ObjectAppendArgs {
    /// Object path (tos://bucket/key or --bucket + --key)
    pub uri: Option<String>,
    /// Bucket name
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key
    #[arg(long)]
    pub key: Option<String>,
    /// Body source (file path or inline data)
    #[arg(long)]
    pub body: String,
    /// Append offset
    #[arg(long)]
    pub offset: u64,
    /// Append last time
    #[arg(long)]
    pub append_last_time: Option<String>,
    /// Version ID
    #[arg(long)]
    pub version_id: Option<String>,
    /// Content-Type header
    #[arg(long)]
    pub content_type: Option<String>,
    /// Content-MD5 header
    #[arg(long)]
    pub content_md5: Option<String>,
    /// x-content-sha256 header
    #[arg(long)]
    pub content_sha256: Option<String>,
    /// x-decoded-content-length header
    #[arg(long)]
    pub decoded_content_length: Option<u64>,
    /// Object lock mode
    #[arg(long)]
    pub object_lock_mode: Option<String>,
    /// Object lock retain-until date
    #[arg(long)]
    pub object_lock_retain_until_date: Option<String>,
    /// ACL value
    #[arg(long)]
    pub acl: Option<String>,
    /// Grant full control
    #[arg(long)]
    pub grant_full_control: Option<String>,
    /// Grant read
    #[arg(long)]
    pub grant_read: Option<String>,
    /// Grant read without list
    #[arg(long)]
    pub grant_read_non_list: Option<String>,
    /// Grant read ACP
    #[arg(long)]
    pub grant_read_acp: Option<String>,
    /// Grant write
    #[arg(long)]
    pub grant_write: Option<String>,
    /// Grant write ACP
    #[arg(long)]
    pub grant_write_acp: Option<String>,
    /// Persistent headers list
    #[arg(long)]
    pub persistent_headers: Option<String>,
    /// Traffic limit
    #[arg(long)]
    pub traffic_limit: Option<u64>,
    /// If-None-Match header
    #[arg(long)]
    pub if_none_match: Option<String>,
    /// If-Match header
    #[arg(long)]
    pub if_match: Option<String>,
}

#[derive(Debug, Args)]
pub struct ObjectSealAppendArgs {
    /// Object path (tos://bucket/key or --bucket + --key)
    pub uri: Option<String>,
    /// Bucket name
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key
    #[arg(long)]
    pub key: Option<String>,
    /// Append offset
    #[arg(long)]
    pub offset: Option<u64>,
    /// Version ID
    #[arg(long)]
    pub version_id: Option<String>,
    /// ACL value
    #[arg(long)]
    pub acl: Option<String>,
    /// Grant full control
    #[arg(long)]
    pub grant_full_control: Option<String>,
    /// Grant read
    #[arg(long)]
    pub grant_read: Option<String>,
    /// Grant read without list
    #[arg(long)]
    pub grant_read_non_list: Option<String>,
    /// Grant read ACP
    #[arg(long)]
    pub grant_read_acp: Option<String>,
    /// Grant write
    #[arg(long)]
    pub grant_write: Option<String>,
    /// Grant write ACP
    #[arg(long)]
    pub grant_write_acp: Option<String>,
    /// If-None-Match header
    #[arg(long)]
    pub if_none_match: Option<String>,
    /// If-Match header
    #[arg(long)]
    pub if_match: Option<String>,
}

#[derive(Debug, Args)]
pub struct ObjectModifyArgs {
    /// Object path (tos://bucket/key or --bucket + --key)
    pub uri: Option<String>,
    /// Bucket name
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key
    #[arg(long)]
    pub key: Option<String>,
    /// File or data to write
    #[arg(long)]
    pub body: String,
    /// Offset to write at
    #[arg(long)]
    pub offset: u64,
    /// Version ID
    #[arg(long)]
    pub version_id: Option<String>,
    /// Content-Type header
    #[arg(long)]
    pub content_type: Option<String>,
    /// Content-MD5 header
    #[arg(long)]
    pub content_md5: Option<String>,
    /// Traffic limit
    #[arg(long)]
    pub traffic_limit: Option<u64>,
    /// If-None-Match header
    #[arg(long)]
    pub if_none_match: Option<String>,
    /// If-Match header
    #[arg(long)]
    pub if_match: Option<String>,
}

#[derive(Debug, Args)]
pub struct ObjectRenameArgs {
    /// Source object path (tos://bucket/key)
    pub source: String,
    /// Destination object path in the same bucket (tos://bucket/key)
    pub destination: String,
    /// Recursive mkdir (x-recursive-mkdir)
    #[arg(long)]
    pub recursive_mkdir: bool,
    /// Do not update timestamp (x-not-update-timestamp)
    #[arg(long)]
    pub not_update_timestamp: bool,
    /// Forbid overwrite (x-forbid-overwrite)
    #[arg(long)]
    pub forbid_overwrite: bool,
    /// Trace ID (X-Tracer-Traceid)
    #[arg(long)]
    pub trace_id: Option<String>,
}

#[derive(Debug, Args)]
pub struct ObjectRestoreArgs {
    /// Object path (tos://bucket/key or --bucket + --key)
    pub uri: Option<String>,
    /// Bucket name
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key
    #[arg(long)]
    pub key: Option<String>,
    /// Restore days
    #[arg(long, default_value = "1")]
    pub days: u32,
    /// Version ID
    #[arg(long)]
    pub version_id: Option<String>,
    /// Content-MD5 header
    #[arg(long)]
    pub content_md5: Option<String>,
}

#[derive(Debug, Args)]
pub struct ObjectStatusArgs {
    /// Object path (tos://bucket/key or --bucket + --key)
    pub uri: Option<String>,
    /// Bucket name
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key
    #[arg(long)]
    pub key: Option<String>,
}

#[derive(Debug, Args)]
pub struct ObjectGetAclArgs {
    /// Object path (tos://bucket/key or --bucket + --key)
    pub uri: Option<String>,
    /// Bucket name
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key
    #[arg(long)]
    pub key: Option<String>,
    /// Version ID
    #[arg(long)]
    pub version_id: Option<String>,
}

#[derive(Debug, Args)]
pub struct ObjectSetAclArgs {
    /// Object path (tos://bucket/key or --bucket + --key)
    pub uri: Option<String>,
    /// Bucket name
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key
    #[arg(long)]
    pub key: Option<String>,
    /// ACL value (private, public-read, public-read-write, authenticated-read)
    #[arg(long)]
    pub acl: String,
    /// Version ID
    #[arg(long)]
    pub version_id: Option<String>,
}

#[derive(Debug, Args)]
pub struct ObjectGetTaggingArgs {
    /// Object path (tos://bucket/key or --bucket + --key)
    pub uri: Option<String>,
    /// Bucket name
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key
    #[arg(long)]
    pub key: Option<String>,
    /// Version ID
    #[arg(long)]
    pub version_id: Option<String>,
}

#[derive(Debug, Args)]
pub struct ObjectSetTaggingArgs {
    /// Object path (tos://bucket/key or --bucket + --key)
    pub uri: Option<String>,
    /// Bucket name
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key
    #[arg(long)]
    pub key: Option<String>,
    /// Tags (JSON or key1=val1&key2=val2)
    #[arg(long)]
    pub tags: String,
}

#[derive(Debug, Args)]
pub struct ObjectDeleteTaggingArgs {
    /// Object path (tos://bucket/key or --bucket + --key)
    pub uri: Option<String>,
    /// Bucket name
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key
    #[arg(long)]
    pub key: Option<String>,
    /// Confirm destructive delete before execution
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct ObjectLinkArgs {
    /// Object path (tos://bucket/key or --bucket + --key)
    pub uri: Option<String>,
    /// Bucket name
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key
    #[arg(long)]
    pub key: Option<String>,
    /// Source object key to link to
    #[arg(long)]
    pub source_key: String,
    /// ACL value
    #[arg(long)]
    pub acl: Option<String>,
    /// Grant full control
    #[arg(long)]
    pub grant_full_control: Option<String>,
    /// Grant read permission
    #[arg(long)]
    pub grant_read: Option<String>,
    /// Grant read without list permission
    #[arg(long)]
    pub grant_read_non_list: Option<String>,
    /// Grant read ACP permission
    #[arg(long)]
    pub grant_read_acp: Option<String>,
    /// Grant write permission
    #[arg(long)]
    pub grant_write: Option<String>,
    /// Grant write ACP permission
    #[arg(long)]
    pub grant_write_acp: Option<String>,
    /// Object tags (key1=value1&key2=value2)
    #[arg(long)]
    pub tagging: Option<String>,
}

#[derive(Debug, Args)]
pub struct ObjectGetSymlinkArgs {
    /// Object path (tos://bucket/key or --bucket + --key)
    pub uri: Option<String>,
    /// Bucket name
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key
    #[arg(long)]
    pub key: Option<String>,
    /// Version ID
    #[arg(long)]
    pub version_id: Option<String>,
}

#[derive(Debug, Args)]
pub struct ObjectCreateSymlinkArgs {
    /// Bucket name
    #[arg(long)]
    pub bucket: Option<String>,
    /// Symlink key
    #[arg(long)]
    pub key: Option<String>,
    /// Target object key
    #[arg(long)]
    pub target_key: String,
    /// Target bucket (if different)
    #[arg(long)]
    pub target_bucket: Option<String>,
    /// ACL value
    #[arg(long)]
    pub acl: Option<String>,
    /// Grant full control
    #[arg(long)]
    pub grant_full_control: Option<String>,
    /// Grant read permission
    #[arg(long)]
    pub grant_read: Option<String>,
    /// Grant read without list permission
    #[arg(long)]
    pub grant_read_non_list: Option<String>,
    /// Grant read ACP permission
    #[arg(long)]
    pub grant_read_acp: Option<String>,
    /// Grant write permission
    #[arg(long)]
    pub grant_write: Option<String>,
    /// Grant write ACP permission
    #[arg(long)]
    pub grant_write_acp: Option<String>,
    /// Object tags (key1=value1&key2=value2)
    #[arg(long)]
    pub tagging: Option<String>,
}

#[derive(Debug, Args)]
pub struct ObjectGetFetchTaskArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Task ID
    #[arg(long)]
    pub task_id: Option<String>,
}

#[derive(Debug, Args)]
pub struct ObjectCreateFetchTaskArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Object key
    #[arg(long)]
    pub key: String,
    /// Source URL to fetch from
    #[arg(long)]
    pub source_url: String,
    /// ETag pattern hint
    #[arg(long)]
    pub etag_pattern: Option<String>,
    /// Traffic limit in bps
    #[arg(long)]
    pub traffic_limit: Option<u64>,
    /// Unmodified-since condition
    #[arg(long)]
    pub if_unmodified_since: Option<String>,
    /// If-None-Match condition
    #[arg(long)]
    pub if_none_match: Option<String>,
    /// If-Match condition
    #[arg(long)]
    pub if_match: Option<String>,
    /// Object lock mode
    #[arg(long)]
    pub object_lock_mode: Option<String>,
    /// Object lock retain-until date
    #[arg(long)]
    pub object_lock_retain_until_date: Option<String>,
    /// ACL value
    #[arg(long)]
    pub acl: Option<String>,
    /// Grant full control
    #[arg(long)]
    pub grant_full_control: Option<String>,
    /// Grant read permission
    #[arg(long)]
    pub grant_read: Option<String>,
    /// Grant read without list permission
    #[arg(long)]
    pub grant_read_non_list: Option<String>,
    /// Grant read ACP permission
    #[arg(long)]
    pub grant_read_acp: Option<String>,
    /// Grant write permission
    #[arg(long)]
    pub grant_write: Option<String>,
    /// Grant write ACP permission
    #[arg(long)]
    pub grant_write_acp: Option<String>,
}

#[derive(Debug, Args)]
pub struct ObjectFetchArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Object key
    #[arg(long)]
    pub key: String,
    /// Source URL to fetch from
    #[arg(long)]
    pub source_url: String,
    /// Storage class
    #[arg(long)]
    pub storage_class: Option<String>,
    /// Custom metadata (key1=val1&key2=val2)
    #[arg(long)]
    pub meta: Option<String>,
}

#[derive(Debug, Args)]
pub struct ObjectSetRetentionArgs {
    /// Object path (tos://bucket/key or --bucket + --key)
    pub uri: Option<String>,
    /// Bucket name
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key
    #[arg(long)]
    pub key: Option<String>,
    /// Retention mode (COMPLIANCE)
    #[arg(long)]
    pub mode: String,
    /// Retain until date (RFC3339)
    #[arg(long)]
    pub retain_until_date: String,
    /// Version ID
    #[arg(long)]
    pub version_id: Option<String>,
    /// Content-MD5 header
    #[arg(long)]
    pub content_md5: Option<String>,
}

#[derive(Debug, Args)]
pub struct ObjectGetRetentionArgs {
    /// Object path (tos://bucket/key or --bucket + --key)
    pub uri: Option<String>,
    /// Bucket name
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key
    #[arg(long)]
    pub key: Option<String>,
    /// Version ID
    #[arg(long)]
    pub version_id: Option<String>,
}

// =============================================================================
// Core: Multipart (7 actions)
// =============================================================================

#[derive(Debug, Args)]
#[command(
    about = "Multipart core APIs",
    long_about = "Low-Level API — Core: multipart upload operations.",
    after_help = "Examples:\n  ve-tos-cli multipart create --bucket mybucket --key bigfile.bin\n  ve-tos-cli multipart upload --bucket mybucket --key bigfile.bin --upload-id xxx --part-number 1 --body file://part1\n  ve-tos-cli multipart complete --bucket mybucket --key bigfile.bin --upload-id xxx --complete-all\n  ve-tos-cli multipart list --bucket mybucket\n  ve-tos-cli multipart abort --bucket mybucket --key bigfile.bin --upload-id xxx --force"
)]
pub struct MultipartCommand {
    #[command(subcommand)]
    pub action: Option<MultipartAction>,
}

#[derive(Debug, Subcommand)]
pub enum MultipartAction {
    /// Create a multipart upload
    Create(MultipartCreateArgs),
    /// Upload a part
    Upload(MultipartUploadArgs),
    /// Complete a multipart upload
    Complete(MultipartCompleteArgs),
    /// Abort a multipart upload
    Abort(MultipartAbortArgs),
    /// Upload a part by copy
    Copy(MultipartCopyArgs),
    /// List uploaded parts
    #[command(name = "list-parts")]
    ListParts(MultipartListPartsArgs),
    /// List multipart uploads
    List(MultipartListArgs),
}

#[derive(Debug, Args)]
pub struct MultipartCreateArgs {
    /// Object path (tos://bucket/key or --bucket + --key)
    pub uri: Option<String>,
    /// Bucket name
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key
    #[arg(long)]
    pub key: Option<String>,
    /// Forbid overwrite existing object
    #[arg(long)]
    pub forbid_overwrite: bool,
    /// ETag pattern hint
    #[arg(long)]
    pub etag_pattern: Option<String>,
    /// ACL value
    #[arg(long)]
    pub acl: Option<String>,
    /// Grant full control
    #[arg(long)]
    pub grant_full_control: Option<String>,
    /// Grant read permission
    #[arg(long)]
    pub grant_read: Option<String>,
    /// Grant read without list permission
    #[arg(long)]
    pub grant_read_non_list: Option<String>,
    /// Grant read ACP permission
    #[arg(long)]
    pub grant_read_acp: Option<String>,
    /// Grant write permission
    #[arg(long)]
    pub grant_write: Option<String>,
    /// Grant write ACP permission
    #[arg(long)]
    pub grant_write_acp: Option<String>,
    /// Persistent headers list
    #[arg(long)]
    pub persistent_headers: Option<String>,
    /// Object lock mode
    #[arg(long)]
    pub object_lock_mode: Option<String>,
    /// Object lock retain-until date
    #[arg(long)]
    pub object_lock_retain_until_date: Option<String>,
    /// Unmodified-since condition
    #[arg(long)]
    pub if_unmodified_since: Option<String>,
    /// If-None-Match condition
    #[arg(long)]
    pub if_none_match: Option<String>,
    /// If-Match condition
    #[arg(long)]
    pub if_match: Option<String>,
    /// Object tags (x-tagging; key1=value1&key2=value2)
    #[arg(long)]
    pub tagging: Option<String>,
    /// Replicated-from (x-replicated-from)
    #[arg(long)]
    pub replicated_from: Option<String>,
    /// CRR source versionId (x-crr-source-versionId)
    #[arg(long)]
    pub crr_source_version_id: Option<String>,
    /// CRR source last modify time (x-crr-source-last-modify-time)
    #[arg(long)]
    pub crr_source_last_modify_time: Option<String>,
    /// CRR source timestamp nsec (x-crr-source-timestamp-nsec)
    #[arg(long)]
    pub crr_source_timestamp_nsec: Option<String>,
    /// CRR source bucket version status (x-crr-source-bucket-version-status)
    #[arg(long)]
    pub crr_source_bucket_version_status: Option<String>,
    /// CRR source uploadId (x-crr-source-uploadId)
    #[arg(long)]
    pub crr_source_upload_id: Option<String>,
    /// From modular marker (X-From-Modular)
    #[arg(long)]
    pub from_modular: Option<String>,
}

#[derive(Debug, Args)]
pub struct MultipartUploadArgs {
    /// Object path (tos://bucket/key or --bucket + --key)
    pub uri: Option<String>,
    /// Bucket name
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key
    #[arg(long)]
    pub key: Option<String>,
    /// Upload ID
    #[arg(long)]
    pub upload_id: String,
    /// Part number
    #[arg(long)]
    pub part_number: u32,
    /// Part body source
    #[arg(long)]
    pub body: String,
    /// Content-MD5 header
    #[arg(long)]
    pub content_md5: Option<String>,
    /// x-content-sha256 header
    #[arg(long)]
    pub content_sha256: Option<String>,
    /// CRC64 checksum
    #[arg(long)]
    pub hash_crc64ecma: Option<String>,
    /// Decoded content length
    #[arg(long)]
    pub decoded_content_length: Option<u64>,
    /// Traffic limit in bps
    #[arg(long)]
    pub traffic_limit: Option<u64>,
}

#[derive(Debug, Args)]
pub struct MultipartCompleteArgs {
    /// Object path (tos://bucket/key or --bucket + --key)
    pub uri: Option<String>,
    /// Bucket name
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key
    #[arg(long)]
    pub key: Option<String>,
    /// Upload ID
    #[arg(long)]
    pub upload_id: String,
    /// Completed parts JSON
    #[arg(long)]
    pub parts: String,
    /// Complete all parts server-side
    #[arg(long)]
    pub complete_all: bool,
    /// Unmodified-since condition
    #[arg(long)]
    pub if_unmodified_since: Option<String>,
    /// If-None-Match condition
    #[arg(long)]
    pub if_none_match: Option<String>,
    /// If-Match condition
    #[arg(long)]
    pub if_match: Option<String>,
    /// Server-side encryption algorithm (x-server-side-encryption)
    #[arg(long)]
    pub server_side_encryption: Option<String>,
    /// From modular marker (X-From-Modular)
    #[arg(long)]
    pub from_modular: Option<String>,
}

#[derive(Debug, Args)]
pub struct MultipartAbortArgs {
    /// Object path (tos://bucket/key or --bucket + --key)
    pub uri: Option<String>,
    /// Bucket name
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key
    #[arg(long)]
    pub key: Option<String>,
    /// Upload ID
    #[arg(long)]
    pub upload_id: String,
    /// Confirm destructive abort before execution
    #[arg(long)]
    pub force: bool,
    /// From modular marker (X-From-Modular)
    #[arg(long)]
    pub from_modular: Option<String>,
}

#[derive(Debug, Args)]
pub struct MultipartCopyArgs {
    /// Object path (tos://bucket/key or --bucket + --key)
    pub uri: Option<String>,
    /// Bucket name
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key
    #[arg(long)]
    pub key: Option<String>,
    /// Upload ID
    #[arg(long)]
    pub upload_id: String,
    /// Part number
    #[arg(long)]
    pub part_number: u32,
    /// Copy source (for example /src-bucket/src-key)
    #[arg(long)]
    pub copy_source: String,
    /// Source byte range
    #[arg(long)]
    pub copy_source_range: Option<String>,
    /// Source part number
    #[arg(long)]
    pub copy_source_part_number: Option<u32>,
    /// Source modified-since condition
    #[arg(long)]
    pub copy_source_if_modified_since: Option<String>,
    /// Source unmodified-since condition
    #[arg(long)]
    pub copy_source_if_unmodified_since: Option<String>,
    /// ETag pattern hint
    #[arg(long)]
    pub etag_pattern: Option<String>,
    /// Traffic limit in bps
    #[arg(long)]
    pub traffic_limit: Option<u64>,
}

#[derive(Debug, Args)]
pub struct MultipartListPartsArgs {
    /// Object path (tos://bucket/key or --bucket + --key)
    pub uri: Option<String>,
    /// Bucket name
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key
    #[arg(long)]
    pub key: Option<String>,
    /// Upload ID
    #[arg(long)]
    pub upload_id: String,
    /// Part number marker
    #[arg(long)]
    pub part_number_marker: Option<u32>,
    /// Maximum parts per response
    #[arg(long)]
    pub max_parts: Option<u32>,
    /// Fetch from KV (fetch-from-kv)
    #[arg(long)]
    pub fetch_from_kv: bool,
}

#[derive(Debug, Args)]
pub struct MultipartListArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Prefix filter
    #[arg(long)]
    pub prefix: Option<String>,
    /// Delimiter
    #[arg(long)]
    pub delimiter: Option<String>,
    /// Key marker
    #[arg(long)]
    pub key_marker: Option<String>,
    /// Upload ID marker
    #[arg(long)]
    pub upload_id_marker: Option<String>,
    /// Maximum uploads per response
    #[arg(long)]
    pub max_uploads: Option<u32>,
    /// Encoding type
    #[arg(long)]
    pub encoding_type: Option<String>,
    /// Fetch from KV (fetch-from-kv)
    #[arg(long)]
    pub fetch_from_kv: bool,
}

// =============================================================================
// Core: Turbo (4 actions) - NEW
// =============================================================================

#[derive(Debug, Args)]
#[command(
    about = "Turbo core APIs",
    long_about = "Low-Level API — Core: turbo accelerated write operations.",
    after_help = "Examples:\n  ve-tos-cli turbo open --bucket mybucket --key file.bin\n  ve-tos-cli turbo append --bucket mybucket --key file.bin --body file://part1 --turbo-token xxx\n  ve-tos-cli turbo list --bucket mybucket\n  ve-tos-cli turbo close --bucket mybucket --key file.bin --turbo-token xxx"
)]
pub struct TurboCommand {
    #[command(subcommand)]
    pub action: Option<TurboAction>,
}

#[derive(Debug, Subcommand)]
pub enum TurboAction {
    /// Open a turbo channel for an object
    Open(TurboOpenArgs),
    /// Append data to a turbo object
    Append(TurboAppendArgs),
    /// List turbo sessions
    List(TurboListArgs),
    /// Close a turbo channel
    Close(TurboCloseArgs),
}

#[derive(Debug, Args)]
pub struct TurboOpenArgs {
    /// Object path (tos://bucket/key or --bucket + --key)
    pub uri: Option<String>,
    /// Bucket name
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key
    #[arg(long)]
    pub key: Option<String>,
    /// Content type
    #[arg(long)]
    pub content_type: Option<String>,
    /// Content-MD5 header
    #[arg(long)]
    pub content_md5: Option<String>,
    /// CRC64 checksum
    #[arg(long)]
    pub hash_crc64ecma: Option<String>,
    /// Traffic limit in bps
    #[arg(long)]
    pub traffic_limit: Option<u64>,
    /// Guard object match condition
    #[arg(long)]
    pub if_match_guard_object: Option<String>,
    /// Open mode query value (0=create open, 1=write open)
    #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u8).range(0..=1))]
    pub mode: u8,
    /// ACL value
    #[arg(long)]
    pub acl: Option<String>,
    /// Grant full control
    #[arg(long)]
    pub grant_full_control: Option<String>,
    /// Grant read permission
    #[arg(long)]
    pub grant_read: Option<String>,
    /// Grant read without list permission
    #[arg(long)]
    pub grant_read_non_list: Option<String>,
    /// Grant read ACP permission
    #[arg(long)]
    pub grant_read_acp: Option<String>,
    /// Grant write permission
    #[arg(long)]
    pub grant_write: Option<String>,
    /// Grant write ACP permission
    #[arg(long)]
    pub grant_write_acp: Option<String>,
}

#[derive(Debug, Args)]
pub struct TurboAppendArgs {
    /// Object path (tos://bucket/key)
    pub uri: Option<String>,
    /// Bucket name
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key
    #[arg(long)]
    pub key: Option<String>,
    /// Data to append (file path or inline)
    #[arg(long)]
    pub body: String,
    /// Turbo token
    #[arg(long)]
    pub turbo_token: Option<String>,
    /// Content-MD5 header
    #[arg(long)]
    pub content_md5: Option<String>,
    /// CRC64 checksum
    #[arg(long)]
    pub hash_crc64ecma: Option<String>,
    /// Traffic limit in bps
    #[arg(long)]
    pub traffic_limit: Option<u64>,
    /// Guard object match condition
    #[arg(long)]
    pub if_match_guard_object: Option<String>,
    /// ACL value
    #[arg(long)]
    pub acl: Option<String>,
    /// Grant full control
    #[arg(long)]
    pub grant_full_control: Option<String>,
    /// Grant read permission
    #[arg(long)]
    pub grant_read: Option<String>,
    /// Grant read without list permission
    #[arg(long)]
    pub grant_read_non_list: Option<String>,
    /// Grant read ACP permission
    #[arg(long)]
    pub grant_read_acp: Option<String>,
    /// Grant write permission
    #[arg(long)]
    pub grant_write: Option<String>,
    /// Grant write ACP permission
    #[arg(long)]
    pub grant_write_acp: Option<String>,
}

#[derive(Debug, Args)]
pub struct TurboListArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Object key
    #[arg(long)]
    pub key: Option<String>,
    /// Marker for pagination
    #[arg(long)]
    pub marker: Option<String>,
    /// Maximum keys per response
    #[arg(long)]
    pub max_keys: Option<u32>,
    /// Prefix filter
    #[arg(long)]
    pub prefix: Option<String>,
    /// Encoding type
    #[arg(long)]
    pub encoding_type: Option<String>,
}

#[derive(Debug, Args)]
pub struct TurboCloseArgs {
    /// Object path (tos://bucket/key or --bucket + --key)
    pub uri: Option<String>,
    /// Bucket name
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key
    #[arg(long)]
    pub key: Option<String>,
    /// Traffic limit in bps
    #[arg(long)]
    pub traffic_limit: Option<u64>,
    /// Guard object match condition
    #[arg(long)]
    pub if_match_guard_object: Option<String>,
    /// Turbo token
    #[arg(long)]
    pub turbo_token: Option<String>,
    /// ACL value
    #[arg(long)]
    pub acl: Option<String>,
    /// Grant full control
    #[arg(long)]
    pub grant_full_control: Option<String>,
    /// Grant read permission
    #[arg(long)]
    pub grant_read: Option<String>,
    /// Grant read without list permission
    #[arg(long)]
    pub grant_read_non_list: Option<String>,
    /// Grant read ACP permission
    #[arg(long)]
    pub grant_read_acp: Option<String>,
    /// Grant write permission
    #[arg(long)]
    pub grant_write: Option<String>,
    /// Grant write ACP permission
    #[arg(long)]
    pub grant_write_acp: Option<String>,
}

// =============================================================================
// Bucket Config: Quota (2 actions: set, get)
// =============================================================================

#[derive(Debug, Args)]
pub struct QuotaCommand {
    #[command(subcommand)]
    pub action: Option<QuotaAction>,
}

#[derive(Debug, Subcommand)]
pub enum QuotaAction {
    /// Set bucket quota
    Set(QuotaSetArgs),
    /// Get bucket quota
    Get(BucketArg),
}

#[derive(Debug, Args)]
pub struct QuotaSetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Full quota request body JSON or file://path
    #[arg(long)]
    pub config: Option<String>,
    /// Bucket quota value in bytes
    #[arg(skip)]
    pub quota: Option<u64>,
}

// =============================================================================
// Bucket Config: Policy (3 actions: set, get, delete)
// =============================================================================

#[derive(Debug, Args)]
#[command(
    after_help = "Examples:\n  ve-tos-cli policy get --bucket mybucket\n  ve-tos-cli policy set --bucket mybucket --config file://policy.json\n  ve-tos-cli policy delete --bucket mybucket --force"
)]
pub struct PolicyCommand {
    #[command(subcommand)]
    pub action: Option<PolicyAction>,
}

#[derive(Debug, Subcommand)]
pub enum PolicyAction {
    /// Get bucket policy
    Get(PolicyGetArgs),
    /// Set bucket policy
    Set(PolicySetArgs),
    /// Delete bucket policy
    Delete(PolicyDeleteArgs),
}

#[derive(Debug, Args)]
pub struct PolicyGetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
}

#[derive(Debug, Args)]
pub struct PolicySetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Full request body JSON or file://path
    #[arg(long)]
    pub config: Option<String>,
    /// Policy JSON (inline or file://path)
    #[arg(skip)]
    pub policy: String,
}

#[derive(Debug, Args)]
pub struct PolicyDeleteArgs {
    /// Confirm destructive delete before execution
    #[arg(long)]
    pub force: bool,
    #[command(flatten)]
    pub bucket: BucketTarget,
}

// =============================================================================
// Bucket Config: Lifecycle (3 actions: set, get, delete)
// =============================================================================

#[derive(Debug, Args)]
#[command(
    after_help = "Examples:\n  ve-tos-cli lifecycle get --bucket mybucket\n  ve-tos-cli lifecycle set --bucket mybucket --config file://lifecycle.json\n  ve-tos-cli lifecycle delete --bucket mybucket --force"
)]
pub struct LifecycleCommand {
    #[command(subcommand)]
    pub action: Option<LifecycleAction>,
}

#[derive(Debug, Subcommand)]
pub enum LifecycleAction {
    /// Get lifecycle rules
    Get(LifecycleGetArgs),
    /// Set lifecycle rules
    Set(LifecycleSetArgs),
    /// Delete lifecycle rules
    Delete(LifecycleDeleteArgs),
}

#[derive(Debug, Args)]
pub struct LifecycleGetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
}

#[derive(Debug, Args)]
pub struct LifecycleSetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Full request body JSON or file://path
    #[arg(long)]
    pub config: Option<String>,
    /// Full lifecycle request body JSON or file://path
    #[arg(skip)]
    pub rules: Option<String>,
    /// Rule ID
    #[arg(skip)]
    pub id: Option<String>,
    /// Prefix match
    #[arg(skip)]
    pub prefix: Option<String>,
    /// Rule status (Enabled or Disabled)
    #[arg(skip)]
    pub status: Option<String>,
    /// Lifecycle tags (JSON or key1=value1&key2=value2)
    #[arg(skip)]
    pub tags: Option<String>,
    /// Lifecycle filter JSON (inline or file://path)
    #[arg(skip)]
    pub filter: Option<String>,
    /// Expiration JSON (inline or file://path)
    #[arg(skip)]
    pub expiration: Option<String>,
    /// NoncurrentVersionExpiration JSON (inline or file://path)
    #[arg(skip)]
    pub noncurrent_version_expiration: Option<String>,
    /// AbortIncompleteMultipartUpload JSON (inline or file://path)
    #[arg(skip)]
    pub abort_incomplete_multipart_upload: Option<String>,
    /// Transitions JSON array (inline or file://path)
    #[arg(skip)]
    pub transitions: Option<String>,
    /// NoncurrentVersionTransitions JSON array (inline or file://path)
    #[arg(skip)]
    pub noncurrent_version_transitions: Option<String>,
    /// AccessTimeTransitions JSON array (inline or file://path)
    #[arg(skip)]
    pub access_time_transitions: Option<String>,
    /// NoncurrentVersionAccessTimeTransitions JSON array (inline or file://path)
    #[arg(skip)]
    pub noncurrent_version_access_time_transitions: Option<String>,
}

#[derive(Debug, Args)]
pub struct LifecycleDeleteArgs {
    /// Confirm destructive delete before execution
    #[arg(long)]
    pub force: bool,
    #[command(flatten)]
    pub bucket: BucketTarget,
}

// =============================================================================
// Bucket Config: Storageclass (1 action: set)
// [Review Fix #M5] Internally standardized as `storageclass`. The historical
// typo `storgeclass` is preserved as a backward-compatible alias on the enum
// variant of `TosCommand` (see `cli/mod.rs`) so existing scripts keep working.
// =============================================================================

#[derive(Debug, Args)]
pub struct StorageclassCommand {
    #[command(subcommand)]
    pub action: Option<StorageclassAction>,
}

#[derive(Debug, Subcommand)]
pub enum StorageclassAction {
    /// Set default storage class for the bucket
    Set(StorageclassSetArgs),
}

#[derive(Debug, Args)]
pub struct StorageclassSetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Storage class. Allowed: STANDARD, IA, ARCHIVE_FR, INTELLIGENT_TIERING, COLD_ARCHIVE, ARCHIVE, DEEP_COLD_ARCHIVE
    #[arg(long)]
    pub storage_class: String,
}

// =============================================================================
// Bucket Config: CORS (3 actions: set, get, delete)
// =============================================================================

#[derive(Debug, Args)]
#[command(
    after_help = "Examples:\n  ve-tos-cli cors get --bucket mybucket\n  ve-tos-cli cors set --bucket mybucket --config file://cors.json\n  ve-tos-cli cors delete --bucket mybucket --force"
)]
pub struct CorsCommand {
    #[command(subcommand)]
    pub action: Option<CorsAction>,
}

#[derive(Debug, Subcommand)]
pub enum CorsAction {
    /// Get CORS configuration
    Get(CorsGetArgs),
    /// Set CORS configuration
    Set(CorsSetArgs),
    /// Delete CORS configuration
    Delete(CorsDeleteArgs),
}

#[derive(Debug, Args)]
pub struct CorsGetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
}

#[derive(Debug, Args)]
pub struct CorsSetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Full request body JSON or file://path
    #[arg(long)]
    pub config: Option<String>,
    /// Full CORS request body JSON or file://path
    #[arg(skip)]
    pub rules: Option<String>,
    /// AllowedOrigins (JSON array or comma-separated list)
    #[arg(skip)]
    pub allowed_origins: Option<String>,
    /// AllowedMethods (JSON array or comma-separated list)
    #[arg(skip)]
    pub allowed_methods: Option<String>,
    /// AllowedHeaders (JSON array or comma-separated list)
    #[arg(skip)]
    pub allowed_headers: Option<String>,
    /// ExposeHeaders (JSON array or comma-separated list)
    #[arg(skip)]
    pub expose_headers: Option<String>,
    /// MaxAgeSeconds / MaxAgeSeconds
    #[arg(skip)]
    pub max_age_seconds: Option<u64>,
    /// ResponseVary / ResponseVary
    #[arg(skip)]
    pub response_vary: Option<bool>,
    /// Optional Content-MD5 header
    #[arg(long)]
    pub content_md5: Option<String>,
}

#[derive(Debug, Args)]
pub struct CorsDeleteArgs {
    /// Confirm destructive delete before execution
    #[arg(long)]
    pub force: bool,
    #[command(flatten)]
    pub bucket: BucketTarget,
}

// =============================================================================
// Bucket Config: Versioning (2 actions: set, get)
// =============================================================================

#[derive(Debug, Args)]
#[command(
    after_help = "Examples:\n  ve-tos-cli versioning get --bucket mybucket\n  ve-tos-cli versioning set --bucket mybucket --config '{\"Status\":\"Enabled\"}'"
)]
pub struct VersioningCommand {
    #[command(subcommand)]
    pub action: Option<VersioningAction>,
}

#[derive(Debug, Subcommand)]
pub enum VersioningAction {
    /// Get versioning status
    Get(VersioningGetArgs),
    /// Set versioning status (Enabled/Suspended)
    Set(VersioningSetArgs),
}

#[derive(Debug, Args)]
pub struct VersioningGetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
}

#[derive(Debug, Args)]
pub struct VersioningSetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Full request body JSON or file://path
    #[arg(long)]
    pub config: Option<String>,
    /// Status: Enabled or Suspended
    #[arg(skip)]
    pub status: String,
}

// =============================================================================
// Bucket Config: Replication (3 actions: set, get, delete)
// =============================================================================

#[derive(Debug, Args)]
#[command(
    after_help = "Examples:\n  ve-tos-cli replication get --bucket mybucket\n  ve-tos-cli replication set --bucket mybucket --config file://replication.json\n  ve-tos-cli replication delete --bucket mybucket --rule-id rule-1 --force"
)]
pub struct ReplicationCommand {
    #[command(subcommand)]
    pub action: Option<ReplicationAction>,
}

#[derive(Debug, Subcommand)]
pub enum ReplicationAction {
    /// Get replication configuration
    Get(ReplicationGetArgs),
    /// Set replication configuration
    Set(ReplicationSetArgs),
    /// Delete replication configuration
    Delete(ReplicationDeleteArgs),
}

#[derive(Debug, Args)]
pub struct ReplicationGetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Specific rule ID
    #[arg(long)]
    pub rule_id: Option<String>,
}

#[derive(Debug, Args)]
pub struct ReplicationSetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Full request body JSON or file://path
    #[arg(long)]
    pub config: Option<String>,
    /// Full replication request body JSON or file://path
    #[arg(skip)]
    pub rules: Option<String>,
    /// Role / Role
    #[arg(skip)]
    pub role: Option<String>,
    /// Rule ID
    #[arg(skip)]
    pub id: Option<String>,
    /// Rule status
    #[arg(skip)]
    pub status: Option<String>,
    /// PrefixSet (JSON array or comma-separated list)
    #[arg(skip)]
    pub prefix_set: Option<String>,
    /// Replication tags (JSON or key1=value1&key2=value2)
    #[arg(skip)]
    pub tags: Option<String>,
    /// Destination bucket ARN
    #[arg(skip)]
    pub destination_bucket: Option<String>,
    /// Destination location
    #[arg(skip)]
    pub destination_location: Option<String>,
    /// Destination storage class
    #[arg(skip)]
    pub destination_storage_class: Option<String>,
    /// StorageClassInheritDirective / StorageClassInheritDirective
    #[arg(skip)]
    pub storage_class_inherit_directive: Option<String>,
    /// HistoricalObjectReplication / HistoricalObjectReplication
    #[arg(skip)]
    pub historical_object_replication: Option<String>,
    /// TransferType / TransferType
    #[arg(skip)]
    pub transfer_type: Option<String>,
    /// AccessControlTranslation.Owner / AccessControlTranslation.Owner
    #[arg(skip)]
    pub access_control_translation_owner: Option<String>,
}

#[derive(Debug, Args)]
pub struct ReplicationDeleteArgs {
    /// Confirm destructive delete before execution
    #[arg(long)]
    pub force: bool,
    #[command(flatten)]
    pub bucket: BucketTarget,
}

// =============================================================================
// Bucket Config: Encryption (3 actions: set, get, delete)
// =============================================================================

#[derive(Debug, Args)]
#[command(
    after_help = "Examples:\n  ve-tos-cli encryption get --bucket mybucket\n  ve-tos-cli encryption set --bucket mybucket --config file://encryption.json\n  ve-tos-cli encryption delete --bucket mybucket --force"
)]
pub struct EncryptionCommand {
    #[command(subcommand)]
    pub action: Option<EncryptionAction>,
}

#[derive(Debug, Subcommand)]
pub enum EncryptionAction {
    /// Get bucket encryption configuration
    Get(EncryptionGetArgs),
    /// Set bucket encryption configuration
    Set(EncryptionSetArgs),
    /// Delete bucket encryption configuration
    Delete(EncryptionDeleteArgs),
}

#[derive(Debug, Args)]
pub struct EncryptionGetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
}

#[derive(Debug, Args)]
pub struct EncryptionSetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Full request body JSON or file://path
    #[arg(long)]
    pub config: Option<String>,
    /// SSEAlgorithm (AES256 or KMS)
    #[arg(skip)]
    pub sse_algorithm: String,
    /// KMSDataEncryption / KMSDataEncryption
    #[arg(skip)]
    pub kms_data_encryption: Option<String>,
    /// KMSMasterKeyID (required when SSEAlgorithm is KMS)
    #[arg(skip)]
    pub kms_master_key_id: Option<String>,
    /// Content-MD5 header; auto-computed when omitted
    #[arg(long)]
    pub content_md5: Option<String>,
}

#[derive(Debug, Args)]
pub struct EncryptionDeleteArgs {
    /// Confirm destructive delete before execution
    #[arg(long)]
    pub force: bool,
    #[command(flatten)]
    pub bucket: BucketTarget,
}

// =============================================================================
// Bucket Config: Custom Domain (5 actions: set, delete, list, set-token, get-token)
// =============================================================================

#[derive(Debug, Args)]
pub struct CustomDomainCommand {
    #[command(subcommand)]
    pub action: Option<CustomDomainAction>,
}

#[derive(Debug, Subcommand)]
pub enum CustomDomainAction {
    /// Set custom domain binding
    Set(CustomDomainSetArgs),
    /// Delete custom domain binding
    Delete(CustomDomainDeleteArgs),
    /// List custom domain bindings
    List(CustomDomainListArgs),
    /// Set custom domain certificate token
    #[command(name = "set-token")]
    SetToken(CustomDomainSetTokenArgs),
    /// Get custom domain certificate token
    #[command(name = "get-token")]
    GetToken(CustomDomainGetTokenArgs),
}

#[derive(Debug, Args)]
pub struct CustomDomainSetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Full request body JSON or file://path
    #[arg(long)]
    pub config: Option<String>,
    /// Custom domain name
    #[arg(skip)]
    pub domain: String,
    /// Certificate ID
    #[arg(skip)]
    pub certificate_id: Option<String>,
    /// Certificate status
    #[arg(skip)]
    pub certificate_status: Option<String>,
    /// Whether the custom domain is forbidden
    #[arg(skip)]
    pub forbidden: Option<bool>,
    /// Forbidden reason
    #[arg(skip)]
    pub forbidden_reason: Option<String>,
    /// Domain CNAME target
    #[arg(skip)]
    pub cname: Option<String>,
    /// Authentication protocol
    #[arg(skip)]
    pub protocol: Option<String>,
    /// Content-MD5 header; auto-computed when omitted
    #[arg(long)]
    pub content_md5: Option<String>,
}

#[derive(Debug, Args)]
pub struct CustomDomainDeleteArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Custom domain name to remove
    #[arg(long)]
    pub domain: String,
    /// Confirm destructive delete before execution
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct CustomDomainListArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
}

#[derive(Debug, Args)]
pub struct CustomDomainSetTokenArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Full request body JSON or file://path
    #[arg(long)]
    pub config: Option<String>,
    /// Custom domain name
    #[arg(skip)]
    pub domain: String,
    /// Certificate token
    #[arg(skip)]
    pub token: String,
    /// Content-MD5 header; auto-computed when omitted
    #[arg(long)]
    pub content_md5: Option<String>,
}

#[derive(Debug, Args)]
pub struct CustomDomainGetTokenArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Custom domain name
    #[arg(long)]
    pub domain: String,
}

// =============================================================================
// Bucket Config: Notification (2 actions: set, get)
// =============================================================================

#[derive(Debug, Args)]
pub struct NotificationCommand {
    #[command(subcommand)]
    pub action: Option<NotificationAction>,
}

#[derive(Debug, Subcommand)]
pub enum NotificationAction {
    /// Get event notification configuration
    Get(NotificationGetArgs),
    /// Set event notification configuration
    Set(NotificationSetArgs),
}

#[derive(Debug, Args)]
pub struct NotificationGetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
}

#[derive(Debug, Args)]
pub struct NotificationSetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Full request body JSON or file://path
    #[arg(long)]
    pub config: Option<String>,
    /// Full notification_v2 Rules JSON array (inline or file://path)
    #[arg(skip)]
    pub rules: Option<String>,
    /// notification_v2 version
    #[arg(skip)]
    pub version: Option<String>,
    /// Notification rule ID
    #[arg(skip)]
    pub rule_id: Option<String>,
    /// Notification events (JSON array or comma-separated list)
    #[arg(skip)]
    pub events: Option<String>,
    /// Notification filter rules (JSON array or key=value,key2=value2)
    #[arg(skip)]
    pub filter_rules: Option<String>,
    /// Full VeFaaS destination array (inline or file://path)
    #[arg(skip)]
    pub destination_vefaas: Option<String>,
    /// VeFaaS function IDs (JSON array or comma-separated list)
    #[arg(skip)]
    pub vefaas_function_ids: Option<String>,
    /// Full Kafka destination array (inline or file://path)
    #[arg(skip)]
    pub destination_kafka: Option<String>,
    /// Kafka role
    #[arg(skip)]
    pub kafka_role: Option<String>,
    /// Kafka instance ID
    #[arg(skip)]
    pub kafka_instance_id: Option<String>,
    /// Kafka topic
    #[arg(skip)]
    pub kafka_topic: Option<String>,
    /// Kafka user
    #[arg(skip)]
    pub kafka_user: Option<String>,
    /// Kafka region
    #[arg(skip)]
    pub kafka_region: Option<String>,
    /// Full RocketMQ destination array (inline or file://path)
    #[arg(skip)]
    pub destination_rocketmq: Option<String>,
    /// RocketMQ role
    #[arg(skip)]
    pub rocketmq_role: Option<String>,
    /// RocketMQ instance ID
    #[arg(skip)]
    pub rocketmq_instance_id: Option<String>,
    /// RocketMQ topic
    #[arg(skip)]
    pub rocketmq_topic: Option<String>,
    /// RocketMQ access key ID
    #[arg(skip)]
    pub rocketmq_access_key_id: Option<String>,
    /// Content-MD5 header; auto-computed when omitted
    #[arg(long)]
    pub content_md5: Option<String>,
}

// =============================================================================
// Bucket Config: Website (3 actions: set, get, delete)
// =============================================================================

#[derive(Debug, Args)]
pub struct WebsiteCommand {
    #[command(subcommand)]
    pub action: Option<WebsiteAction>,
}

#[derive(Debug, Subcommand)]
pub enum WebsiteAction {
    /// Get static website configuration
    Get(WebsiteGetArgs),
    /// Set static website configuration
    Set(WebsiteSetArgs),
    /// Delete static website configuration
    Delete(WebsiteDeleteArgs),
}

#[derive(Debug, Args)]
pub struct WebsiteGetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
}

#[derive(Debug, Args)]
pub struct WebsiteSetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Full request body JSON or file://path
    #[arg(long)]
    pub config: Option<String>,
    /// Redirect all requests to the specified hostname
    #[arg(skip)]
    pub redirect_all_requests_to_host_name: Option<String>,
    /// Redirect-all protocol
    #[arg(skip)]
    pub redirect_all_requests_to_protocol: Option<String>,
    /// Index document suffix (e.g., index.html)
    #[arg(skip)]
    pub index_document_suffix: Option<String>,
    /// Whether to forbid sub-directory access for the index document
    #[arg(skip)]
    pub index_document_forbidden_sub_dir: Option<bool>,
    /// Error document key (e.g., error.html)
    #[arg(skip)]
    pub error_document_key: Option<String>,
    /// Full routing rules JSON array (inline or file://path)
    #[arg(skip)]
    pub routing_rules: Option<String>,
    /// Single routing rule condition: key prefix equals
    #[arg(skip)]
    pub routing_rule_key_prefix_equals: Option<String>,
    /// Single routing rule condition: HTTP error code equals
    #[arg(skip)]
    pub routing_rule_http_error_code_returned_equals: Option<u16>,
    /// Single routing rule redirect protocol
    #[arg(skip)]
    pub routing_rule_protocol: Option<String>,
    /// Single routing rule redirect hostname
    #[arg(skip)]
    pub routing_rule_host_name: Option<String>,
    /// Single routing rule redirect ReplaceKeyPrefixWith
    #[arg(skip)]
    pub routing_rule_replace_key_prefix_with: Option<String>,
    /// Single routing rule redirect ReplaceKeyWith
    #[arg(skip)]
    pub routing_rule_replace_key_with: Option<String>,
    /// Single routing rule redirect HTTP status code
    #[arg(skip)]
    pub routing_rule_http_redirect_code: Option<u16>,
    /// Content-MD5 header; auto-computed when omitted
    #[arg(long)]
    pub content_md5: Option<String>,
}

#[derive(Debug, Args)]
pub struct WebsiteDeleteArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Confirm destructive delete before execution
    #[arg(long)]
    pub force: bool,
}

// =============================================================================
// Bucket Config: Mirror (3 actions: set, get, delete)
// =============================================================================

#[derive(Debug, Args)]
pub struct MirrorCommand {
    #[command(subcommand)]
    pub action: Option<MirrorAction>,
}

#[derive(Debug, Subcommand)]
pub enum MirrorAction {
    /// Get mirror back-to-source rules
    Get(MirrorGetArgs),
    /// Set mirror back-to-source rules
    Set(MirrorSetArgs),
    /// Delete mirror back-to-source rules
    Delete(MirrorDeleteArgs),
}

#[derive(Debug, Args)]
pub struct MirrorGetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
}

#[derive(Debug, Args)]
pub struct MirrorSetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Full request body JSON or file://path
    #[arg(long)]
    pub config: Option<String>,
    /// Full mirror rules JSON array (inline or file://path)
    #[arg(skip)]
    pub rules: Option<String>,
    /// Mirror rule ID
    #[arg(skip)]
    pub id: Option<String>,
    /// Condition HTTP code
    #[arg(skip)]
    pub condition_http_code: Option<u16>,
    /// Condition key prefix
    #[arg(skip)]
    pub condition_key_prefix: Option<String>,
    /// Condition key suffix
    #[arg(skip)]
    pub condition_key_suffix: Option<String>,
    /// Condition allow-host list (JSON array or comma-separated list)
    #[arg(skip)]
    pub condition_allow_hosts: Option<String>,
    /// Condition HTTP method list (JSON array or comma-separated list)
    #[arg(skip)]
    pub condition_http_methods: Option<String>,
    /// Redirect type
    #[arg(skip)]
    pub redirect_type: Option<String>,
    /// Whether to fetch source on redirect
    #[arg(skip)]
    pub fetch_source_on_redirect: Option<bool>,
    /// Whether to pass query string
    #[arg(skip)]
    pub pass_query: Option<bool>,
    /// Whether to follow redirect
    #[arg(skip)]
    pub follow_redirect: Option<bool>,
    /// Mirror header pass-all flag
    #[arg(skip)]
    pub mirror_header_pass_all: Option<bool>,
    /// Mirror headers to pass (JSON array or comma-separated list)
    #[arg(skip)]
    pub mirror_header_pass: Option<String>,
    /// Mirror headers to remove (JSON array or comma-separated list)
    #[arg(skip)]
    pub mirror_header_remove: Option<String>,
    /// Mirror headers to set (JSON array or key=value,key2=value2)
    #[arg(skip)]
    pub mirror_header_set: Option<String>,
    /// Public source primary endpoints (JSON array or comma-separated list)
    #[arg(skip)]
    pub public_source_primary_endpoints: Option<String>,
    /// Public source follower endpoints (JSON array or comma-separated list)
    #[arg(skip)]
    pub public_source_follower_endpoints: Option<String>,
    /// Whether public source uses fixed endpoint
    #[arg(skip)]
    pub public_source_fixed_endpoint: Option<bool>,
    /// Transform with key prefix
    #[arg(skip)]
    pub transform_with_key_prefix: Option<String>,
    /// Transform with key suffix
    #[arg(skip)]
    pub transform_with_key_suffix: Option<String>,
    /// Transform replace key prefix
    #[arg(skip)]
    pub transform_replace_key_prefix: Option<String>,
    /// Transform replace key prefix with
    #[arg(skip)]
    pub transform_replace_key_prefix_with: Option<String>,
    /// FetchHeaderToMetaDataRules (JSON array or key=value,key2=value2)
    #[arg(skip)]
    pub fetch_header_to_metadata_rules: Option<String>,
    /// Private source primary endpoints (JSON array or comma-separated list)
    #[arg(skip)]
    pub private_source_primary_endpoints: Option<String>,
    /// Private source follower endpoints (JSON array or comma-separated list)
    #[arg(skip)]
    pub private_source_follower_endpoints: Option<String>,
    /// Private source bucket name
    #[arg(skip)]
    pub private_source_bucket_name: Option<String>,
    /// Private source role
    #[arg(skip)]
    pub private_source_role: Option<String>,
    /// Private source region
    #[arg(skip)]
    pub private_source_region: Option<String>,
    /// Private source storage vendor
    #[arg(skip)]
    pub private_source_storage_vendor: Option<String>,
    /// Private source access key
    #[arg(skip)]
    pub private_source_ak: Option<String>,
    /// Private source secret key
    #[arg(skip)]
    pub private_source_sk: Option<String>,
    /// Private source secret key encrypt type
    #[arg(skip)]
    pub private_source_sk_encrypt_type: Option<String>,
    /// Whether to fetch source on redirect with query
    #[arg(skip)]
    pub fetch_source_on_redirect_with_query: Option<bool>,
    /// Status codes to pass from source (JSON array or comma-separated list)
    #[arg(skip)]
    pub pass_status_code_from_source: Option<String>,
    /// Headers to pass from source (JSON array or comma-separated list)
    #[arg(skip)]
    pub pass_header_from_source: Option<String>,
    /// Content-MD5 header; auto-computed when omitted
    #[arg(long)]
    pub content_md5: Option<String>,
}

#[derive(Debug, Args)]
pub struct MirrorDeleteArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Confirm destructive delete before execution
    #[arg(long)]
    pub force: bool,
}

// =============================================================================
// Bucket Config: Inventory (4 actions: set, get, delete, list)
// =============================================================================

#[derive(Debug, Args)]
pub struct InventoryCommand {
    #[command(subcommand)]
    pub action: Option<InventoryAction>,
}

#[derive(Debug, Subcommand)]
pub enum InventoryAction {
    /// Get inventory configuration
    Get(InventoryGetArgs),
    /// Set inventory configuration
    Set(InventorySetArgs),
    /// Delete inventory configuration
    Delete(InventoryDeleteArgs),
    /// List inventory configurations
    List(InventoryListArgs),
}

#[derive(Debug, Args)]
pub struct InventoryGetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Inventory configuration ID
    #[arg(long)]
    pub id: String,
}

#[derive(Debug, Args)]
pub struct InventorySetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Full request body JSON or file://path
    #[arg(long)]
    pub config: Option<String>,
    /// Inventory configuration ID
    #[arg(long)]
    pub id: String,
    /// Whether this inventory configuration is enabled
    #[arg(skip)]
    pub is_enabled: Option<bool>,
    /// Inventory filter prefix
    #[arg(skip)]
    pub filter_prefix: Option<String>,
    /// Destination format
    #[arg(skip)]
    pub destination_format: Option<String>,
    /// Destination account ID
    #[arg(skip)]
    pub destination_account_id: Option<String>,
    /// Destination role
    #[arg(skip)]
    pub destination_role: Option<String>,
    /// Destination bucket
    #[arg(skip)]
    pub destination_bucket: Option<String>,
    /// Destination prefix
    #[arg(skip)]
    pub destination_prefix: Option<String>,
    /// Inventory schedule frequency
    #[arg(skip)]
    pub schedule_frequency: Option<String>,
    /// Included object versions
    #[arg(skip)]
    pub included_object_versions: Option<String>,
    /// Optional fields (JSON array or comma-separated list)
    #[arg(skip)]
    pub optional_fields: Option<String>,
    /// Whether the inventory file is uncompressed
    #[arg(skip)]
    pub is_uncompressed: Option<bool>,
    /// Content-MD5 header; auto-computed when omitted
    #[arg(long)]
    pub content_md5: Option<String>,
}

#[derive(Debug, Args)]
pub struct InventoryDeleteArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Inventory configuration ID
    #[arg(long)]
    pub id: String,
    /// Confirm destructive delete before execution
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct InventoryListArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Continuation token
    #[arg(long)]
    pub continuation_token: Option<String>,
}

// =============================================================================
// Bucket Config: Tagging (3 actions: set, get, delete) - bucket-only
// =============================================================================

#[derive(Debug, Args)]
#[command(
    after_help = "Examples:\n  ve-tos-cli tagging get --bucket mybucket\n  ve-tos-cli tagging set --bucket mybucket --config file://tags.json\n  ve-tos-cli tagging delete --bucket mybucket --force"
)]
pub struct TaggingCommand {
    #[command(subcommand)]
    pub action: Option<TaggingAction>,
}

#[derive(Debug, Subcommand)]
pub enum TaggingAction {
    /// Get bucket tagging
    Get(TaggingGetArgs),
    /// Set bucket tagging
    Set(TaggingSetArgs),
    /// Delete bucket tagging
    Delete(TaggingDeleteArgs),
}

#[derive(Debug, Args)]
pub struct TaggingGetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
}

#[derive(Debug, Args)]
pub struct TaggingSetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Full request body JSON or file://path
    #[arg(long)]
    pub config: Option<String>,
    /// Tags (JSON or key1=val1&key2=val2)
    #[arg(skip)]
    pub tags: String,
    /// Content-MD5 header; auto-computed when omitted
    #[arg(long)]
    pub content_md5: Option<String>,
}

#[derive(Debug, Args)]
pub struct TaggingDeleteArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Confirm destructive delete before execution
    #[arg(long)]
    pub force: bool,
}

// =============================================================================
// Bucket Config: ACL (2 actions: set, get) - bucket-only
// =============================================================================

#[derive(Debug, Args)]
#[command(
    after_help = "Examples:\n  ve-tos-cli acl get --bucket mybucket\n  ve-tos-cli acl set --bucket mybucket --acl public-read\n  ve-tos-cli object get-acl --bucket mybucket --key file.txt\n  ve-tos-cli object set-acl --bucket mybucket --key file.txt --acl private"
)]
pub struct AclCommand {
    #[command(subcommand)]
    pub action: Option<AclAction>,
}

#[derive(Debug, Subcommand)]
pub enum AclAction {
    /// Get bucket ACL
    Get(AclGetArgs),
    /// Set bucket ACL
    Set(AclSetArgs),
}

#[derive(Debug, Args)]
pub struct AclGetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
}

#[derive(Debug, Args)]
pub struct AclSetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Full request body JSON or file://path
    #[arg(long)]
    pub config: Option<String>,
    /// Canned ACL value (private, public-read, public-read-write, authenticated-read)
    #[arg(long)]
    pub acl: Option<String>,
    /// Grant full control permission header
    #[arg(long)]
    pub grant_full_control: Option<String>,
    /// Grant read permission header
    #[arg(long)]
    pub grant_read: Option<String>,
    /// Grant read without list permission header
    #[arg(long)]
    pub grant_read_non_list: Option<String>,
    /// Grant read ACP permission header
    #[arg(long)]
    pub grant_read_acp: Option<String>,
    /// Grant write permission header
    #[arg(long)]
    pub grant_write: Option<String>,
    /// Grant write ACP permission header
    #[arg(long)]
    pub grant_write_acp: Option<String>,
    /// Owner.ID in request body
    #[arg(skip)]
    pub owner_id: Option<String>,
    /// BucketAclDelivered in request body
    #[arg(skip)]
    pub bucket_acl_delivered: Option<bool>,
    /// Full Grants array JSON (inline or file://path)
    #[arg(skip)]
    pub grants: Option<String>,
    /// Grantee.Type for a single grant
    #[arg(skip)]
    pub grantee_type: Option<String>,
    /// Grantee.ID for a single grant
    #[arg(skip)]
    pub grantee_id: Option<String>,
    /// Grantee.Canned for a single grant
    #[arg(skip)]
    pub grantee_canned: Option<String>,
    /// Permission for a single grant
    #[arg(skip)]
    pub permission: Option<String>,
}

// =============================================================================
// Bucket Config: Rename (3 actions: set, get, delete)
// =============================================================================

#[derive(Debug, Args)]
pub struct RenameCommand {
    #[command(subcommand)]
    pub action: Option<RenameAction>,
}

#[derive(Debug, Subcommand)]
pub enum RenameAction {
    /// Get bucket rename configuration
    Get(RenameGetArgs),
    /// Set bucket rename configuration
    Set(RenameSetArgs),
    /// Delete bucket rename configuration
    Delete(RenameDeleteArgs),
}

#[derive(Debug, Args)]
pub struct RenameGetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
}

#[derive(Debug, Args)]
pub struct RenameSetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Full request body JSON or file://path
    #[arg(long)]
    pub config: Option<String>,
    /// Enable RenameObject for the bucket
    #[arg(skip)]
    pub enabled: bool,
}

#[derive(Debug, Args)]
pub struct RenameDeleteArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Confirm destructive delete before execution
    #[arg(long)]
    pub force: bool,
}

// =============================================================================
// Bucket Config: Real-Time Log (3 actions: set, get, delete)
// =============================================================================

#[derive(Debug, Args)]
pub struct RealTimeLogCommand {
    #[command(subcommand)]
    pub action: Option<RealTimeLogAction>,
}

#[derive(Debug, Subcommand)]
pub enum RealTimeLogAction {
    /// Get real-time log configuration
    Get(RealTimeLogGetArgs),
    /// Set real-time log configuration
    Set(RealTimeLogSetArgs),
    /// Delete real-time log configuration
    Delete(RealTimeLogDeleteArgs),
}

#[derive(Debug, Args)]
pub struct RealTimeLogGetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
}

#[derive(Debug, Args)]
pub struct RealTimeLogSetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Full request body JSON or file://path
    #[arg(long)]
    pub config: Option<String>,
    /// IAM role for log delivery
    #[arg(skip)]
    pub role: String,
    /// Whether to use service-managed TLS topic
    #[arg(skip)]
    pub use_service_topic: Option<bool>,
    /// TLS project ID
    #[arg(skip)]
    pub tls_project_id: Option<String>,
    /// TLS topic ID
    #[arg(skip)]
    pub tls_topic_id: Option<String>,
    /// Content-MD5 header; auto-computed when omitted
    #[arg(long)]
    pub content_md5: Option<String>,
}

#[derive(Debug, Args)]
pub struct RealTimeLogDeleteArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Confirm destructive delete before execution
    #[arg(long)]
    pub force: bool,
}

// =============================================================================
// Bucket Config: Access Monitor (2 actions: set, get)
// =============================================================================

#[derive(Debug, Args)]
pub struct AccessMonitorCommand {
    #[command(subcommand)]
    pub action: Option<AccessMonitorAction>,
}

#[derive(Debug, Subcommand)]
pub enum AccessMonitorAction {
    /// Get access monitor status
    Get(AccessMonitorGetArgs),
    /// Set access monitor status
    Set(AccessMonitorSetArgs),
}

#[derive(Debug, Args)]
pub struct AccessMonitorGetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
}

#[derive(Debug, Args)]
pub struct AccessMonitorSetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Full request body JSON or file://path
    #[arg(long)]
    pub config: Option<String>,
    /// Status (Enabled or Disabled)
    #[arg(skip)]
    pub status: String,
    /// Content-MD5 header; auto-computed when omitted
    #[arg(long)]
    pub content_md5: Option<String>,
}

// =============================================================================
// Bucket Config: WORM (2 actions: set, get)
// =============================================================================

#[derive(Debug, Args)]
pub struct WormCommand {
    #[command(subcommand)]
    pub action: Option<WormAction>,
}

#[derive(Debug, Subcommand)]
pub enum WormAction {
    /// Get WORM (object lock) configuration
    Get(WormGetArgs),
    /// Set WORM (object lock) configuration
    Set(WormSetArgs),
}

#[derive(Debug, Args)]
pub struct WormGetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
}

#[derive(Debug, Args)]
pub struct WormSetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Full request body JSON or file://path
    #[arg(long)]
    pub config: Option<String>,
    /// Object lock enabled status
    #[arg(skip)]
    pub object_lock_enabled: Option<String>,
    /// Default retention mode (COMPLIANCE or GOVERNANCE)
    #[arg(skip)]
    pub default_retention_mode: Option<String>,
    /// Default retention period in days
    #[arg(skip)]
    pub default_retention_days: Option<u32>,
    /// Default retention period in years
    #[arg(skip)]
    pub default_retention_years: Option<u32>,
    /// Content-MD5 header; auto-computed when omitted
    #[arg(long)]
    pub content_md5: Option<String>,
}

// =============================================================================
// Bucket Config: Trash (2 actions: set, get)
// =============================================================================

#[derive(Debug, Args)]
pub struct TrashCommand {
    #[command(subcommand)]
    pub action: Option<TrashAction>,
}

#[derive(Debug, Subcommand)]
pub enum TrashAction {
    /// Get trash (recycle bin) configuration
    Get(TrashGetArgs),
    /// Set trash (recycle bin) configuration
    Set(TrashSetArgs),
}

#[derive(Debug, Args)]
pub struct TrashGetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
}

#[derive(Debug, Args)]
pub struct TrashSetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Full trash configuration JSON or file://path
    #[arg(long)]
    pub config: Option<String>,
    /// Trash status (Enabled or Disabled)
    #[arg(skip)]
    pub status: Option<String>,
    /// Retention days for objects moved to trash
    #[arg(skip)]
    pub days: Option<u32>,
    /// Content-MD5 header; auto-computed when omitted
    #[arg(long)]
    pub content_md5: Option<String>,
}

// =============================================================================
// Bucket Config: Payment (2 actions: set, get)
// =============================================================================

#[derive(Debug, Args)]
pub struct PaymentCommand {
    #[command(subcommand)]
    pub action: Option<PaymentAction>,
}

#[derive(Debug, Subcommand)]
pub enum PaymentAction {
    /// Get payment (requester pays) configuration
    Get(PaymentGetArgs),
    /// Set payment (requester pays) configuration
    Set(PaymentSetArgs),
}

#[derive(Debug, Args)]
pub struct PaymentGetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
}

#[derive(Debug, Args)]
pub struct PaymentSetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Full request body JSON or file://path
    #[arg(long)]
    pub config: Option<String>,
    /// Payer (BucketOwner or Requester)
    #[arg(skip)]
    pub payer: String,
}

// =============================================================================
// Bucket Config: Logging (2 actions: set, get)
// =============================================================================

#[derive(Debug, Args)]
pub struct LoggingCommand {
    #[command(subcommand)]
    pub action: Option<LoggingAction>,
}

#[derive(Debug, Subcommand)]
pub enum LoggingAction {
    /// Get bucket logging configuration
    Get(LoggingGetArgs),
    /// Set bucket logging configuration
    Set(LoggingSetArgs),
}

#[derive(Debug, Args)]
pub struct LoggingGetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
}

#[derive(Debug, Args)]
pub struct LoggingSetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Full request body JSON or file://path
    #[arg(long)]
    pub config: Option<String>,
    /// Target bucket for log delivery; omit together with --target-prefix to disable logging
    #[arg(skip)]
    pub target_bucket: Option<String>,
    /// Target prefix for log objects; omit together with --target-bucket to disable logging
    #[arg(skip)]
    pub target_prefix: Option<String>,
    /// Content-MD5 header; auto-computed when omitted
    #[arg(long)]
    pub content_md5: Option<String>,
}

// =============================================================================
// Bucket Config: Intelligent Tiering (2 actions: set, get)
// =============================================================================

#[derive(Debug, Args)]
pub struct IntelligentTieringCommand {
    #[command(subcommand)]
    pub action: Option<IntelligentTieringAction>,
}

#[derive(Debug, Subcommand)]
pub enum IntelligentTieringAction {
    /// Get intelligent tiering configuration
    Get(IntelligentTieringGetArgs),
    /// Set intelligent tiering configuration
    Set(IntelligentTieringSetArgs),
}

#[derive(Debug, Args)]
pub struct IntelligentTieringGetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
}

#[derive(Debug, Args)]
pub struct IntelligentTieringSetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Full intelligent tiering configuration JSON or file://path
    #[arg(long)]
    pub config: Option<String>,
    /// Intelligent tiering status (Enabled or Disabled)
    #[arg(skip)]
    pub status: Option<String>,
    /// Access tier name in Tiering rule
    #[arg(skip)]
    pub access_tier: Option<String>,
    /// Days before transitioning to the access tier
    #[arg(skip)]
    pub days: Option<u32>,
    /// Content-MD5 header; auto-computed when omitted
    #[arg(long)]
    pub content_md5: Option<String>,
}

// =============================================================================
// Bucket Config: Transfer Acceleration (2 actions: set, get)
// =============================================================================

#[derive(Debug, Args)]
pub struct TransferAccelerationCommand {
    #[command(subcommand)]
    pub action: Option<TransferAccelerationAction>,
}

#[derive(Debug, Subcommand)]
pub enum TransferAccelerationAction {
    /// Get transfer acceleration status
    Get(TransferAccelerationGetArgs),
    /// Set transfer acceleration status
    Set(TransferAccelerationSetArgs),
}

#[derive(Debug, Args)]
pub struct TransferAccelerationGetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
}

#[derive(Debug, Args)]
pub struct TransferAccelerationSetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Full request body JSON or file://path
    #[arg(long)]
    pub config: Option<String>,
    /// Enable or disable transfer acceleration
    #[arg(skip)]
    pub enabled: Option<bool>,
    /// Transfer acceleration status (Enabled or Suspended)
    #[arg(skip)]
    pub status: Option<String>,
    /// Content-MD5 header; auto-computed when omitted
    #[arg(long)]
    pub content_md5: Option<String>,
}

// =============================================================================
// Bucket Config: CDN Notification (3 actions: set, get, delete)
// =============================================================================

#[derive(Debug, Args)]
pub struct CdnNotificationCommand {
    #[command(subcommand)]
    pub action: Option<CdnNotificationAction>,
}

#[derive(Debug, Subcommand)]
pub enum CdnNotificationAction {
    /// Get CDN notification configuration
    Get(CdnNotificationGetArgs),
    /// Set CDN notification configuration
    Set(CdnNotificationSetArgs),
    /// Delete CDN notification configuration
    Delete(CdnNotificationDeleteArgs),
}

#[derive(Debug, Args)]
pub struct CdnNotificationGetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
}

#[derive(Debug, Args)]
pub struct CdnNotificationSetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Full CDN notification configuration JSON or file://path
    #[arg(long)]
    pub config: Option<String>,
    /// Notification events (comma-separated or JSON array)
    #[arg(skip)]
    pub events: Option<String>,
    /// Filter rules (key=value pairs or JSON array)
    #[arg(skip)]
    pub filter_rules: Option<String>,
    /// CDN notification role
    #[arg(skip)]
    pub role: Option<String>,
    /// Notification endpoint URL
    #[arg(skip)]
    pub endpoint: Option<String>,
    /// Content-MD5 header; auto-computed when omitted
    #[arg(long)]
    pub content_md5: Option<String>,
}

#[derive(Debug, Args)]
pub struct CdnNotificationDeleteArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Force deletion before execution
    #[arg(long)]
    pub force: bool,
}

// =============================================================================
// Bucket Config: HTTPS Config (2 actions: set, get)
// =============================================================================

#[derive(Debug, Args)]
pub struct HttpsConfigCommand {
    #[command(subcommand)]
    pub action: Option<HttpsConfigAction>,
}

#[derive(Debug, Subcommand)]
pub enum HttpsConfigAction {
    /// Get HTTPS / TLS version configuration
    Get(HttpsConfigGetArgs),
    /// Set HTTPS / TLS version configuration
    Set(HttpsConfigSetArgs),
}

#[derive(Debug, Args)]
pub struct HttpsConfigGetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
}

#[derive(Debug, Args)]
pub struct HttpsConfigSetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Full request body JSON or file://path
    #[arg(long)]
    pub config: Option<String>,
    /// Whether to enable TLS version configuration
    #[arg(skip)]
    pub enable: Option<bool>,
    /// Minimum TLS version (TLSv1.0, TLSv1.1, TLSv1.2, TLSv1.3)
    #[arg(skip)]
    pub min_tls_version: Option<String>,
    /// Maximum TLS version (TLSv1.0, TLSv1.1, TLSv1.2, TLSv1.3)
    #[arg(skip)]
    pub max_tls_version: Option<String>,
}

// =============================================================================
// Bucket Config: Pay-By-Traffic (2 actions: set, get)
// =============================================================================

#[derive(Debug, Args)]
pub struct PayByTrafficCommand {
    #[command(subcommand)]
    pub action: Option<PayByTrafficAction>,
}

#[derive(Debug, Subcommand)]
pub enum PayByTrafficAction {
    /// Get pay-by-traffic configuration
    Get(PayByTrafficGetArgs),
    /// Set pay-by-traffic configuration
    Set(PayByTrafficSetArgs),
}

#[derive(Debug, Args)]
pub struct PayByTrafficGetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
}

#[derive(Debug, Args)]
pub struct PayByTrafficSetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Full request body JSON or file://path
    #[arg(long)]
    pub config: Option<String>,
    /// Pay-by-traffic status (Enabled or Disabled)
    #[arg(skip)]
    pub status: String,
    /// Content-MD5 header; auto-computed when omitted
    #[arg(long)]
    pub content_md5: Option<String>,
}

// =============================================================================
// Bucket Config: Max-Age (3 actions: set, get, delete)
// =============================================================================

#[derive(Debug, Args)]
pub struct MaxAgeCommand {
    #[command(subcommand)]
    pub action: Option<MaxAgeAction>,
}

#[derive(Debug, Subcommand)]
pub enum MaxAgeAction {
    /// Get max-age configuration
    Get(MaxAgeGetArgs),
    /// Set max-age configuration
    Set(MaxAgeSetArgs),
    /// Delete max-age configuration
    Delete(MaxAgeDeleteArgs),
}

#[derive(Debug, Args)]
pub struct MaxAgeGetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
}

#[derive(Debug, Args)]
pub struct MaxAgeSetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Full request body JSON or file://path
    #[arg(long)]
    pub config: Option<String>,
    /// Max-Age cache seconds
    #[arg(skip)]
    pub max_age_seconds: u32,
    /// Content-MD5 header; auto-computed when omitted
    #[arg(long)]
    pub content_md5: Option<String>,
}

#[derive(Debug, Args)]
pub struct MaxAgeDeleteArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Force deletion before execution
    #[arg(long)]
    pub force: bool,
}

// =============================================================================
// Bucket Config: Redundancy Transition (5 actions)
// =============================================================================

#[derive(Debug, Args)]
pub struct RedundancyTransitionCommand {
    #[command(subcommand)]
    pub action: Option<RedundancyTransitionAction>,
}

#[derive(Debug, Subcommand)]
pub enum RedundancyTransitionAction {
    /// Create a redundancy transition task
    Create(RedundancyTransitionCreateArgs),
    /// Delete a redundancy transition task
    Delete(RedundancyTransitionDeleteArgs),
    /// Get a redundancy transition task
    Get(RedundancyTransitionGetArgs),
    /// List redundancy transition tasks
    List(RedundancyTransitionListArgs),
    /// Get remaining time for a redundancy transition task
    #[command(name = "get-remaining-time")]
    GetRemainingTime(RedundancyTransitionGetRemainingTimeArgs),
}

#[derive(Debug, Args)]
pub struct RedundancyTransitionCreateArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Configuration (JSON or file://path)
    #[arg(long)]
    pub config: Option<String>,
    /// Target redundancy type
    #[arg(skip)]
    pub target_redundancy: Option<String>,
    /// Object prefix scope
    #[arg(skip)]
    pub prefix: Option<String>,
    /// Source storage class filter
    #[arg(skip)]
    pub storage_class: Option<String>,
    /// Content-MD5 header; auto-computed when omitted
    #[arg(long)]
    pub content_md5: Option<String>,
}

#[derive(Debug, Args)]
pub struct RedundancyTransitionDeleteArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Redundancy transition task ID
    #[arg(long, alias = "x-tos-redundancy-transition-taskid")]
    pub task_id: Option<String>,
    /// Force deletion before execution
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct RedundancyTransitionGetArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Redundancy transition task ID
    #[arg(long, alias = "x-tos-redundancy-transition-taskid")]
    pub task_id: String,
}

#[derive(Debug, Args)]
pub struct RedundancyTransitionListArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Continuation token for listing tasks
    #[arg(long)]
    pub continuation_token: Option<String>,
}

#[derive(Debug, Args)]
pub struct RedundancyTransitionGetRemainingTimeArgs {
    #[command(flatten)]
    pub bucket: BucketTarget,
    /// Redundancy transition task ID
    #[arg(long, alias = "x-tos-redundancy-transition-taskid")]
    pub task_id: Option<String>,
}

// =============================================================================
// Advanced: Data Process (35 actions)
// =============================================================================

#[derive(Debug, Args)]
pub struct DataProcessCommand {
    #[command(subcommand)]
    pub action: Option<DataProcessAction>,
}

#[derive(Debug, Subcommand)]
pub enum DataProcessAction {
    /// Delete an image style
    #[command(name = "delete-image-style")]
    DeleteImageStyle(GenericArgs),
    /// Get an image style
    #[command(name = "get-image-style")]
    GetImageStyle(GenericArgs),
    /// List all image styles
    #[command(name = "list-image-styles")]
    ListImageStyles(GenericArgs),
    /// List image style brief infos
    #[command(name = "list-image-style-brief-infos")]
    ListImageStyleBriefInfos(GenericArgs),
    /// List image style contents
    #[command(name = "list-image-style-contents")]
    ListImageStyleContents(GenericArgs),
    /// Set an image style
    #[command(name = "set-image-style")]
    SetImageStyle(GenericArgs),
    /// Set image protect rule
    #[command(name = "set-image-protect-rule")]
    SetImageProtectRule(GenericArgs),
    /// Get image protect rule
    #[command(name = "get-image-protect-rule")]
    GetImageProtectRule(GenericArgs),
    /// Set image style separator
    #[command(name = "set-image-style-separator")]
    SetImageStyleSeparator(GenericArgs),
    /// Get image style separator
    #[command(name = "get-image-style-separator")]
    GetImageStyleSeparator(GenericArgs),
    /// Set private M3U8 rule
    #[command(name = "set-private-m3u8-rule")]
    SetPrivateM3u8Rule(GenericArgs),
    /// Get private M3U8 rule
    #[command(name = "get-private-m3u8-rule")]
    GetPrivateM3u8Rule(GenericArgs),
    /// Set blind watermark rule
    #[command(name = "set-blind-watermark-rule")]
    SetBlindWatermarkRule(GenericArgs),
    /// Get blind watermark rule
    #[command(name = "get-blind-watermark-rule")]
    GetBlindWatermarkRule(GenericArgs),
    /// Delete a workflow
    #[command(name = "delete-workflow")]
    DeleteWorkflow(GenericArgs),
    /// Get a workflow
    #[command(name = "get-workflow")]
    GetWorkflow(GenericArgs),
    /// Set a workflow
    #[command(name = "set-workflow")]
    SetWorkflow(GenericArgs),
    /// Get a workflow execution
    #[command(name = "get-workflow-execution")]
    GetWorkflowExecution(GenericArgs),
    /// List workflow executions
    #[command(name = "list-workflow-executions")]
    ListWorkflowExecutions(GenericArgs),
    /// Delete a template
    #[command(name = "delete-template")]
    DeleteTemplate(GenericArgs),
    /// Get a template
    #[command(name = "get-template")]
    GetTemplate(GenericArgs),
    /// Set a template
    #[command(name = "set-template")]
    SetTemplate(GenericArgs),
    /// Create an audit job
    #[command(name = "create-audit-job")]
    CreateAuditJob(GenericArgs),
    /// Create a document processing job
    #[command(name = "create-doc-job")]
    CreateDocJob(GenericArgs),
    /// Create a file processing job
    #[command(name = "create-file-job")]
    CreateFileJob(GenericArgs),
    /// Create a media processing job
    #[command(name = "create-media-job")]
    CreateMediaJob(GenericArgs),
    /// Get a job
    #[command(name = "get-job")]
    GetJob(GenericArgs),
    /// List jobs
    #[command(name = "list-jobs")]
    ListJobs(GenericArgs),
    /// Delete an increment audit configuration
    #[command(name = "delete-increment-audit")]
    DeleteIncrementAudit(GenericArgs),
    /// Get an audit configuration
    #[command(name = "get-audit")]
    GetAudit(GenericArgs),
    /// Get an increment audit configuration
    #[command(name = "get-increment-audit")]
    GetIncrementAudit(GenericArgs),
    /// List audit configurations
    #[command(name = "list-audits")]
    ListAudits(GenericArgs),
    /// List increment audit configurations
    #[command(name = "list-increment-audits")]
    ListIncrementAudits(GenericArgs),
    /// Create an increment audit configuration
    #[command(name = "create-increment-audit")]
    CreateIncrementAudit(GenericArgs),
    /// Create an audit configuration
    #[command(name = "create-audit")]
    CreateAudit(GenericArgs),
}

// =============================================================================
// Advanced: Object Set (21 actions)
// =============================================================================

#[derive(Debug, Args)]
pub struct ObjectSetCommand {
    #[command(subcommand)]
    pub action: Option<ObjectSetAction>,
}

#[derive(Debug, Subcommand)]
pub enum ObjectSetAction {
    /// Delete an object set
    Delete(GenericArgs),
    /// Delete object set lifecycle configuration
    #[command(name = "delete-lifecycle")]
    DeleteLifecycle(GenericArgs),
    /// Delete object set lifecycle by tag
    #[command(name = "delete-lifecycle-by-tag")]
    DeleteLifecycleByTag(GenericArgs),
    /// Delete object set quota by tag
    #[command(name = "delete-quota-by-tag")]
    DeleteQuotaByTag(GenericArgs),
    /// Get global object set configuration
    #[command(name = "get-global")]
    GetGlobal(GenericArgs),
    /// Get an object set
    Get(GenericArgs),
    /// Get object set endpoint
    #[command(name = "get-endpoint")]
    GetEndpoint(GenericArgs),
    /// Get object set lifecycle configuration
    #[command(name = "get-lifecycle")]
    GetLifecycle(GenericArgs),
    /// Get object set lifecycle by tag
    #[command(name = "get-lifecycle-by-tag")]
    GetLifecycleByTag(GenericArgs),
    /// Get object set quota
    #[command(name = "get-quota")]
    GetQuota(GenericArgs),
    /// Get object set quota by tag
    #[command(name = "get-quota-by-tag")]
    GetQuotaByTag(GenericArgs),
    /// Get object set storage info
    #[command(name = "get-storage")]
    GetStorage(GenericArgs),
    /// Get object set tagging
    #[command(name = "get-tagging")]
    GetTagging(GenericArgs),
    /// List object sets
    List(GenericArgs),
    /// Set global object set configuration
    #[command(name = "set-global")]
    SetGlobal(GenericArgs),
    /// Set an object set
    Set(GenericArgs),
    /// Set object set lifecycle configuration
    #[command(name = "set-lifecycle")]
    SetLifecycle(GenericArgs),
    /// Set object set lifecycle by tag
    #[command(name = "set-lifecycle-by-tag")]
    SetLifecycleByTag(GenericArgs),
    /// Set object set quota
    #[command(name = "set-quota")]
    SetQuota(GenericArgs),
    /// Set object set quota by tag
    #[command(name = "set-quota-by-tag")]
    SetQuotaByTag(GenericArgs),
    /// Set object set tagging
    #[command(name = "set-tagging")]
    SetTagging(GenericArgs),
}

// =============================================================================
// Advanced: Accelerator (21 actions)
// =============================================================================

#[derive(Debug, Args)]
pub struct AcceleratorCommand {
    #[command(subcommand)]
    pub action: Option<AcceleratorAction>,
}

#[derive(Debug, Subcommand)]
pub enum AcceleratorAction {
    /// Delete an accelerator
    Delete(GenericArgs),
    /// Delete an evict job
    #[command(name = "delete-evict-job")]
    DeleteEvictJob(GenericArgs),
    /// Delete a prefetch job
    #[command(name = "delete-prefetch-job")]
    DeletePrefetchJob(GenericArgs),
    /// Unbind a bucket from an accelerator
    #[command(name = "unbind-bucket")]
    UnbindBucket(GenericArgs),
    /// Get accelerator details
    Get(GenericArgs),
    /// Get an evict job
    #[command(name = "get-evict-job")]
    GetEvictJob(GenericArgs),
    /// Get a prefetch job
    #[command(name = "get-prefetch-job")]
    GetPrefetchJob(GenericArgs),
    /// Get accelerator bandwidth
    #[command(name = "get-bandwidth")]
    GetBandwidth(GenericArgs),
    /// Get accelerator capacity
    #[command(name = "get-capacity")]
    GetCapacity(GenericArgs),
    /// List accelerators
    List(GenericArgs),
    /// List evict jobs
    #[command(name = "list-evict-jobs")]
    ListEvictJobs(GenericArgs),
    /// List prefetch jobs
    #[command(name = "list-prefetch-jobs")]
    ListPrefetchJobs(GenericArgs),
    /// List prefetch records
    #[command(name = "list-prefetch-records")]
    ListPrefetchRecords(GenericArgs),
    /// List availability zones
    #[command(name = "list-az")]
    ListAz(GenericArgs),
    /// List accelerators for a bucket
    #[command(name = "list-for-bucket")]
    ListForBucket(GenericArgs),
    /// List bound access points
    #[command(name = "list-binded-aps")]
    ListBindedAps(GenericArgs),
    /// List bound buckets
    #[command(name = "list-binded-buckets")]
    ListBindedBuckets(GenericArgs),
    /// Create an accelerator
    Create(GenericArgs),
    /// Create an evict job
    #[command(name = "create-evict-job")]
    CreateEvictJob(GenericArgs),
    /// Create a prefetch job
    #[command(name = "create-prefetch-job")]
    CreatePrefetchJob(GenericArgs),
    /// Bind a bucket to an accelerator
    #[command(name = "bind-bucket")]
    BindBucket(GenericArgs),
}

// =============================================================================
// Advanced: MRAP (16 actions)
// =============================================================================

#[derive(Debug, Args)]
pub struct MrapCommand {
    #[command(subcommand)]
    pub action: Option<MrapAction>,
}

#[derive(Debug, Subcommand)]
pub enum MrapAction {
    /// Delete an MRAP
    Delete(GenericArgs),
    /// Delete MRAP mirror configuration
    #[command(name = "delete-mirror")]
    DeleteMirror(GenericArgs),
    /// Delete MRAP policy
    #[command(name = "delete-policy")]
    DeletePolicy(GenericArgs),
    /// Unbind accelerator from MRAP
    #[command(name = "unbind-accelerator")]
    UnbindAccelerator(GenericArgs),
    /// Bind accelerator to MRAP
    #[command(name = "bind-accelerator")]
    BindAccelerator(GenericArgs),
    /// Get MRAP details
    Get(GenericArgs),
    /// Get MRAP mirror configuration
    #[command(name = "get-mirror")]
    GetMirror(GenericArgs),
    /// Get MRAP policy
    #[command(name = "get-policy")]
    GetPolicy(GenericArgs),
    /// Get MRAP routes
    #[command(name = "get-routes")]
    GetRoutes(GenericArgs),
    /// List accelerators for MRAP
    #[command(name = "list-accelerators")]
    ListAccelerators(GenericArgs),
    /// List MRAPs for an accelerator
    #[command(name = "list-mraps-for-accelerator")]
    ListMrapsForAccelerator(GenericArgs),
    /// List all MRAPs
    List(GenericArgs),
    /// Create MRAP routes
    #[command(name = "create-routes")]
    CreateRoutes(GenericArgs),
    /// Create an MRAP
    Create(GenericArgs),
    /// Set MRAP mirror configuration
    #[command(name = "set-mirror")]
    SetMirror(GenericArgs),
    /// Set MRAP policy
    #[command(name = "set-policy")]
    SetPolicy(GenericArgs),
}

// =============================================================================
// Advanced: AP - Access Point (10 actions)
// =============================================================================

#[derive(Debug, Args)]
pub struct ApCommand {
    #[command(subcommand)]
    pub action: Option<ApAction>,
}

#[derive(Debug, Subcommand)]
pub enum ApAction {
    /// Delete an access point
    Delete(GenericArgs),
    /// Delete access point policy
    #[command(name = "delete-policy")]
    DeletePolicy(GenericArgs),
    /// Unbind accelerator from access point
    #[command(name = "unbind-accelerator")]
    UnbindAccelerator(GenericArgs),
    /// Bind accelerator to access point
    #[command(name = "bind-accelerator")]
    BindAccelerator(GenericArgs),
    /// Get access point details
    Get(GenericArgs),
    /// Get access point policy
    #[command(name = "get-policy")]
    GetPolicy(GenericArgs),
    /// List access points
    List(GenericArgs),
    /// List accelerators for access point
    #[command(name = "list-accelerators")]
    ListAccelerators(GenericArgs),
    /// Create an access point
    Create(GenericArgs),
    /// Set access point policy
    #[command(name = "set-policy")]
    SetPolicy(GenericArgs),
}

// =============================================================================
// Advanced: CAP - Cross-Account Access Point (9 actions)
// =============================================================================

#[derive(Debug, Args)]
pub struct CapCommand {
    #[command(subcommand)]
    pub action: Option<CapAction>,
}

#[derive(Debug, Subcommand)]
pub enum CapAction {
    /// Delete a cross-account access point
    Delete(GenericArgs),
    /// Delete custom endpoint for CAP
    #[command(name = "delete-custom-endpoint")]
    DeleteCustomEndpoint(GenericArgs),
    /// Get cross-account access point details
    Get(GenericArgs),
    /// Get custom endpoint token
    #[command(name = "get-custom-endpoint-token")]
    GetCustomEndpointToken(GenericArgs),
    /// List cross-account access points
    List(GenericArgs),
    /// Create a cross-account access point
    Create(GenericArgs),
    /// Create custom endpoint for CAP
    #[command(name = "create-custom-endpoint")]
    CreateCustomEndpoint(GenericArgs),
    /// Create custom endpoint token
    #[command(name = "create-custom-endpoint-token")]
    CreateCustomEndpointToken(GenericArgs),
    /// Create object set for CAP
    #[command(name = "create-object-set")]
    CreateObjectSet(GenericArgs),
}

// =============================================================================
// Advanced: Dataset (11 actions)
// =============================================================================

#[derive(Debug, Args)]
pub struct DatasetCommand {
    #[command(subcommand)]
    pub action: Option<DatasetAction>,
}

#[derive(Debug, Subcommand)]
pub enum DatasetAction {
    /// Delete a dataset
    Delete(GenericArgs),
    /// Delete a dataset binding
    #[command(name = "delete-binding")]
    DeleteBinding(GenericArgs),
    /// Get dataset details
    Get(GenericArgs),
    /// Get a dataset binding
    #[command(name = "get-binding")]
    GetBinding(GenericArgs),
    /// List dataset bindings
    #[command(name = "list-bindings")]
    ListBindings(GenericArgs),
    /// List datasets
    List(GenericArgs),
    /// List dataset templates
    #[command(name = "list-templates")]
    ListTemplates(GenericArgs),
    /// Create a dataset
    Create(GenericArgs),
    /// Create a dataset binding
    #[command(name = "create-binding")]
    CreateBinding(GenericArgs),
    /// Query a dataset
    Query(GenericArgs),
    /// Update a dataset
    Update(GenericArgs),
}

// =============================================================================
// Advanced: Control (21 actions)
// =============================================================================

#[derive(Debug, Args)]
pub struct ControlCommand {
    #[command(subcommand)]
    pub action: Option<ControlAction>,
}

#[derive(Debug, Subcommand)]
pub enum ControlAction {
    /// Create URL cache purge/prefetch
    #[command(name = "create-url-cache")]
    CreateUrlCache(GenericArgs),
    /// Delete URL cache
    #[command(name = "delete-url-cache")]
    DeleteUrlCache(GenericArgs),
    /// Delete event subscription
    #[command(name = "delete-subscribe")]
    DeleteSubscribe(GenericArgs),
    /// Get event subscription
    #[command(name = "get-subscribe")]
    GetSubscribe(GenericArgs),
    /// Set event subscription
    #[command(name = "set-subscribe")]
    SetSubscribe(GenericArgs),
    /// Delete a batch job
    #[command(name = "delete-batch-job")]
    DeleteBatchJob(GenericArgs),
    /// Get a batch job
    #[command(name = "get-batch-job")]
    GetBatchJob(GenericArgs),
    /// List batch jobs
    #[command(name = "list-batch-jobs")]
    ListBatchJobs(GenericArgs),
    /// Create a batch job
    #[command(name = "create-batch-job")]
    CreateBatchJob(GenericArgs),
    /// Set batch job priority
    #[command(name = "set-batch-job-priority")]
    SetBatchJobPriority(GenericArgs),
    /// Set batch job status
    #[command(name = "set-batch-job-status")]
    SetBatchJobStatus(GenericArgs),
    /// Delete a lens configuration
    #[command(name = "delete-lens")]
    DeleteLens(GenericArgs),
    /// Get a lens configuration
    #[command(name = "get-lens")]
    GetLens(GenericArgs),
    /// List lens configurations
    #[command(name = "list-lens")]
    ListLens(GenericArgs),
    /// Set a lens configuration
    #[command(name = "set-lens")]
    SetLens(GenericArgs),
    /// Delete QoS policy
    #[command(name = "delete-qos-policy")]
    DeleteQosPolicy(GenericArgs),
    /// Get QoS policy
    #[command(name = "get-qos-policy")]
    GetQosPolicy(GenericArgs),
    /// Set QoS policy
    #[command(name = "set-qos-policy")]
    SetQosPolicy(GenericArgs),
    /// List resource tags
    #[command(name = "list-resource-tags")]
    ListResourceTags(GenericArgs),
    /// Set a resource tag
    #[command(name = "set-resource-tag")]
    SetResourceTag(GenericArgs),
    /// Delete a resource tag
    #[command(name = "delete-resource-tag")]
    DeleteResourceTag(GenericArgs),
}
