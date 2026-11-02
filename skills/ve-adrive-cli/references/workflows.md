# Task workflows

Read the command's installed `--help` and `--describe` before adapting these
examples. Replace example targets with the user's actual paths. Establish
profile, authentication, endpoint, and target scope using SKILL.md first.

## Upload and download

1. Identify the direction, exact destination, and whether the source is one
   file or a directory/prefix. Inspect remote state with `ls` or `stat`, and
   inspect local file existence and available disk space for a download.
2. Use explicit filenames for a single transfer to avoid ambiguous destination
   mapping. For a directory/prefix, use `cp --recursive`; inspect the mapping
   with `--dry-run` before executing.
3. Check planned writes, skipped entries, and warnings. A dry-run is a preview,
   not evidence that objects were transferred or that every runtime permission
   will succeed. Execute the same scoped command without `--dry-run` once it
   matches the user's request.
4. Verify the destination and summarize transferred, skipped, and failed items.

```bash
ve-adrive-cli cp ./file.txt adrive://instance-id/space-id/file.txt --dry-run --output json
ve-adrive-cli cp adrive://instance-id/space-id/file.txt ./downloaded-file.txt --dry-run --output json
ve-adrive-cli cp ./dir/ adrive://instance-id/space-id/backup/ --recursive --dry-run --output json
ve-adrive-cli stat adrive://instance-id/space-id/file.txt --output json
```

Use `adrive://instance-id/space-id/path` with discovered IDs. Use `--by-name`
only when intentionally resolving names; do not silently interpret an unknown
ID as a name. `--include-parent` on recursive copy includes the source directory
or prefix name. Inspect the dry-run mapping before relying on trailing-slash
behavior or replacing existing files. Resolve Space ownership and pagination
using the corresponding SKILL.md sections before selecting a destination.

Read filter and overwrite options from the contract instead of assuming shell
glob or sync semantics. Quote patterns such as `'*.log'` so the CLI receives
them unchanged. For large transfers, inspect `--checkpoint` and the checkpoint
location before selecting resumable transfer; retain state for a retry.

## Sync

Use `sync` for incremental reconciliation and `cp --recursive` for a recursive
copy. Do not add `--delete` unless removal of destination-only entries is part
of the user's request. Preview both directions and filters carefully:

```bash
ve-adrive-cli sync ./dir/ adrive://instance-id/space-id/backup/ --dry-run --output json
ve-adrive-cli sync adrive://instance-id/space-id/backup/ ./restore/ --dry-run --output json
```

`sync` has no `--recursive` flag. Check its comparison options before choosing
size-only or timestamp policies. When using `--delete`, inspect the destination
scope and planned removals and preserve the contract's required force and
confirmation flags. After execution, read the result/report and rerun a dry-run
with the same comparison options to identify remaining work. A zero-work plan
is evidence under those comparison rules, not a byte-for-byte integrity proof.

## Delete

List the exact target first. Distinguish one object/file from a prefix/folder;
recursive deletion expands the affected set. Read `rm --help` and `rm --describe`
with the target and preview the operation with `--dry-run` when supported.
Only execute within the already authorized scope, preserving required `--force`
and exact `--confirm` targets. Do not add broader flags to bypass a refusal.
For `mv`, verify the destination and source-removal result because moving
includes a destructive source operation. Re-list the affected scope afterward;
an authorization or network error is not proof of absence.

## Failure and verification

- Check exit status as well as structured stdout and diagnostics on stderr.
  A batch may have completed some items before failing: inspect failed/skipped
  counts and reports before claiming success or retrying the entire batch.
- Validation errors: correct target syntax or unsupported flag combinations
  using local help. Authentication/permission errors: check the selected mode,
  profile, endpoint, and access to the exact target; do not switch credentials
  or widen permissions as a workaround.
- Network errors: inspect current state and retry only operations whose outcome
  is known or safely repeatable. Use supported checkpoints/reports; do not
  implement an unbounded retry loop or restart destructive work blindly.
- Upload/copy: `stat` the destination and compare expected size and relevant
  metadata. Download: verify the local file exists and its expected size; use
  an independently known checksum when integrity verification is required.
  Do not assume an ETag is a full-file checksum.
- Report the exact target, executed versus preview-only status, success/skip/
  failure counts when available, and any incomplete verification. Redact
  credentials and signed query strings from diagnostic excerpts.

## Share management and receiving

