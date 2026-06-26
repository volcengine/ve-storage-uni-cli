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

#[derive(Debug, Clone, Args)]
#[command(
    after_help = "Examples:\n  ve-adrive-cli cp ./file.txt adrive://inst/space/docs/file.txt\n  ve-adrive-cli cp adrive://inst/space/docs/ ./local/ --recursive\n  ve-adrive-cli cp ./dir/ adrive://inst/space/backup/ --recursive --include \"*.log\""
)]
pub struct CpArgs {
    /// Source path (local path or adrive://instance/space/folder/file)
    pub source: String,
    /// Destination path (local path or adrive://instance/space/folder/file)
    pub destination: String,
    /// Treat ADrive instance/space target segments as names and resolve them to IDs
    #[arg(long)]
    pub by_name: bool,
    /// Recursive copy
    #[arg(long)]
    pub recursive: bool,
    /// Include the source directory/prefix name under the destination path
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
    /// File size threshold for checkpoint multipart/range transfer (e.g., 20MB)
    #[arg(long)]
    pub checkpoint_threshold: Option<String>,
    /// Maximum files/items running concurrently in batch commands
    #[arg(long)]
    pub batch_concurrency: Option<usize>,
    /// Maximum folder prefixes listed concurrently in recursive batch commands
    #[arg(long)]
    pub list_concurrency: Option<usize>,
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
    /// Do not overwrite existing files
    #[arg(long)]
    pub no_clobber: bool,
}

#[derive(Debug, Clone, Args)]
#[command(
    after_help = "Examples:\n  ve-adrive-cli mv adrive://inst/space/old.txt adrive://inst/space/new.txt --force --confirm adrive://inst/space/old.txt\n  ve-adrive-cli mv adrive://inst/space/src/ adrive://inst/space/dst/ --recursive --force --confirm adrive://inst/space/src/\n  ve-adrive-cli mv ./local.txt adrive://inst/space/remote.txt --force --confirm ./local.txt"
)]
pub struct MvArgs {
    /// Source path (local path or adrive://instance/space/folder/file)
    pub source: String,
    /// Destination path (local path or adrive://instance/space/folder/file)
    pub destination: String,
    /// Treat ADrive instance/space target segments as names and resolve them to IDs
    #[arg(long)]
    pub by_name: bool,
    /// Recursive move for directories
    #[arg(long)]
    pub recursive: bool,
    /// Include the source directory/prefix name under the destination path
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
    /// File size threshold for checkpoint multipart/range transfer (e.g., 20MB)
    #[arg(long)]
    pub checkpoint_threshold: Option<String>,
    /// Maximum files/items running concurrently in batch commands
    #[arg(long)]
    pub batch_concurrency: Option<usize>,
    /// Maximum folder prefixes listed concurrently in recursive batch commands
    #[arg(long)]
    pub list_concurrency: Option<usize>,
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

#[derive(Debug, Clone, Args)]
#[command(
    after_help = "Examples:\n  ve-adrive-cli sync adrive://inst/space/src/ adrive://inst/space/dst/\n  ve-adrive-cli sync ./local-dir/ adrive://inst/space/backup/ --delete --force --confirm adrive://inst/space/backup/\n  ve-adrive-cli sync adrive://inst/space/data/ ./local/ --include '*.csv'"
)]
pub struct SyncArgs {
    /// Source path
    pub source: String,
    /// Destination path
    pub destination: String,
    /// Treat ADrive instance/space target segments as names and resolve them to IDs
    #[arg(long)]
    pub by_name: bool,
    /// Delete extraneous files/folders from destination
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
    /// Include the source directory/prefix name under the destination path
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
    /// File size threshold for checkpoint multipart/range transfer (e.g., 20MB)
    #[arg(long)]
    pub checkpoint_threshold: Option<String>,
    /// Maximum files/items running concurrently in batch commands
    #[arg(long)]
    pub batch_concurrency: Option<usize>,
    /// Maximum folder prefixes listed concurrently in recursive batch commands
    #[arg(long)]
    pub list_concurrency: Option<usize>,
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

#[derive(Debug, Clone, Args)]
#[command(
    after_help = "Examples:\n  ve-adrive-cli crt adrive://inst-name\n  ve-adrive-cli crt adrive://inst-id/space-name\n  ve-adrive-cli crt --instance inst-id --space docs --index-enabled"
)]
pub struct CreateArgs {
    /// Resource to create (adrive://instance-name or adrive://instance-id/space-name)
    #[arg(value_name = "PATH", conflicts_with_all = ["instance", "space"])]
    pub path: Option<String>,
    /// Treat the parent instance target as a name when creating a space
    #[arg(long)]
    pub by_name: bool,
    /// Instance name to create, or existing instance ID when --space is set
    #[arg(long)]
    pub instance: Option<String>,
    /// Space name to create under --instance
    #[arg(long)]
    pub space: Option<String>,
    /// Display name for the created instance or space
    #[arg(long)]
    pub display_name: Option<String>,
    /// Description for the created instance or space
    #[arg(long)]
    pub description: Option<String>,
    /// Enable search indexing for a newly-created space
    #[arg(long)]
    pub index_enabled: bool,
}

