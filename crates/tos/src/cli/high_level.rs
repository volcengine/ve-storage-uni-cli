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

use clap::{Args, ValueEnum};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ProgressGranularity {
    /// Count completed transfer parts
    Part,
    /// Count transferred bytes
    Byte,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OverwriteStrategy {
    /// Always overwrite destination when the operation supports it
    Force,
    /// Do not overwrite existing destination
    NoClobber,
    /// Overwrite only when source timestamp is newer than destination
    Newer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum RecursiveListMode {
    /// Use surface defaults: tos lists with delimiter="/"; ve-tos uses HNS hierarchical and FNS flat.
    Auto,
    /// List recursively with delimiter="" where supported.
    Flat,
    /// List recursively by prefix with delimiter="/".
    Hierarchical,
}

fn parse_bucket_uri_value(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    let Some(rest) = trimmed.strip_prefix("tos://") else {
        return Err(format!(
            "invalid bucket URI '{}': expected tos://bucket",
            raw
        ));
    };
    let mut parts = rest.splitn(2, '/');
    let bucket = parts.next().unwrap_or_default();
    if bucket.is_empty() {
        return Err(format!("invalid bucket URI '{}': missing bucket name", raw));
    }
    if let Some(suffix) = parts.next() {
        if !suffix.is_empty() {
            return Err(format!(
                "invalid bucket URI '{}': expected tos://bucket",
                raw
            ));
        }
    }
    Ok(trimmed.to_string())
}

#[derive(Debug, Args)]
#[command(
    after_help = "Examples:\n  ve-tos-cli cp ./file.txt tos://mybucket/path/file.txt\n  ve-tos-cli cp tos://mybucket/path/ ./local_dir/ --recursive\n  ve-tos-cli cp ./dir/ tos://mybucket/prefix/ --recursive --include \"*.log\"\n  ve-tos-cli cp large.bin tos://mybucket/large.bin --checkpoint"
)]
pub struct CpArgs {
    /// Source path (local path or tos://bucket/key)
    pub source: String,
    /// Destination path (local path or tos://bucket/key)
    pub destination: String,
    /// Recursive copy
    // [Review Fix #8] `-r` is reserved by global `--region`; keep recursive as long-only.
    #[arg(long)]
    pub recursive: bool,
    /// Include the source directory/prefix name under the destination prefix
    #[arg(long, requires = "recursive")]
    pub include_parent: bool,
    /// Include pattern
    #[arg(long)]
    pub include: Option<String>,
    /// Exclude pattern
    #[arg(long)]
    pub exclude: Option<String>,
    /// Enable checkpoint for resumable transfer
    #[arg(long)]
    pub checkpoint: bool,
    /// Checkpoint directory override
    #[arg(long)]
    pub checkpoint_dir: Option<String>,
    /// Content-Type for TOS uploads/copies
    #[arg(long)]
    pub content_type: Option<String>,
    /// Storage class for ve-tos uploads and TOS-to-TOS copies. ByteTOS tos uploads do not support creation-time override. Allowed: STANDARD, IA, ARCHIVE_FR, INTELLIGENT_TIERING, COLD_ARCHIVE, ARCHIVE, DEEP_COLD_ARCHIVE
    #[arg(long)]
    pub storage_class: Option<String>,
    /// Target object ACL for TOS uploads/copies. Allowed: private, public-read, public-read-write, authenticated-read, bucket-owner-read, bucket-owner-full-control, bucket-owner-entrusted, default
    #[arg(long)]
    pub acl: Option<String>,
    /// Custom TOS metadata as key=value#key2=value2; writes x-tos-meta-* headers
    #[arg(long, alias = "metadata")]
    pub meta: Option<String>,
    /// File size threshold for checkpoint multipart/range transfer (e.g., 20MB)
    #[arg(long)]
    pub checkpoint_threshold: Option<String>,
    /// Maximum files/items running concurrently in batch commands
    #[arg(long)]
    pub batch_concurrency: Option<usize>,
    /// Maximum prefixes listed concurrently when recursive listing uses delimiter="/"
    #[arg(long)]
    pub list_concurrency: Option<usize>,
    /// Recursive listing mode: auto, flat, or hierarchical
    #[arg(long, value_enum, requires = "recursive")]
    pub recursive_list_mode: Option<RecursiveListMode>,
    /// Maximum parts/ranges running concurrently for one large file
    #[arg(long)]
    pub multipart_concurrency: Option<usize>,
    /// Progress granularity: part (default) or byte
    #[arg(long, value_enum)]
    pub progress_granularity: Option<ProgressGranularity>,
    /// Destination overwrite strategy
    #[arg(long, value_enum)]
    pub overwrite_strategy: Option<OverwriteStrategy>,
    /// Write batch success/failure report to this path
    #[arg(long)]
    pub report_path: Option<String>,
    /// Write only failed items to the batch report
    #[arg(long)]
    pub report_failures_only: bool,
    /// Write planned transfer manifest to this path
    #[arg(long)]
    pub manifest_path: Option<String>,
    /// Do not write a planned transfer manifest
    #[arg(long, conflicts_with = "manifest_path")]
    pub no_manifest: bool,
    /// Bandwidth limit (e.g., 100MB)
    #[arg(long)]
    pub bandwidth_limit: Option<String>,
    /// Enable listing-phase echo output even when stderr is not a TTY
    #[arg(long, conflicts_with = "no_list_echo")]
    pub list_echo: bool,
    /// Disable listing-phase echo output
    #[arg(long, conflicts_with = "list_echo")]
    pub no_list_echo: bool,
    /// Enable execution progress output even when stderr is not a TTY
    #[arg(long, conflicts_with = "no_progress")]
    pub progress: bool,
    /// Disable execution progress output
    #[arg(long, conflicts_with = "progress")]
    pub no_progress: bool,
    /// Force overwrite without confirmation
    #[arg(long)]
    pub force: bool,
    /// Do not overwrite existing objects (sets if-none-match: *)
    #[arg(long)]
    pub no_clobber: bool,
}