```bash
ve-adrive-cli share create adrive://instance/space/docs/ --name Docs --expires-in 7d --permission download
ve-adrive-cli share create adrive://instance/space --permission download
ve-adrive-cli share create --instance instance --space space --permission download
ve-adrive-cli share get share-id --instance instance
ve-adrive-cli share get adrive://instance/share-id
ve-adrive-cli share list adrive://instance
ve-adrive-cli share list --instance instance --creator user-id --status active --limit 100
ve-adrive-cli share update share-id --instance instance --require-login false --expires-in 0
ve-adrive-cli share cancel share-id --instance instance --yes
ve-adrive-cli share inspect https://instance.example.test/share/share-id
ve-adrive-cli share files https://instance.example.test/share/share-id --path docs/ --max-keys 100
ve-adrive-cli share files https://instance.example.test/share/share-id --path / --max-keys 100
ve-adrive-cli share view https://instance.example.test/share/share-id --path docs/report.docx
ve-adrive-cli share view https://instance.example.test/share/share-id --path readme.txt --raw | less
ve-adrive-cli share download https://instance.example.test/share/share-id ./downloads --recursive --checkpoint
ve-adrive-cli share download https://instance.example.test/share/share-id ./downloads --path / --recursive
```

Whole-Space sharing is conditional on backend support. BOE V1 currently rejects
creation with `InvalidParameter.ShareAllFilesNotAllowed`; local fixture coverage
does not establish real whole-Space creation, listing or download availability.
Preserve the service rejection instead of inventing a placeholder path or a
collection of ordinary shares to bypass it.

1. Confirm the exact file, folder or whole Space scope before create. A Space root
   URI (optional trailing slash), or `--instance instance --space space`, uses
   `ShareAllFiles=true` and `FilePathList=[]`; a non-root target remains one path.
   Explicit empty `--file` values remain invalid. Use `--dry-run` to inspect
   selectors without acquiring or displaying secrets. Management results use the
   service's validated `ShareURL` as public `ShareUrl` by default; the API endpoint
   does not construct a page URL. To override the page origin, use
   `--share-endpoint` > profile `share_endpoint` > `ADRIVE_SHARE_ENDPOINT` > legacy
   `ADRIVE_SHARE_BASE_URL`. The override appends `/share/<encoded ShareID>` without
   adding an Instance label or password. `config show` reports the override source.
   Receiver-side standard public/BOE `idsapp` pages map to `idsapi` hosts under
   `volces.com`, preserving Instance, scheme and port. Other hosts, including
   `adriveapp`, are unchanged. Custom receiver APIs use explicit `--endpoint`.
   Anonymous receipt never loads account configuration; explicit `--auth-mode oauth`
   uses the selected recipient profile API or `--endpoint`.
2. Inspect returned `ShareID` and optional `ShareUrl`. Missing `ShareID` after create
   means `share_result_unknown`; reconcile with get/list before another create.
   Missing server URL without an override instead preserves success and `ShareID`,
   omits `ShareUrl`, and supplies `ShareUrlWarning`. Configure `share_endpoint` or
   retrieve the existing share again; do not create a duplicate to obtain a URL.
   Manage an existing share through `adrive://instance/share-id` or the compatible
   `share-id --instance instance` form; get/update/cancel all support both.
   Duplicate instances must agree, and `--by-name` resolves the Instance only.
   List shares in an Instance with `adrive://instance` (optional root slash), or
   `--instance instance`; both support the same filters and one-page pagination.
   Omit `--creator` only when OAuth supplies a bound `user_id`.
   Actual create/get results may include the service-returned `SharePwd` and
   retain the server URL's encoded `pwd` query for authorized delivery. Treat the
   full URL and password as secrets; exclude them from logs and plans. List/update
   omit passwords and strip all URL queries. Diagnostics omit account tokens and
   raw response URLs.
3. For update, omit `--updated-at` to fetch the current concurrency guard, or use
   an explicitly known guard. A conflict requires a fresh read. Preserve explicit
   zero/false/empty settings. Password clearing is not confirmed by the PRD;
   do not promise that an empty input clears a service password.