#[derive(Debug, Clone, Args)]
#[command(
    after_help = "Examples:\n  ve-adrive-cli del adrive://inst-id --force --confirm adrive://inst-id\n  ve-adrive-cli del adrive://inst-id/space-id --force --confirm adrive://inst-id/space-id"
)]
pub struct DeleteArgs {
    /// Resource to delete (adrive://instance-id or adrive://instance-id/space-id)
    #[arg(value_name = "PATH", conflicts_with_all = ["instance", "space"])]
    pub path: Option<String>,
    /// Treat ADrive instance/space target segments as names and resolve them to IDs
    #[arg(long)]
    pub by_name: bool,
    /// Instance ID to delete, or containing instance ID when --space is set
    #[arg(long)]
    pub instance: Option<String>,
    /// Space ID to delete under --instance
    #[arg(long)]
    pub space: Option<String>,
    /// Confirm resource deletion
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Clone, Args)]
#[command(
    after_help = "Examples:\n  ve-adrive-cli rm adrive://inst/space/docs/file.txt --force --confirm adrive://inst/space/docs/file.txt\n  ve-adrive-cli rm adrive://inst/space/docs/ --recursive --include '*.log' --force --confirm adrive://inst/space/docs/\n  ve-adrive-cli rm adrive://inst/space --recursive --include-uploads --force --confirm adrive://inst/space\n  ve-adrive-cli rm --instance inst --space space --folder docs --force --recursive --confirm adrive://inst/space/docs"
)]
pub struct RmArgs {
    /// Target path (adrive://instance/space/folder/file or adrive://instance/space/folder/)
    #[arg(value_name = "PATH", conflicts_with_all = ["instance", "space", "folder", "file"])]
    pub path: Option<String>,
    /// Treat ADrive instance/space target segments as names and resolve them to IDs
    #[arg(long)]
    pub by_name: bool,
    /// Instance name (alternative to positional URI)
    #[arg(long)]
    pub instance: Option<String>,
    /// Space name (used with --instance)
    #[arg(long)]
    pub space: Option<String>,
    /// Folder path (used with --instance --space)
    #[arg(long)]
    pub folder: Option<String>,
    /// File name (used with --instance --space --folder)
    #[arg(long)]
    pub file: Option<String>,
    /// Recursive delete
    #[arg(long)]
    pub recursive: bool,
    /// Recursive folder delete strategy
    #[arg(long, value_enum, default_value = "bottom-up")]
    pub recursive_delete_mode: RecursiveDeleteMode,
    /// Force delete without confirmation
    #[arg(long)]
    pub force: bool,
    /// Also abort incomplete multipart uploads recorded in ADrive checkpoints matching the target
    #[arg(long)]
    pub include_uploads: bool,
    /// Checkpoint directory to scan when --include-uploads is set
    #[arg(long, requires = "include_uploads")]
    pub checkpoint_dir: Option<String>,
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
    /// Maximum folder prefixes listed concurrently in recursive batch deletes
    #[arg(long)]
    pub list_concurrency: Option<usize>,
    /// Include pattern for bottom-up recursive deletes
    #[arg(long)]
    pub include: Option<String>,
    /// Exclude pattern for bottom-up recursive deletes
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
    /// Delete children before parent folders.
    BottomUp,
    /// Ask the service to delete the folder directly.
    Direct,
}

