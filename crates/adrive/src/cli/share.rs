//! Share-link management and receiver commands with explicit OAuth opt-in.
use clap::{Args, Subcommand, ValueEnum};

pub(crate) const SHARE_VIEW_HELP: &str = "view previews one exact file; a single shared file is selected when --path is omitted. Folders and multiple roots require --path to a file. Default auto mode returns a short-lived PDF PreviewURL for supported documents, or raw content metadata for other files; it never opens a browser. --raw streams original bytes to a pipe or redirected stdout (for example | less), rejects terminal stdout and conflicts with --output/--query. Preview requests consume PreviewLimit and are never automatically retried. Document URLs are temporary capabilities; keep them private. Unsupported deployments can return 404. Password precedence is stdin > URL pwd > hidden TTY prompt; --password-stdin reads to EOF.";

pub(crate) const DEFAULT_CREATE_SHARE_PERMISSION: &str = "view";
// [Review Fix #WholeSpaceDocs] A root selector must disclose the broader V1 scope before creation.
pub(crate) const SHARE_CREATE_HELP: &str = "create shares one file, one folder or a whole Space. adrive://instance/space (with an optional trailing slash) selects the whole Space with ShareAllFiles=true and FilePathList=[]; --instance instance --space space selects the same root. Non-root file/folder targets omit ShareAllFiles (default false) and retain one FilePathList entry. Explicit empty --file values remain invalid.";
pub(crate) const SHARE_SETTINGS_HELP: &str = "Create defaults to view (preview only); use --permission download to allow downloads. Update preserves omitted settings. SharePwd and RequireLogin are server fields: --password-stdin sets a share password in create/update and supplies it in files/view/download. RequireLogin controls receiver login; management uses the selected account. Receivers default to anonymous; explicit --auth-mode oauth uses the selected recipient profile.";