#[derive(Debug, Args)]
#[command(
    after_help = "Examples:\n  ve-tos-cli mv tos://mybucket/old.txt tos://mybucket/new.txt --force --confirm tos://mybucket/old.txt\n  ve-tos-cli mv tos://mybucket/src/ tos://mybucket/dst/ --recursive --force --confirm tos://mybucket/src/\n  ve-tos-cli mv ./local.txt tos://mybucket/remote.txt --force --confirm ./local.txt"
)]
pub struct MvArgs {
    /// Source path (local path or tos://bucket/key)
    pub source: String,
    /// Destination path (local path or tos://bucket/key)
    pub destination: String,
    /// Recursive move for directories or prefixes
    // [Review Fix #9] `-r` is reserved by global `--region`; keep recursive as long-only.
    #[arg(long)]
    pub recursive: bool,
    /// Include the source directory/prefix name under the destination prefix
    #[arg(long, requires = "recursive")]
    pub include_parent: bool,
    /// Include pattern
    #[arg(long)]
    pub include: Option<String>,
    /// Exclude pattern
    #[arg(long)]
    pub exclude: Option<String>,
    /// Checkpoint directory override
    #[arg(long)]
    pub checkpoint_dir: Option<String>,
    /// Content-Type for TOS uploads/copies
    #[arg(long)]
    pub content_type: Option<String>,
    /// Storage class for ve-tos uploads and TOS-to-TOS copies. ByteTOS tos uploads do not support creation-time override. Allowed: STANDARD, IA, ARCHIVE_FR, INTELLIGENT_TIERING, COLD_ARCHIVE, ARCHIVE, DEEP_COLD_ARCHIVE
    #[arg(long)]
    pub storage_class: Option<String>,
    /// Target object ACL for TOS uploads/copies. Allowed: private, public-read, public-read-write, authenticated-read, bucket-owner-read, bucket-owner-full-control, bucket-owner-entrusted, default
    #[arg(long)]
    pub acl: Option<String>,
    /// Custom TOS metadata as key=value#key2=value2; writes x-tos-meta-* headers
    #[arg(long, alias = "metadata")]
    pub meta: Option<String>,
    /// File size threshold for checkpoint multipart/range transfer (e.g., 20MB)
    #[arg(long)]
    pub checkpoint_threshold: Option<String>,
    /// Maximum files/items running concurrently in batch commands
    #[arg(long)]
    pub batch_concurrency: Option<usize>,
    /// Maximum prefixes listed concurrently when recursive listing uses delimiter="/"
    #[arg(long)]
    pub list_concurrency: Option<usize>,
    /// Recursive listing mode: auto, flat, or hierarchical
    #[arg(long, value_enum, requires = "recursive")]
    pub recursive_list_mode: Option<RecursiveListMode>,
    /// Maximum parts/ranges running concurrently for one large file
    #[arg(long)]
    pub multipart_concurrency: Option<usize>,
    /// Progress granularity: part (default) or byte
    #[arg(long, value_enum)]
    pub progress_granularity: Option<ProgressGranularity>,
    /// Destination overwrite strategy
    #[arg(long, value_enum)]
    pub overwrite_strategy: Option<OverwriteStrategy>,
    /// Write batch success/failure report to this path
    #[arg(long)]
    pub report_path: Option<String>,
    /// Write only failed items to the batch report
    #[arg(long)]
    pub report_failures_only: bool,
    /// Write planned transfer manifest to this path
    #[arg(long)]
    pub manifest_path: Option<String>,
    /// Do not write a planned transfer manifest
    #[arg(long, conflicts_with = "manifest_path")]
    pub no_manifest: bool,
    /// Enable listing-phase echo output even when stderr is not a TTY
    #[arg(long, conflicts_with = "no_list_echo")]
    pub list_echo: bool,
    /// Disable listing-phase echo output
    #[arg(long, conflicts_with = "list_echo")]
    pub no_list_echo: bool,
    /// Enable execution progress output even when stderr is not a TTY
    #[arg(long, conflicts_with = "no_progress")]
    pub progress: bool,
    /// Disable execution progress output
    #[arg(long, conflicts_with = "progress")]
    pub no_progress: bool,
    /// Force overwrite/delete confirmation
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
#[command(
    after_help = "Examples:\n  ve-tos-cli sync tos://src-bucket/prefix/ tos://dst-bucket/prefix/\n  ve-tos-cli sync ./local-dir/ tos://mybucket/backup/ --delete --force --confirm tos://mybucket/backup/\n  ve-tos-cli sync tos://mybucket/data/ ./local/ --include '*.csv'"
)]
pub struct SyncArgs {
    /// Source path
    pub source: String,
    /// Destination path
    pub destination: String,
    /// Delete extraneous files from destination
    #[arg(long)]
    pub delete: bool,
    /// Confirm deletion when --delete is enabled
    #[arg(long)]
    pub force: bool,
    /// Compare by size only (skip mtime)
    #[arg(long)]
    pub size_only: bool,
    /// Use exact timestamps for comparison
    #[arg(long)]
    pub exact_timestamps: bool,
    /// Include the source directory/prefix name under the destination prefix
    #[arg(long)]
    pub include_parent: bool,
    /// Include pattern
    #[arg(long)]
    pub include: Option<String>,
    /// Exclude pattern
    #[arg(long)]
    pub exclude: Option<String>,
    /// Checkpoint directory override
    #[arg(long)]
    pub checkpoint_dir: Option<String>,
    /// Content-Type for TOS uploads/copies
    #[arg(long)]
    pub content_type: Option<String>,
    /// Storage class for ve-tos uploads and TOS-to-TOS copies. ByteTOS tos uploads do not support creation-time override. Allowed: STANDARD, IA, ARCHIVE_FR, INTELLIGENT_TIERING, COLD_ARCHIVE, ARCHIVE, DEEP_COLD_ARCHIVE
    #[arg(long)]
    pub storage_class: Option<String>,
    /// Target object ACL for TOS uploads/copies. Allowed: private, public-read, public-read-write, authenticated-read, bucket-owner-read, bucket-owner-full-control, bucket-owner-entrusted, default
    #[arg(long)]
    pub acl: Option<String>,
    /// Custom TOS metadata as key=value#key2=value2; writes x-tos-meta-* headers
    #[arg(long, alias = "metadata")]
    pub meta: Option<String>,
    /// File size threshold for checkpoint multipart/range transfer (e.g., 20MB)
    #[arg(long)]
    pub checkpoint_threshold: Option<String>,
    /// Maximum files/items running concurrently in batch commands
    #[arg(long)]
    pub batch_concurrency: Option<usize>,
    /// Maximum prefixes listed concurrently when recursive listing uses delimiter="/"
    #[arg(long)]
    pub list_concurrency: Option<usize>,
    /// Recursive listing mode: auto, flat, or hierarchical
    #[arg(long, value_enum)]
    pub recursive_list_mode: Option<RecursiveListMode>,
    /// Maximum parts/ranges running concurrently for one large file
    #[arg(long)]
    pub multipart_concurrency: Option<usize>,
    /// Progress granularity: part (default) or byte
    #[arg(long, value_enum)]
    pub progress_granularity: Option<ProgressGranularity>,
    /// Destination overwrite strategy
    #[arg(long, value_enum)]
    pub overwrite_strategy: Option<OverwriteStrategy>,
    /// Write batch success/failure report to this path
    #[arg(long)]
    pub report_path: Option<String>,
    /// Write only failed items to the batch report
    #[arg(long)]
    pub report_failures_only: bool,
    /// Write planned transfer manifest to this path
    #[arg(long)]
    pub manifest_path: Option<String>,
    /// Do not write a planned transfer manifest
    #[arg(long, conflicts_with = "manifest_path")]
    pub no_manifest: bool,
    /// Bandwidth limit
    #[arg(long)]
    pub bandwidth_limit: Option<String>,
    /// Enable listing-phase echo output even when stderr is not a TTY
    #[arg(long, conflicts_with = "no_list_echo")]
    pub list_echo: bool,
    /// Disable listing-phase echo output
    #[arg(long, conflicts_with = "list_echo")]
    pub no_list_echo: bool,
    /// Enable execution progress output even when stderr is not a TTY
    #[arg(long, conflicts_with = "no_progress")]
    pub progress: bool,
    /// Disable execution progress output
    #[arg(long, conflicts_with = "progress")]
    pub no_progress: bool,
}