#[derive(Debug, Clone, Args)]
#[command(
    after_help = "Examples:\n  ve-adrive-cli ls\n  ve-adrive-cli ls --instance myinst\n  ve-adrive-cli ls --instance myinst --space myspace\n  ve-adrive-cli ls adrive://myinst/myspace/prefix/ --max-keys 100"
)]
pub struct LsArgs {
    /// Path to list (adrive://instance/space or adrive://instance/space/folder/)
    #[arg(value_name = "PATH", conflicts_with_all = ["instance", "space", "folder"])]
    pub path: Option<String>,
    /// Treat ADrive instance/space target segments as names and resolve them to IDs
    #[arg(long)]
    pub by_name: bool,
    /// Instance name (alternative to positional URI)
    #[arg(long)]
    pub instance: Option<String>,
    /// Space name (used with --instance)
    #[arg(long)]
    pub space: Option<String>,
    /// Folder path (used with --instance --space)
    #[arg(long)]
    pub folder: Option<String>,
    /// Maximum entries to return from the current directory level
    #[arg(long, default_value = "1000")]
    pub max_keys: i32,
    /// Pagination marker returned by a previous listing
    #[arg(long)]
    pub marker: Option<String>,
    /// Human-readable sizes
    #[arg(long, short = 'H')]
    pub human_readable: bool,
    /// Sort field
    #[arg(long)]
    pub sort: Option<String>,
    /// Select columns for table/csv output (comma-separated)
    #[arg(long)]
    pub columns: Option<String>,
    /// Write listing manifest to this path. No manifest is written unless this is set.
    #[arg(long)]
    pub manifest_path: Option<String>,
}

#[derive(Debug, Clone, Args)]
#[command(
    after_help = "Examples:\n  ve-adrive-cli stat adrive://myinst\n  ve-adrive-cli stat adrive://myinst/myspace\n  ve-adrive-cli stat adrive://myinst/myspace/file.txt"
)]
pub struct StatArgs {
    /// Path to inspect (adrive://instance/space/folder/file)
    #[arg(value_name = "PATH", conflicts_with_all = ["instance", "space", "folder", "file"])]
    pub path: Option<String>,
    /// Treat ADrive instance/space target segments as names and resolve them to IDs
    #[arg(long)]
    pub by_name: bool,
    /// Instance name (alternative to positional URI)
    #[arg(long)]
    pub instance: Option<String>,
    /// Space name (used with --instance)
    #[arg(long)]
    pub space: Option<String>,
    /// Folder path (used with --instance --space)
    #[arg(long)]
    pub folder: Option<String>,
    /// File name (used with --instance --space --folder)
    #[arg(long)]
    pub file: Option<String>,
}

#[derive(Debug, Clone, Args)]
#[command(
    after_help = "Examples:\n  ve-adrive-cli du adrive://inst/space/\n  ve-adrive-cli du adrive://inst/space/docs/ --human-readable\n  ve-adrive-cli du adrive://inst/space/ --max-depth 2 --cost"
)]
pub struct DuArgs {
    /// Path to measure (adrive://instance/space or adrive://instance/space/folder/)
    #[arg(value_name = "PATH", conflicts_with_all = ["instance", "space", "folder"])]
    pub path: Option<String>,
    /// Treat ADrive instance/space target segments as names and resolve them to IDs
    #[arg(long)]
    pub by_name: bool,
    /// Instance name (alternative to positional URI)
    #[arg(long)]
    pub instance: Option<String>,
    /// Space name (used with --instance)
    #[arg(long)]
    pub space: Option<String>,
    /// Folder path (used with --instance --space)
    #[arg(long)]
    pub folder: Option<String>,
    /// Human-readable sizes
    #[arg(long, short = 'H')]
    pub human_readable: bool,
    /// Maximum directory depth
    #[arg(long)]
    pub max_depth: Option<u32>,
    /// Number of largest/oldest file samples to keep in --verbose diagnostics; 0 disables samples
    #[arg(long, default_value = "10")]
    pub top_k: usize,
    /// Include estimated monthly storage cost by storage class
    #[arg(long)]
    pub cost: bool,
    /// Override storage price, e.g. STANDARD=0.12 (CNY/GB/month)
    #[arg(long, value_name = "CLASS=PRICE")]
    pub storage_price: Vec<String>,
    /// Write traversed-file manifest to this path. No manifest is written unless this is set.
    #[arg(long)]
    pub manifest_path: Option<String>,
    /// Maximum folder prefixes listed concurrently while measuring recursively
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

#[derive(Debug, Clone, Args)]
#[command(
    after_help = "Examples:\n  ve-adrive-cli find adrive://inst/space/ --name \"*.pdf\"\n  ve-adrive-cli find adrive://inst/space/ --size +100MB\n  ve-adrive-cli find adrive://inst/space/docs/ --mtime -7d"
)]
pub struct FindArgs {
    /// Search path (adrive://instance/space or adrive://instance/space/folder/)
    #[arg(value_name = "PATH", conflicts_with_all = ["instance", "space", "folder"])]
    pub path: Option<String>,
    /// Treat ADrive instance/space target segments as names and resolve them to IDs
    #[arg(long)]
    pub by_name: bool,
    /// Instance name (alternative to positional URI)
    #[arg(long)]
    pub instance: Option<String>,
    /// Space name (used with --instance)
    #[arg(long)]
    pub space: Option<String>,
    /// Folder path (used with --instance --space)
    #[arg(long)]
    pub folder: Option<String>,
    /// Name pattern
    #[arg(long)]
    pub name: Option<String>,
    /// Size filter (e.g., +1GB, -100KB)
    #[arg(long, allow_hyphen_values = true)]
    pub size: Option<String>,
    /// Modification time filter; bare durations such as 7d mean files modified within that window
    #[arg(long, allow_hyphen_values = true)]
    pub mtime: Option<String>,
    /// Write matched-file manifest to this path. No manifest is written unless this is set.
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

#[derive(Debug, Clone, Args)]
#[command(
    after_help = "Examples:\n  ve-adrive-cli cat adrive://myinst/myspace/file.txt\n  ve-adrive-cli cat adrive://myinst/myspace/file.txt --range bytes=0-1023"
)]
pub struct CatArgs {
    /// File path (adrive://instance/space/folder/file)
    #[arg(value_name = "PATH", conflicts_with_all = ["instance", "space", "folder", "file"])]
    pub path: Option<String>,
    /// Treat ADrive instance/space target segments as names and resolve them to IDs
    #[arg(long)]
    pub by_name: bool,
    /// Instance name (alternative to positional URI)
    #[arg(long)]
    pub instance: Option<String>,
    /// Space name (used with --instance)
    #[arg(long)]
    pub space: Option<String>,
    /// Folder path (used with --instance --space)
    #[arg(long)]
    pub folder: Option<String>,
    /// File name (used with --instance --space --folder)
    #[arg(long)]
    pub file: Option<String>,
    /// Byte range (e.g., 0-1023)
    #[arg(long)]
    pub range: Option<String>,
}