// [Review Fix #ShareServerURLDocs] Publish the service URL source, override precedence and password boundary consistently.
pub(crate) const SHARE_CONNECTION_HELP: &str = "Management ShareUrl uses the server ShareURL unless share-endpoint is configured. Override precedence: --share-endpoint > selected profile share_endpoint > ADRIVE_SHARE_ENDPOINT > legacy ADRIVE_SHARE_BASE_URL; overrides append /share/<encoded ShareID> without adding an Instance label or password. Without a server URL or override, the successful ShareID is retained, ShareUrl is omitted and ShareUrlWarning explains the missing URL; the API endpoint is never used to construct it. Create/get may retain the server URL pwd query for authorized delivery; list/update remove all query parameters and omit passwords. Standard IDS receiver hosts map between idsapp/idsapi and idsapp-boe/idsapi-boe under volces.com, preserving the Instance label, scheme and port. Share pages, receiver APIs and download addresses support HTTP and HTTPS without protocol upgrades. Custom receiver APIs use --endpoint. Anonymous receivers derive APIs from page URLs without account config; explicit OAuth uses the selected profile API or --endpoint.";
pub(crate) const SHARE_RECEIVER_HELP: &str = "inspect/files/view/download default to anonymous even if a profile configures OAuth. Explicit --auth-mode oauth reuses --profile via auth login/status and existing token refresh; it never starts login or falls back to another identity. OAuth authenticates share info/token exchange; file APIs use only x-share-token and content downloads carry no account credentials. Login and share permissions are validated by the service; passwords are still required when configured. Public URLs accept /share/<id> or /share/<id>/; one trailing slash is normalized away. Select directory prefixes with --path, not additional URL segments.";
// [Review Fix #ShareFilesCap1] Files expose the same result cap as ls, independent of service page sizes.
// [Review Fix #WholeSpaceFilesHelp] Missing and explicit root paths must describe the same whole-Space listing.
pub(crate) const SHARE_FILES_HELP: &str = "files lists the current directory level like ls. --max-keys (default 1000) caps files and folders together; service pages are fetched automatically until the cap or listing end. Continue with --marker using the previous NextMarker; IsTruncated=false means the listing ended. --limit is not supported. Exact file paths select one file; directory paths end in /. For a whole-Space share, omitting --path or using --path / lists the Space root. Each invocation needs the extraction code when configured: stdin > URL pwd > hidden ordinary-TTY prompt. --password-stdin reads to EOF (use a pipe); passwords are not saved. Default table/CSV uses the same file_path, size, file_type, updated_at and is_folder columns as ls, with folders first. --human-readable (-H) formats table/CSV sizes and --columns selects display fields. Explicit --sort key/name|size|last-modified/mtime orders the current capped set in every output format and explicit queries, preserving raw metadata values and pagination markers. --manifest-path explicitly writes a listing CSV; files writes no manifest by default. Folder markers, self markers and duplicates are normalized in all output modes; Total and pagination.total_returned count the same logical entries. JSON and explicit --query retain the aggregated normalized Files/Folders metadata.";
// [Review Fix #ShareDownloadHelp1] Preserve shared selection and password guidance without advertising files-only pagination flags.
pub(crate) const SHARE_DOWNLOAD_SELECTION_HELP: &str = "Downloads enumerate all selected service pages automatically. Exact file paths select one file; directory paths end in / and require --recursive. Password precedence is stdin > URL pwd > hidden ordinary-TTY prompt; --password-stdin reads to EOF (use a pipe), and passwords are not saved.";
// [Review Fix #D03D04D06Docs] Local conflicts need local remedies; an ambiguous service denial cannot establish a unique cause.
// [Review Fix #WholeSpaceLayoutHelp] Root recursion preserves Space paths without inventing a parent basename.
pub(crate) const SHARE_DOWNLOAD_HELP: &str = "The OS resolves user-selected destination and checkpoint directory aliases before the CLI pins them; downloaded subpaths reject traversal, symlinks/reparse points and case/Unicode or file/directory collisions with Conflict (exit 9). Resume compares resolved roots and verifies completed local sizes/digests; changes fail with Conflict. No --path selects all shared roots; a single-file share is selected automatically, while folders require --recursive. Directory contents map like cp by default; --include-parent retains the selected directory name. For a whole-Space share, omitting --path or using --path / selects the entire Space and requires --recursive. It preserves the full Space-relative hierarchy; --include-parent adds no wrapper because the Space root has no basename. A selected subdirectory keeps the cp layout rules. Multiple roots with colliding contents fail safely; use --include-parent to preserve distinct root names instead of guessing new names. --include and --exclude match cp relative source paths. Default parallelism is 16 files, 4 directory listings and 4 ranges per file; --batch-concurrency, --list-concurrency and --multipart-concurrency override these positive values.\n\nOverwrite defaults to force, matching cp. --force and legacy --overwrite select force; --no-clobber skips existing ordinary files, and --overwrite-strategy newer replaces only older destinations. Replacements publish only after verification. --checkpoint [FILE] enables recovery; a bare flag uses a stable task path under ~/.tos/checkpoints/ve-adrive, --checkpoint-dir overrides that directory, and FILE preserves the legacy explicit path. --resume uses the saved selection and policy, revalidates completed files and restores verified partial ranges. Keep selection, filters, layout and overwrite policy unchanged; old checkpoints with the directory-name layout need --include-parent. New remote files are not added during resume. Keep the checkpoint JSON together with its .completed/ completion-ledger directory and .parts/ directory when retaining or moving recovery state; completed entries in the main JSON are not updated after every file.\n\nActual downloads create directories, temporary files, default report/manifest CSVs and enabled checkpoint/lock state; preflight may leave empty directories before a later denial. --report-path and --manifest-path override defaults under ~/.tos/reports/ve-adrive; --report-failures-only filters report rows only. --no-manifest disables the manifest CSV while preserving the safety pre-scan and recovery snapshot. Aggregate data includes report_path, manifest_path and checkpoint_path. Report CSV rows retain the request_id of their own operation; artifacts exclude passwords, tokens and signed URLs. The default table shows a cp-style summary of total/succeeded/failed/skipped/pending; Summary.Skipped includes verified resumed files and overwrite-policy skips, is included in Summary.Completed, and succeeded=Completed-Skipped. Files.Status may be completed, failed, pending or skipped; JSON and explicit queries retain details.\n\nFiles at least 20MB with a strong ETag or CRC64 automatically use parallel 16MiB ranges without requiring checkpoint; --checkpoint-threshold changes the threshold. Files lacking both identities use one streamed GET. Content retries reuse the issued URL. Ordinary file failures allow other files to continue. DirectoryFailures lists directory errors separately, with failed mkdir report rows; Summary counts files. An unknown URL-issuance outcome stops new dispatch and is never automatically replayed. Issuance may consume download-count quota even when content fails. Explicit service denials such as 403 retain the service code/status/request ID and set IssuanceOutcomeUnknown=false. Forbidden.ShareDownloadDisabled can mean downloads are forbidden or the quota is exhausted; ask the owner to check both.\n\nListing echo and file-count progress use stderr and default to TTY. --list-echo/--no-list-echo control listing echo; --progress/--no-progress control progress; --quiet disables both. Opposing echo/progress flags conflict. --progress-granularity part (default) or byte selects transfer progress units. --bandwidth-limit applies one bytes-per-second limit across all files/ranges; 0 is unlimited. Local transfer settings use CLI > selected profile adrive/shared transfer fields > ADRIVE environment > defaults, without loading account credentials or changing receiver authentication.";
// [Review Fix #ShareReviewDocs] Keep read-only planning and explicit input validation visible in every discovery surface.
pub(crate) const SHARE_DRY_RUN_HELP: &str = "Download --dry-run uses read-only share info/token/list/get metadata requests to enumerate every selected page and recursive folder, or validate the saved resume snapshot and completed local file digests. It returns selection.Manifest, PlanComplete=true, FileCount, DirectoryCount, TotalBytes, SkippedCount and Conflicts, plus expected report_path, manifest_path and checkpoint_path. Completed plans with local conflicts exit 9 and retain the plan. DownloadsIssued=0; LocalWrites=false means no local writes: no URL issuance, content fetch, directory creation, temporary file, checkpoint, lock, report or manifest writes. MetadataResponses counts received metadata responses; NetworkRequests=null because wire attempts are not counted. SnapshotGuaranteed=false: remote content and local paths can change after inspection. Missing or negative file sizes fail validation (exit 6); explicit zero is a valid empty file. OAuth dry-run disables automatic refresh to avoid credential and auth-lock writes. If the token is expired, within 60 seconds of expiry, or rejected with 401, first run ve-adrive --profile <profile> share inspect <share-url> --auth-mode oauth as a normal OAuth read to refresh, then retry. Planning never acquires the checkpoint lock; an existing regular sidecar may be active. Execution rechecks state and acquires its lock.";
pub(crate) const SHARE_PASSWORD_HELP: &str = "Explicit --password-stdin must contain a nonempty password after removing one terminal LF or CRLF; EOF-only, LF-only and CRLF-only input fail validation (exit 6). Significant spaces are preserved. Omitting the flag leaves an existing password unchanged on update. create/update --dry-run also reads and validates explicit stdin; --describe and --help never read stdin.";
pub(crate) const SHARE_OFFLINE_HELP: &str = "--describe and --help do not load config, access the network or read stdin. Share --dry-run is offline except download metadata planning. Explicit create/update --password-stdin is read and validated during dry-run.";
pub(crate) const SHARE_SELECTOR_HELP: &str = "get/update/cancel accept adrive://instance/share-id or a bare ShareID with --instance. A repeated --instance must match the URI; --by-name resolves only the instance. Share URIs use one ShareID segment and decode components once; file/space targets are interpreted by their own command context.";
pub(crate) const SHARE_LIST_HELP: &str = "list accepts adrive://instance (with an optional root slash) or --instance instance. A repeated --instance must match the URI; --by-name resolves the instance name. The instance URI has no share/space/file path and decodes once. List returns one page; --creator is optional only when OAuth supplies a bound user_id.";