#[derive(Debug, Args)]
#[command(
    after_help = "Examples:\n  ve-tos-cli mb tos://new-bucket\n  ve-tos-cli mb tos://new-bucket --storage-class IA\n  ve-tos-cli mb tos://new-bucket --bucket-type hns\n  ve-tos-cli mb tos://new-bucket --region cn-beijing"
)]
pub struct MbArgs {
    /// Bucket URI (tos://bucket)
    #[arg(value_name = "URI", value_parser = parse_bucket_uri_value)]
    pub bucket: String,
    /// Region override for this request
    #[arg(long)]
    pub region: Option<String>,
    /// Storage class. Allowed: STANDARD, IA, ARCHIVE_FR, INTELLIGENT_TIERING, COLD_ARCHIVE, ARCHIVE, DEEP_COLD_ARCHIVE
    #[arg(long, default_value = "STANDARD")]
    pub storage_class: String,
    /// Bucket ACL. Allowed: private, public-read, public-read-write, authenticated-read, bucket-owner-read, bucket-owner-full-control
    #[arg(long)]
    pub acl: Option<String>,
    /// AZ redundancy mode. Allowed: single-az, multi-az
    #[arg(long)]
    pub az_redundancy: Option<String>,
    /// Bucket type. Allowed: fns, hns
    #[arg(long)]
    pub bucket_type: Option<String>,
    /// Enable bucket object lock
    #[arg(long)]
    pub bucket_object_lock_enabled: bool,
}

