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
ve-tos-cli cp ./file.txt tos://bucket/file.txt --dry-run --output json
ve-tos-cli cp tos://bucket/file.txt ./downloaded-file.txt --dry-run --output json
ve-tos-cli cp ./dir/ tos://bucket/backup/ --recursive --dry-run --output json
ve-tos-cli stat tos://bucket/file.txt --output json
```

<!-- [Review Fix #1] Include the implemented local-to-local transfer direction. -->
TOS `cp` supports local-to-local copy, local-to-TOS upload, TOS-to-local
download, and TOS-to-TOS copy; remote copies are restricted to same-region
buckets. For a single transfer, a remote destination
ending in `/` (or a bucket root) appends the source filename; an explicit object
key names the destination exactly. An existing local directory or a local path
ending in `/` likewise receives the source filename. Recursive copies preserve
relative paths. For transfers involving TOS, `--include-parent` additionally
includes the source directory or prefix name.
<!-- [Review Fix #2] Local destinations use the original relative path for local sources. -->
For local-to-local recursive copies, `--include-parent` does not change the
destination mapping; include the parent directory explicitly in the destination
path if required. Do not infer recursive layout from shell `cp` conventions.

Read filter and overwrite options from the contract instead of assuming shell
glob or sync semantics. Quote patterns such as `'*.log'` so the CLI receives
them unchanged. For large transfers, inspect `--checkpoint` and the checkpoint
location before selecting resumable transfer; retain state for a retry.

## Sync

Use `sync` for incremental reconciliation and `cp --recursive` for a recursive
copy. Do not add `--delete` unless removal of destination-only entries is part
of the user's request. Preview both directions and filters carefully:

```bash
ve-tos-cli sync ./dir/ tos://bucket/backup/ --dry-run --output json
ve-tos-cli sync tos://bucket/backup/ ./restore/ --dry-run --output json
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

## Signed download links

Use `ve-tos-cli presign` for an explicitly requested signed link. Check the
object with `stat`, inspect `presign --help`, and select the requested expiry.
Return the CLI's URL to the authorized requester with its expiry; keep it out
of logs. Generating a signature alone does not prove the object is readable.