/// Share-link command group; an omitted action displays group help.
#[derive(Debug, Args)]
pub struct ShareCommand {
    #[command(subcommand)]
    pub action: Option<ShareAction>,
}

/// Supported share operations.
#[derive(Debug, Subcommand)]
pub enum ShareAction {
    /// Create a share link
    #[command(after_help = format!("{SHARE_CONNECTION_HELP}\n\n{SHARE_CREATE_HELP}\n\n{SHARE_SETTINGS_HELP}\n\n{SHARE_PASSWORD_HELP}\n\n{SHARE_OFFLINE_HELP}"))]
    Create(CreateShareArgs),
    /// Get a share link
    #[command(after_help = format!("{SHARE_CONNECTION_HELP}\n\n{SHARE_SELECTOR_HELP}\n\n{SHARE_OFFLINE_HELP}"))]
    Get(ShareIdArgs),
    /// List share links
    #[command(after_help = format!("{SHARE_CONNECTION_HELP}\n\n{SHARE_LIST_HELP}\n\n{SHARE_OFFLINE_HELP}"))]
    List(ListShareArgs),
    /// Update share settings
    #[command(after_help = format!("{SHARE_CONNECTION_HELP}\n\n{SHARE_SELECTOR_HELP}\n\n{SHARE_SETTINGS_HELP}\n\n{SHARE_PASSWORD_HELP}\n\n{SHARE_OFFLINE_HELP}"))]
    Update(UpdateShareArgs),
    /// Cancel a share link
    #[command(after_help = format!("{SHARE_SELECTOR_HELP}\n\n{SHARE_OFFLINE_HELP}"))]
    Cancel(CancelShareArgs),
    /// Inspect share info
    #[command(after_help = format!("{SHARE_CONNECTION_HELP}\n\n{SHARE_RECEIVER_HELP}\n\n{SHARE_OFFLINE_HELP}"))]
    Inspect(InspectShareArgs),
    /// List shared files
    #[command(after_help = format!("{SHARE_CONNECTION_HELP}\n\n{SHARE_RECEIVER_HELP}\n\n{SHARE_FILES_HELP}\n\n{SHARE_PASSWORD_HELP}\n\n{SHARE_OFFLINE_HELP}"))]
    Files(ShareFilesArgs),
    /// Preview one shared file
    #[command(after_help = format!("{SHARE_CONNECTION_HELP}\n\n{SHARE_RECEIVER_HELP}\n\n{SHARE_VIEW_HELP}\n\n{SHARE_PASSWORD_HELP}\n\n{SHARE_OFFLINE_HELP}"))]
    View(ShareViewArgs),
    /// Download shared files
    #[command(after_help = format!("{SHARE_CONNECTION_HELP}\n\n{SHARE_RECEIVER_HELP}\n\n{SHARE_DOWNLOAD_SELECTION_HELP}\n\n{SHARE_DOWNLOAD_HELP}\n\n{SHARE_DRY_RUN_HELP}\n\n{SHARE_PASSWORD_HELP}\n\n{SHARE_OFFLINE_HELP}"))]
    Download(ShareDownloadArgs),
}