#[derive(Debug, Args)]
#[command(
    after_help = "Examples:\n  ve-tos-cli rb tos://mybucket --force --confirm tos://mybucket"
)]
pub struct RbArgs {
    /// Bucket URI (tos://bucket)
    #[arg(value_name = "URI", value_parser = parse_bucket_uri_value)]
    pub bucket: String,
    /// Confirm bucket deletion
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
#[command(
    after_help = "Examples:\n  ve-tos-cli mkdir tos://mybucket/folder/\n  ve-tos-cli mkdir tos://mybucket/folder/subfolder --parents\n  ve-tos-cli mkdir --bucket mybucket --key folder/"
)]
pub struct MkdirArgs {
    /// Folder path (tos://bucket/folder/)
    #[arg(value_name = "PATH", conflicts_with_all = ["bucket", "key"])]
    pub path: Option<String>,
    /// Bucket name (alternative to positional URI)
    #[arg(long)]
    pub bucket: Option<String>,
    /// Folder key (used with --bucket)
    #[arg(long)]
    pub key: Option<String>,
    /// Create parent folder markers as needed
    #[arg(long, short = 'p')]
    pub parents: bool,
}

#[derive(Debug, Args)]
#[command(
    after_help = "Examples:\n  ve-tos-cli rm tos://mybucket/file.txt --force --confirm tos://mybucket/file.txt\n  ve-tos-cli rm tos://mybucket/prefix/ --recursive --force --confirm tos://mybucket/prefix/\n  ve-tos-cli rm tos://mybucket/prefix/ --recursive --all-versions --force --confirm tos://mybucket/prefix/"
)]
pub struct RmArgs {
    /// Target path (tos://bucket/key or tos://bucket/prefix/)
    #[arg(value_name = "PATH", conflicts_with_all = ["bucket", "key"])]
    pub path: Option<String>,
    /// Bucket name (alternative to positional URI)
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key or prefix (used with --bucket)
    #[arg(long)]
    pub key: Option<String>,
    /// Recursive delete
    // [Review Fix #8] `-r` is reserved by global `--region`; keep recursive as long-only.
    #[arg(long)]
    pub recursive: bool,
    /// Recursive delete strategy for HNS buckets
    #[arg(long, value_enum, requires = "recursive")]
    pub recursive_delete_mode: Option<RecursiveDeleteMode>,
    /// Force delete without confirmation
    #[arg(long)]
    pub force: bool,
    /// Delete every object version (and delete markers) instead of only the
    /// current version. Required when the bucket has versioning enabled and
    /// the caller wants permanent removal.
    #[arg(long)]
    pub all_versions: bool,
    /// Also abort incomplete multipart uploads matching the prefix
    #[arg(long)]
    pub include_uploads: bool,
    /// Write batch success/failure report to this path
    #[arg(long)]
    pub report_path: Option<String>,
    /// Write only failed items to the batch report
    #[arg(long)]
    pub report_failures_only: bool,
    /// Write planned delete manifest to this path
    #[arg(long)]
    pub manifest_path: Option<String>,
    /// Do not write a planned delete manifest
    #[arg(long, conflicts_with = "manifest_path")]
    pub no_manifest: bool,
    /// Maximum files/items running concurrently in this batch delete
    #[arg(long)]
    pub batch_concurrency: Option<usize>,
    /// Maximum prefixes listed concurrently when recursive listing uses delimiter="/"
    #[arg(long)]
    pub list_concurrency: Option<usize>,
    /// Recursive listing mode: auto, flat, or hierarchical
    #[arg(long, value_enum, requires = "recursive")]
    pub recursive_list_mode: Option<RecursiveListMode>,
    /// Include pattern
    #[arg(long)]
    pub include: Option<String>,
    /// Exclude pattern
    #[arg(long)]
    pub exclude: Option<String>,
    /// Enable listing-phase echo output even when stderr is not a TTY
    #[arg(long, conflicts_with = "no_list_echo")]
    pub list_echo: bool,
    /// Disable listing-phase echo output
    #[arg(long, conflicts_with = "list_echo")]
    pub no_list_echo: bool,
    /// Enable execution progress output even when stderr is not a TTY
    #[arg(long, conflicts_with = "no_progress")]
    pub progress: bool,
    /// Disable execution progress output
    #[arg(long, conflicts_with = "progress")]
    pub no_progress: bool,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum RecursiveDeleteMode {
    /// Delete children before parent directory objects.
    BottomUp,
    /// Ask the service to delete a directory object recursively.
    Direct,
}

#[derive(Debug, Args)]
#[command(
    after_help = "Examples:\n  ve-tos-cli ls\n  ve-tos-cli ls tos://mybucket/\n  ve-tos-cli ls tos://mybucket/prefix/ --max-keys 100\n  ve-tos-cli ls tos://mybucket/ --human-readable --sort size"
)]
pub struct LsArgs {
    /// Path to list (tos://bucket or tos://bucket/prefix/)
    #[arg(value_name = "PATH", conflicts_with_all = ["bucket", "key"])]
    pub path: Option<String>,
    /// Bucket name (alternative to positional URI)
    #[arg(long)]
    pub bucket: Option<String>,
    /// Key prefix (used with --bucket)
    #[arg(long)]
    pub key: Option<String>,
    /// Maximum buckets, objects, or prefixes to return from the current level
    #[arg(long, default_value = "1000")]
    pub max_keys: u32,
    /// Continuation token returned by a previous listing
    #[arg(long)]
    pub continuation_token: Option<String>,
    /// Human-readable sizes
    #[arg(long, short = 'H')]
    pub human_readable: bool,
    /// Sort field
    #[arg(long)]
    pub sort: Option<String>,
    /// Select columns for table/csv output (comma-separated, e.g. key,size,last_modified)
    #[arg(long)]
    pub columns: Option<String>,
    /// Write listing manifest to this path. No manifest is written unless this is set.
    #[arg(long)]
    pub manifest_path: Option<String>,
}