#[derive(Debug, Clone, Args)]
#[command(
    after_help = "Stdin input is uploaded after EOF: press Ctrl+D on Unix/macOS, or Ctrl+Z then Enter on Windows. Ctrl+C cancels the command.\n\nExamples:\n  ve-adrive-cli cat adrive://inst/space/src.txt | gzip | ve-adrive-cli put adrive://inst/space/src.txt.gz\n  ve-adrive-cli put adrive://inst/space/docs/file.gz --content-type application/gzip\n  ve-adrive-cli put --instance inst --space space --folder docs --file file.gz --no-clobber"
)]
pub struct PutArgs {
    /// File path to write (adrive://instance/space/folder/file)
    #[arg(value_name = "PATH", conflicts_with_all = ["instance", "space", "folder", "file"])]
    pub path: Option<String>,
    /// Treat ADrive instance/space target segments as names and resolve them to IDs
    #[arg(long)]
    pub by_name: bool,
    /// Instance name (alternative to positional URI)
    #[arg(long)]
    pub instance: Option<String>,
    /// Space name (used with --instance)
    #[arg(long)]
    pub space: Option<String>,
    /// Folder path (used with --instance --space)
    #[arg(long)]
    pub folder: Option<String>,
    /// File name (used with --instance --space --folder)
    #[arg(long)]
    pub file: Option<String>,
    /// Content-Type for the uploaded file
    #[arg(long)]
    pub content_type: Option<String>,
    /// Stdin size threshold for switching to multipart upload (e.g., 20MB)
    #[arg(long)]
    pub multipart_threshold: Option<String>,
    /// Do not overwrite an existing file
    #[arg(long)]
    pub no_clobber: bool,
    /// Enable execution progress output even when stderr is not a TTY
    #[arg(long, conflicts_with = "no_progress")]
    pub progress: bool,
    /// Disable execution progress output
    #[arg(long, conflicts_with = "progress")]
    pub no_progress: bool,
}

#[derive(Debug, Clone, Args)]
#[command(
    after_help = "Examples:\n  ve-adrive-cli mkdir adrive://inst/space/new_folder/\n  ve-adrive-cli mkdir adrive://inst/space/a/b/c/ --parents\n  ve-adrive-cli mkdir --instance inst --space space --folder deep/nested/path"
)]
pub struct MkdirArgs {
    /// Folder path (adrive://instance/space/folder/subfolder)
    #[arg(value_name = "PATH", conflicts_with_all = ["instance", "space", "folder"])]
    pub path: Option<String>,
    /// Treat ADrive instance/space target segments as names and resolve them to IDs
    #[arg(long)]
    pub by_name: bool,
    /// Instance name (alternative to positional URI)
    #[arg(long)]
    pub instance: Option<String>,
    /// Space name (used with --instance)
    #[arg(long)]
    pub space: Option<String>,
    /// Folder path to create (used with --instance --space)
    #[arg(long)]
    pub folder: Option<String>,
    /// Create parent folders as needed
    #[arg(long, short = 'p')]
    pub parents: bool,
}