impl ShareAction {
    /// Return the canonical leaf name without user arguments.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Create(_) => "create",
            Self::Get(_) => "get",
            Self::List(_) => "list",
            Self::Update(_) => "update",
            Self::Cancel(_) => "cancel",
            Self::Inspect(_) => "inspect",
            Self::Files(_) => "files",
            Self::View(_) => "view",
            Self::Download(_) => "download",
        }
    }
}

/// Permission granted by a share link.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum SharePermission {
    View,
    Download,
}
impl SharePermission {
    /// Return the V1 wire value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::View => "view",
            Self::Download => "download",
        }
    }
}

/// Searchable lifecycle status.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ShareStatus {
    Active,
    Expired,
    Revoked,
}
impl ShareStatus {
    /// Return the V1 wire value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Expired => "expired",
            Self::Revoked => "revoked",
        }
    }
}

/// Optional mutable share settings; explicit zero, false and empty strings are retained.
#[derive(Debug, Clone, Default, Args)]
pub struct ShareSettings {
    /// Share name
    #[arg(long)]
    pub name: Option<String>,
    /// Share description
    #[arg(long)]
    pub description: Option<String>,
    /// Share permission: view or download
    #[arg(long, value_enum)]
    pub permission: Option<SharePermission>,
    /// Share lifetime duration; 0 means never expires
    #[arg(long)]
    pub expires_in: Option<String>,
    /// Read one share password from stdin (EOF; use a pipe or Enter then Ctrl-D on macOS/Linux)
    #[arg(long = "password-stdin")]
    pub is_password_stdin: bool,
    /// Require receiver login; accepts explicit true or false
    #[arg(long = "require-login", action = clap::ArgAction::Set)]
    pub is_login_required: Option<bool>,
    /// Maximum preview count; 0 means unlimited
    #[arg(long, value_parser = clap::value_parser!(i64).range(0..))]
    pub preview_limit: Option<i64>,
    /// Maximum download count; 0 means unlimited
    #[arg(long, value_parser = clap::value_parser!(i64).range(0..))]
    pub download_limit: Option<i64>,
}