#[derive(Debug, Args)]
#[command(
    after_help = "Examples:\n  ve-tos-cli stat tos://mybucket/file.txt\n  ve-tos-cli stat tos://mybucket/file.txt --version-id v1"
)]
pub struct StatArgs {
    /// Path to inspect (tos://bucket or tos://bucket/key)
    #[arg(value_name = "PATH", conflicts_with_all = ["bucket", "key"])]
    pub path: Option<String>,
    /// Bucket name (alternative to positional URI)
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key (used with --bucket)
    #[arg(long)]
    pub key: Option<String>,
    /// Object version ID
    #[arg(long)]
    pub version_id: Option<String>,
}

#[derive(Debug, Args)]
#[command(
    after_help = "Examples:\n  ve-tos-cli du tos://mybucket/\n  ve-tos-cli du tos://mybucket/prefix/ --human-readable\n  ve-tos-cli du tos://mybucket/ --max-depth 2 --cost"
)]
pub struct DuArgs {
    /// Path to measure (tos://bucket or tos://bucket/prefix/)
    #[arg(value_name = "PATH", conflicts_with_all = ["bucket", "key"])]
    pub path: Option<String>,
    /// Bucket name (alternative to positional URI)
    #[arg(long)]
    pub bucket: Option<String>,
    /// Key prefix (used with --bucket)
    #[arg(long)]
    pub key: Option<String>,
    /// Human-readable sizes
    #[arg(long, short = 'H')]
    pub human_readable: bool,
    /// Maximum directory depth
    #[arg(long)]
    pub max_depth: Option<u32>,
    /// Number of largest/oldest object samples to keep in --verbose diagnostics; 0 disables samples
    #[arg(long, default_value = "10")]
    pub top_k: usize,
    /// Include estimated monthly storage cost by storage class
    #[arg(long)]
    pub cost: bool,
    /// Override storage price, e.g. STANDARD=0.12 (CNY/GB/month)
    #[arg(long, value_name = "CLASS=PRICE")]
    pub storage_price: Vec<String>,
    /// Write traversed-object manifest to this path. No manifest is written unless this is set.
    #[arg(long)]
    pub manifest_path: Option<String>,
    /// Maximum prefixes listed concurrently when the bucket is listed hierarchically
    #[arg(long)]
    pub list_concurrency: Option<usize>,
    /// Enable listing-phase echo output even when stderr is not a TTY
    #[arg(long, conflicts_with = "no_list_echo")]
    pub list_echo: bool,
    /// Disable listing-phase echo output
    #[arg(long, conflicts_with = "list_echo")]
    pub no_list_echo: bool,
    /// Enable execution progress output even when stderr is not a TTY
    #[arg(long, conflicts_with = "no_progress")]
    pub progress: bool,
    /// Disable execution progress output
    #[arg(long, conflicts_with = "progress")]
    pub no_progress: bool,
}