4. <!-- [Review Fix #WholeSpaceWorkflowCap] Keep root selection and files pagination aligned with the live contract. -->
   Anonymous inspect does not require account setup. For files, choose exact paths
   or trailing-slash folders. On a whole-Space share, omit `--path` or use `--path /`
   for the Space root. `--max-keys` caps combined files/folders (default 1000);
   service pages are fetched automatically. Continue with `--marker` from
   `NextMarker`, ending at `IsTruncated=false`. Anonymous overview metadata may
   omit `ShareAllFiles`; only explicit scope denial permits read-only root
   adaptation, and arbitrary 403 responses preserve service errors. Protected shares
   need the extraction code for every independent call/page; stdin is read to EOF
   and is best supplied by a pipe. Never echo or save it. Login-required receipt
   uses an explicitly selected recipient profile: run `auth login --instance <id>`
   and `auth status`, then pass `--auth-mode oauth --profile receiver` on
   inspect/files/view/download. OAuth does not bypass passwords. Never switch identity
   or retry anonymously automatically after rejection.
5. `share view` selects one exact file. Omit `--path` only for a single-file share.
   Default auto returns a temporary PDF descriptor for supported PDF/Office files,
   or raw metadata for other files. Keep `PreviewURL` private and respect expiry;
   no browser opens. Use `--raw` through a pipe/redirection, without `--output`
   or `--query`. Preview consumes `PreviewLimit` and never automatically retries.
   A 404 may mean the deployment does not provide preview.
6. Downloads resolve the user-selected local root with the OS (including `/tmp`,
   `/var` or configured aliases), then pin it and protect downloaded subpaths.
   Select recursion explicitly; directory contents map like cp, and --include-parent
   retains the directory name. For a whole-Space share, omit `--path` or use
   `--path /` with `--recursive` to retain the full Space-relative hierarchy.
   The Space root has no basename, so `--include-parent` adds no wrapper; selected
   subdirectories retain cp layout rules. Force overwrite is the default; use --no-clobber
   or --overwrite-strategy newer when needed. Keep checkpoint state for verified
   file/range recovery. Inspect completed, failed, pending and skipped items.

Resume the exact same share download with the same destination and policy:

```bash
ve-adrive-cli share download https://instance.example.test/share/share-id ./downloads --recursive --checkpoint --resume
```

The original manifest is reused; newly added remote files are excluded. Changed
remote/local files fail validation. Completed files require strong remote ETag or
CRC64 plus the recorded local SHA-256. Verified partial ranges are restored; unverified or changed ranges are fetched again. Inspect `Complete`, `PlanComplete`, `CheckpointAvailable`, `Summary`, and
`Files[].Status`; nonzero exit means interruption or failure. If a signed URL's
issuance outcome is unknown, check server state/quota before explicit resume.
An explicit download-disabled 403 instead retains `Forbidden.ShareDownloadDisabled`,
exits 5, and sets `IssuanceOutcomeUnknown=false`; changing local paths or blindly
resuming does not grant download permission. Checkpoint/lock collision checks and
resume identity include resolved local aliases.
Download --dry-run performs read-only share metadata requests and returns the full
selection.Manifest, counts, conflicts and expected artifact paths without local
writes. It issues no download URLs and fetches no file content. Other share dry-runs
remain offline.

For a retained legacy directory-name layout, keep `--checkpoint FILE`, original
filters/overwrite policy and `--include-parent` during resume. A bare checkpoint
flag uses the stable default task path; `--checkpoint-dir` changes its directory.
Download defaults are 16 files / 4 listings / 4 ranges; concurrency flags override
them. `--include`/`--exclude` filter relative source paths, and
`--bandwidth-limit 0` is unlimited. CSV reports/manifests use default paths unless
overridden; `--no-manifest` disables only the CSV, retaining pre-scan/recovery state.
Check `data.report_path`, `data.manifest_path`, `data.checkpoint_path`, and each
report row's operation-specific request_id. Ordinary file failures allow the batch
to continue; unknown issuance stops new dispatch.

Directory errors appear separately in `DirectoryFailures` (LocalPath/Path/Message)
and failed `mkdir` report rows; `Summary` continues to count files.

Multiple shared roots can collide when copying their contents. Use
`--include-parent` to preserve distinct root names; the CLI rejects ambiguous
mappings and never guesses replacement names.

Share recovery state consists of the checkpoint JSON, `<checkpoint>.completed/`
completion-ledger directory, and `<checkpoint>.parts/` range state. Retain or move them
together; the main JSON completed array is not updated after each file and cannot
replace the completion ledger. Report rows still expose operation-specific request IDs.