/// Create a share for one file, one folder or a whole Space.
#[derive(Debug, Args)]
pub struct CreateShareArgs {
    /// ADrive file, folder or Space root target
    #[arg(conflicts_with_all = ["instance", "space", "folder", "file"])]
    pub target: Option<String>,
    /// ADrive instance identifier
    #[arg(long)]
    pub instance: Option<String>,
    /// ADrive space identifier
    #[arg(long)]
    pub space: Option<String>,
    /// Folder path inside the space
    #[arg(long)]
    pub folder: Option<String>,
    /// File name inside the folder
    #[arg(long)]
    pub file: Option<String>,
    /// Treat ADrive instance/space target segments as names and resolve them to IDs
    #[arg(long = "by-name")]
    pub is_by_name: bool,
    /// Share page origin override; otherwise use server ShareURL
    // [Review Fix #ShareEndpoint2] Keep existing invocations valid without advertising the former option name.
    #[arg(long, alias = "share-base-url")]
    pub share_endpoint: Option<String>,
    #[command(flatten)]
    pub settings: ShareSettings,
}

/// Existing share selector.
#[derive(Debug, Args)]
pub struct ShareIdArgs {
    /// Share identifier or adrive://instance/share-id
    pub share_id: String,
    /// ADrive instance identifier; required for a bare ShareID, optional for a share URI
    #[arg(long)]
    pub instance: Option<String>,
    /// Treat the share target instance as a name and resolve it to an ID
    #[arg(long = "by-name")]
    pub is_by_name: bool,
    /// Share page origin override; otherwise use server ShareURL
    #[arg(long, alias = "share-base-url")]
    pub share_endpoint: Option<String>,
}

/// Cancellation selector; revocation requires no share-page origin.
#[derive(Debug, Args)]
pub struct CancelShareArgs {
    /// Share identifier or adrive://instance/share-id
    pub share_id: String,
    /// ADrive instance identifier; required for a bare ShareID, optional for a share URI
    #[arg(long)]
    pub instance: Option<String>,
    /// Treat the share target instance as a name and resolve it to an ID
    #[arg(long = "by-name")]
    pub is_by_name: bool,
}

/// Search one page of share links.
#[derive(Debug, Args)]
pub struct ListShareArgs {
    /// ADrive instance target: adrive://instance
    pub target: Option<String>,
    /// ADrive instance identifier; required unless an instance URI is supplied
    #[arg(long)]
    pub instance: Option<String>,
    /// Treat the share target instance as a name and resolve it to an ID
    #[arg(long = "by-name")]
    pub is_by_name: bool,
    /// Creator user identifier; repeat for multiple creators
    #[arg(long)]
    pub creator: Vec<String>,
    /// Share lifecycle status
    #[arg(long, value_enum)]
    pub status: Option<ShareStatus>,
    /// Share name
    #[arg(long)]
    pub name: Option<String>,
    /// Creation time range start in RFC3339
    #[arg(long)]
    pub created_at_start: Option<String>,
    /// Creation time range end in RFC3339
    #[arg(long)]
    pub created_at_end: Option<String>,
    /// Search offset, at most 10000
    #[arg(long, value_parser = clap::value_parser!(i64).range(0..=10000))]
    pub offset: Option<i64>,
    /// Maximum entries in this page
    #[arg(long, value_parser = clap::value_parser!(i64).range(1..))]
    pub limit: Option<i64>,
    /// Share page origin override; otherwise use server ShareURL
    #[arg(long, alias = "share-base-url")]
    pub share_endpoint: Option<String>,
}

/// Update one share with an optional optimistic concurrency guard.
#[derive(Debug, Args)]
pub struct UpdateShareArgs {
    #[command(flatten)]
    pub selector: ShareIdArgs,
    /// Optimistic concurrency timestamp; omitted fetches the current value
    #[arg(long)]
    pub updated_at: Option<i64>,
    #[command(flatten)]
    pub settings: ShareSettings,
}

/// Public share page selector.
#[derive(Args)]
pub struct InspectShareArgs {
    /// Public share URL (one optional trailing slash); optional pwd query is kept secret
    pub share_url: String,
}