#[derive(Debug, Args)]
#[command(
    after_help = "Examples:\n  ve-tos-cli find tos://mybucket/ --name \"*.log\"\n  ve-tos-cli find tos://mybucket/ --size +100MB\n  ve-tos-cli find tos://mybucket/ --mtime -7d --storage-class STANDARD"
)]
pub struct FindArgs {
    /// Search path (tos://bucket or tos://bucket/prefix/)
    #[arg(value_name = "PATH", conflicts_with_all = ["bucket", "key"])]
    pub path: Option<String>,
    /// Bucket name (alternative to positional URI)
    #[arg(long)]
    pub bucket: Option<String>,
    /// Key prefix (used with --bucket)
    #[arg(long)]
    pub key: Option<String>,
    /// Name pattern
    #[arg(long)]
    pub name: Option<String>,
    /// Size filter (e.g., +1GB, -100KB)
    #[arg(long, allow_hyphen_values = true)]
    pub size: Option<String>,
    /// Modification time filter; bare durations such as 7d mean objects modified within that window
    #[arg(long, allow_hyphen_values = true)]
    pub mtime: Option<String>,
    /// Storage class filter
    #[arg(long)]
    pub storage_class: Option<String>,
    /// Write matched-object manifest to this path. No manifest is written unless this is set.
    #[arg(long)]
    pub manifest_path: Option<String>,
    /// Enable listing-phase echo output even when stderr is not a TTY
    #[arg(long, conflicts_with = "no_list_echo")]
    pub list_echo: bool,
    /// Disable listing-phase echo output
    #[arg(long, conflicts_with = "list_echo")]
    pub no_list_echo: bool,
    /// Enable execution progress output even when stderr is not a TTY
    #[arg(long, conflicts_with = "no_progress")]
    pub progress: bool,
    /// Disable execution progress output
    #[arg(long, conflicts_with = "progress")]
    pub no_progress: bool,
}

#[derive(Debug, Args)]
#[command(
    after_help = "Examples:\n  ve-tos-cli cat tos://mybucket/file.txt\n  ve-tos-cli cat tos://mybucket/file.txt --range bytes=0-1023"
)]
pub struct CatArgs {
    /// Object path (tos://bucket/key)
    #[arg(value_name = "PATH", conflicts_with_all = ["bucket", "key"])]
    pub path: Option<String>,
    /// Bucket name (alternative to positional URI)
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key (used with --bucket)
    #[arg(long)]
    pub key: Option<String>,
    /// Byte range (e.g., 0-1023)
    #[arg(long)]
    pub range: Option<String>,
    /// Version ID
    #[arg(long)]
    pub version_id: Option<String>,
}

#[derive(Debug, Args)]
#[command(
    after_help = "Stdin input is uploaded after EOF: press Ctrl+D on Unix/macOS, or Ctrl+Z then Enter on Windows. Ctrl+C cancels the command.\n\nExamples:\n  ve-tos-cli cat tos://src/file.txt | gzip | ve-tos-cli put tos://dst/file.txt.gz\n  ve-tos-cli put tos://mybucket/path/file.gz --content-type application/gzip\n  ve-tos-cli put --bucket mybucket --key path/file.gz --no-clobber"
)]
pub struct PutArgs {
    /// Object path to write (tos://bucket/key)
    #[arg(value_name = "PATH", conflicts_with_all = ["bucket", "key"])]
    pub path: Option<String>,
    /// Bucket name (alternative to positional URI)
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key (used with --bucket)
    #[arg(long)]
    pub key: Option<String>,
    /// Content-Type for the uploaded object
    #[arg(long)]
    pub content_type: Option<String>,
    /// Storage class for ve-tos stdin uploads. ByteTOS tos put does not support creation-time override. Allowed: STANDARD, IA, ARCHIVE_FR, INTELLIGENT_TIERING, COLD_ARCHIVE, ARCHIVE, DEEP_COLD_ARCHIVE
    #[arg(long)]
    pub storage_class: Option<String>,
    /// Target object ACL. Allowed: private, public-read, public-read-write, authenticated-read, bucket-owner-read, bucket-owner-full-control, bucket-owner-entrusted, default
    #[arg(long)]
    pub acl: Option<String>,
    /// Custom TOS metadata as key=value#key2=value2; writes x-tos-meta-* headers
    #[arg(long, alias = "metadata")]
    pub meta: Option<String>,
    /// Stdin size threshold for switching to multipart upload (e.g., 20MB)
    #[arg(long)]
    pub multipart_threshold: Option<String>,
    /// Do not overwrite an existing object
    #[arg(long)]
    pub no_clobber: bool,
    /// Enable execution progress output even when stderr is not a TTY
    #[arg(long, conflicts_with = "no_progress")]
    pub progress: bool,
    /// Disable execution progress output
    #[arg(long, conflicts_with = "progress")]
    pub no_progress: bool,
}