/// Receiver file selection with a combined current-level result cap.
#[derive(Args)]
pub struct ShareFilesArgs {
    /// Public share URL (one optional trailing slash); optional pwd query is kept secret
    pub share_url: String,
    /// Exact file path or directory path ending in slash
    #[arg(long)]
    pub path: Option<String>,
    /// Pagination marker returned by a previous listing
    #[arg(long)]
    pub marker: Option<String>,
    /// Maximum entries to return from the current directory level
    #[arg(long, default_value = "1000", value_parser = clap::value_parser!(i32).range(1..))]
    pub max_keys: i32,
    /// Human-readable sizes
    #[arg(long, short = 'H')]
    pub human_readable: bool,
    /// Sort field: key/name, size, or last-modified/mtime
    #[arg(long)]
    pub sort: Option<String>,
    /// Select columns for table/csv output (comma-separated)
    #[arg(long)]
    pub columns: Option<String>,
    /// Write listing manifest to this path. No manifest is written unless this is set.
    #[arg(long)]
    pub manifest_path: Option<String>,
    /// Read the receiver password from stdin; it does not change the share password (EOF; use a pipe or Enter then Ctrl-D on macOS/Linux)
    #[arg(long = "password-stdin")]
    pub is_password_stdin: bool,
}

/// Preview selector; raw output requires a pipe or redirected stdout.
#[derive(Args)]
pub struct ShareViewArgs {
    /// Public share URL (one optional trailing slash); optional pwd query is kept secret
    pub share_url: String,
    /// Exact file path; omitted selects a single shared file
    #[arg(long)]
    pub path: Option<String>,
    /// Stream original bytes to a pipe or redirected stdout
    #[arg(long = "raw")]
    pub is_raw: bool,
    /// Read the receiver password from stdin; it does not change the share password (EOF; use a pipe or Enter then Ctrl-D on macOS/Linux)
    #[arg(long = "password-stdin")]
    pub is_password_stdin: bool,
}

impl std::fmt::Debug for ShareViewArgs {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let safe_url = crate::domain::share_url::parse_share_url(&self.share_url)
            .map(|parsed| parsed.safe_url)
            .unwrap_or_else(|_| "[invalid share URL]".into());
        formatter
            .debug_struct("ShareViewArgs")
            .field("share_url", &safe_url)
            .field("is_raw", &self.is_raw)
            .finish_non_exhaustive()
    }
}