#[derive(Debug, Args)]
#[command(
    after_help = "Examples:\n  ve-tos-cli presign tos://mybucket/file.txt\n  ve-tos-cli presign tos://mybucket/file.txt --expires 7200\n  ve-tos-cli presign tos://mybucket/file.txt --method PUT"
)]
pub struct PresignArgs {
    /// Object path (tos://bucket/key)
    #[arg(value_name = "PATH", conflicts_with_all = ["bucket", "key"])]
    pub path: Option<String>,
    /// Bucket name (alternative to positional URI)
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key (used with --bucket)
    #[arg(long)]
    pub key: Option<String>,
    /// URL expiration time (e.g., 3600)
    #[arg(long, default_value = "3600")]
    pub expires: u64,
    /// HTTP method (GET, PUT)
    #[arg(long, default_value = "GET")]
    pub method: String,
}

#[derive(Debug, Args)]
#[command(
    after_help = "Examples:\n  ve-tos-cli restore tos://mybucket/archived.txt --days 7 --force\n  ve-tos-cli restore tos://mybucket/prefix/ --recursive --tier Standard --force\n  ve-tos-cli restore tos://mybucket/prefix/ --manifest list.txt --force"
)]
pub struct RestoreArgs {
    /// Archived object path or prefix (tos://bucket/key or tos://bucket/prefix/)
    #[arg(value_name = "PATH", conflicts_with_all = ["bucket", "key"])]
    pub path: Option<String>,
    /// Bucket name (alternative to positional URI)
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key or prefix (used with --bucket)
    #[arg(long)]
    pub key: Option<String>,
    /// Restore all archived objects under a prefix
    #[arg(long)]
    pub recursive: bool,
    /// Restore objects listed in a manifest file
    #[arg(long)]
    pub manifest: Option<String>,
    /// Include pattern for recursive restore
    #[arg(long)]
    pub include: Option<String>,
    /// Exclude pattern for recursive restore
    #[arg(long)]
    pub exclude: Option<String>,
    /// Restore days
    #[arg(long)]
    pub days: Option<u32>,
    /// Restore tier (Expedited, Standard, Bulk)
    #[arg(long)]
    pub tier: Option<String>,
    /// Object version ID
    #[arg(long)]
    pub version_id: Option<String>,
    /// Write batch success/failure report to this path
    #[arg(long)]
    pub report_path: Option<String>,
    /// Write only failed items to the batch report
    #[arg(long)]
    pub report_failures_only: bool,
    /// Write planned restore manifest to this path
    #[arg(long)]
    pub manifest_path: Option<String>,
    /// Do not write a planned restore manifest
    #[arg(long, conflicts_with = "manifest_path")]
    pub no_manifest: bool,
    /// Maximum files/items running concurrently in this batch restore
    #[arg(long)]
    pub batch_concurrency: Option<usize>,
    /// Maximum prefixes listed concurrently when recursive listing uses delimiter="/"
    #[arg(long)]
    pub list_concurrency: Option<usize>,
    /// Recursive listing mode: auto, flat, or hierarchical
    #[arg(long, value_enum, requires = "recursive")]
    pub recursive_list_mode: Option<RecursiveListMode>,
    /// Confirm batch restore and cost-related side effects
    #[arg(long)]
    pub force: bool,
    /// Enable listing-phase echo output even when stderr is not a TTY
    #[arg(long, conflicts_with = "no_list_echo")]
    pub list_echo: bool,
    /// Disable listing-phase echo output
    #[arg(long, conflicts_with = "list_echo")]
    pub no_list_echo: bool,
    /// Enable execution progress output even when stderr is not a TTY
    #[arg(long, conflicts_with = "no_progress")]
    pub progress: bool,
    /// Disable execution progress output
    #[arg(long, conflicts_with = "progress")]
    pub no_progress: bool,
}