/// Download selection; destination always names a directory.
#[derive(Clone, Args, Default)]
pub struct ShareDownloadArgs {
    /// Public share URL (one optional trailing slash); optional pwd query is kept secret
    pub share_url: String,
    /// Destination directory for shared files
    pub destination_dir: String,
    /// Exact file path or directory path ending in slash
    #[arg(long)]
    pub path: Option<String>,
    /// Recursively download selected folders
    #[arg(long = "recursive")]
    pub is_recursive: bool,
    /// Include the source directory/prefix name under the destination path
    #[arg(long = "include-parent", requires = "is_recursive")]
    pub is_include_parent: bool,
    /// Include pattern, matching cp relative source paths
    #[arg(long)]
    pub include: Option<String>,
    /// Exclude pattern, matching cp relative source paths
    #[arg(long)]
    pub exclude: Option<String>,
    /// Legacy alias for force overwrite (the default strategy)
    #[arg(long = "overwrite")]
    pub is_overwrite: bool,
    /// Force overwrite, matching cp's default strategy
    #[arg(long = "force", conflicts_with = "is_no_clobber")]
    pub is_force: bool,
    /// Do not overwrite existing files
    #[arg(long = "no-clobber", conflicts_with_all = ["is_force", "is_overwrite"])]
    pub is_no_clobber: bool,
    /// Destination overwrite strategy
    #[arg(long, value_enum)]
    pub overwrite_strategy: Option<super::high_level::OverwriteStrategy>,
    /// Read the receiver password from stdin; it does not change the share password (EOF; use a pipe or Enter then Ctrl-D on macOS/Linux)
    #[arg(long = "password-stdin")]
    pub is_password_stdin: bool,
    /// Enable recovery with a stable task checkpoint; optionally supply an explicit FILE
    #[arg(long, num_args = 0..=1, default_missing_value = "__auto__")]
    pub checkpoint: Option<String>,
    /// Directory for automatic task checkpoints (default ~/.tos/checkpoints/ve-adrive)
    #[arg(long)]
    pub checkpoint_dir: Option<String>,
    /// Automatically range-download files at this size (default 20MB), even without a checkpoint
    #[arg(long)]
    pub checkpoint_threshold: Option<String>,
    /// Maximum ranges transferred concurrently for one file (default 4)
    #[arg(long)]
    pub multipart_concurrency: Option<usize>,
    /// Maximum files/items running concurrently (default 16)
    #[arg(long)]
    pub batch_concurrency: Option<usize>,
    /// Maximum directory prefixes listed concurrently (default 4)
    #[arg(long)]
    pub list_concurrency: Option<usize>,
    /// Progress granularity: part (default) or byte
    #[arg(long, value_enum)]
    pub progress_granularity: Option<super::high_level::ProgressGranularity>,
    /// Shared bytes-per-second limit for all files and ranges (for example 100MB; 0 is unlimited)
    #[arg(long)]
    pub bandwidth_limit: Option<String>,
    /// Write batch success/failure report to this path
    #[arg(long)]
    pub report_path: Option<String>,
    /// Write only failed items to the batch report
    #[arg(long = "report-failures-only")]
    pub is_report_failures_only: bool,
    /// Write planned transfer manifest to this path
    #[arg(long, conflicts_with = "is_no_manifest")]
    pub manifest_path: Option<String>,
    /// Disable the manifest CSV; preserve the safety pre-scan and recovery snapshot
    #[arg(long = "no-manifest", conflicts_with = "manifest_path")]
    pub is_no_manifest: bool,
    /// Enable listing echo even when stderr is not a TTY
    #[arg(long = "list-echo", conflicts_with = "is_no_list_echo")]
    pub is_list_echo: bool,
    /// Disable listing echo
    #[arg(long = "no-list-echo", conflicts_with = "is_list_echo")]
    pub is_no_list_echo: bool,
    /// Resume using the checkpoint file
    #[arg(long = "resume", requires = "checkpoint")]
    pub is_resume: bool,
    /// Enable execution progress output even when stderr is not a TTY
    #[arg(long = "progress", conflicts_with = "is_no_progress")]
    pub is_progress: bool,
    /// Disable execution progress output
    #[arg(long = "no-progress", conflicts_with = "is_progress")]
    pub is_no_progress: bool,
}

// [Review Fix #ShareSecret2] Debug must never echo URL query passwords.
impl std::fmt::Debug for InspectShareArgs {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let safe_url = crate::domain::share_url::parse_share_url(&self.share_url)
            .map(|parsed| parsed.safe_url)
            .unwrap_or_else(|_| "[invalid share URL]".into());
        formatter
            .debug_struct("InspectShareArgs")
            .field("share_url", &safe_url)
            .finish_non_exhaustive()
    }
}

// [Review Fix #ShareSecret2] Debug must never echo URL query passwords.
impl std::fmt::Debug for ShareFilesArgs {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let safe_url = crate::domain::share_url::parse_share_url(&self.share_url)
            .map(|parsed| parsed.safe_url)
            .unwrap_or_else(|_| "[invalid share URL]".into());
        formatter
            .debug_struct("ShareFilesArgs")
            .field("share_url", &safe_url)
            .finish_non_exhaustive()
    }
}

// [Review Fix #ShareSecret2] Debug must never echo URL query passwords.
impl std::fmt::Debug for ShareDownloadArgs {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let safe_url = crate::domain::share_url::parse_share_url(&self.share_url)
            .map(|parsed| parsed.safe_url)
            .unwrap_or_else(|_| "[invalid share URL]".into());
        formatter
            .debug_struct("ShareDownloadArgs")
            .field("share_url", &safe_url)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod files_pagination_tests {
    use super::*;

    #[test]
    fn files_accepts_max_keys_and_rejects_removed_limit() {
        let command = ShareFilesArgs::augment_args(clap::Command::new("files"));
        let parsed = command.clone().try_get_matches_from([
            "files",
            "https://example.test/share/s",
            "--max-keys",
            "4",
        ]);
        assert!(
            parsed.is_ok(),
            "--max-keys must select the result cap: {parsed:?}"
        );
        assert!(
            command
                .try_get_matches_from(["files", "https://example.test/share/s", "--limit", "4",])
                .is_err(),
            "removed --limit must not remain an alias"
        );
    }
}
