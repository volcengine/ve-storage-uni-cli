## Copy behavior and decisions

### Transfer direction and limits

- Supports local-to-local, local upload to `tos://bucket/key`, remote download, and server-side remote copy. Different remote buckets must be in the same region; automatic cross-region relay is not supported. Use an explicitly planned download then upload for that case.
- This surface rejects `--storage-class` and uses hierarchical recursive listing with delimiter `/`; `--recursive-list-mode auto` and `flat` are not supported.

### File names, directories, and filters

- Without `--recursive`, use a single file/object source. A remote destination at the bucket root or ending in `/` appends the source file name; a destination with a final file name uses that exact name. An existing local directory or local target ending in `/` also appends the source name.
- Use `--recursive` for a local directory or remote prefix/folder. By default its contents are placed under the destination. `--include-parent` requires `--recursive` and retains the source's final directory/prefix segment; a source root with no final segment cannot use it. The source's trailing slash alone does not request inclusion of that segment.
- For `source/photos/a.jpg` under a recursive source `source/photos/`, a remote destination `.../backup/` receives `backup/a.jpg`, or `backup/photos/a.jpg` with `--include-parent`. Local-to-local recursive copies currently place files relative to the source root even with `--include-parent`; spell the desired parent directory in the destination explicitly.
- Recursive `--include` and `--exclude` match relative paths (including the retained parent segment when used); exclusion wins. Supports `*` and `?`; patterns without either wildcard use substring matching. Quote patterns to prevent shell expansion. These filters do not select a single-file transfer.

### Overwrite, resumability, and performance

- Set `--overwrite-strategy` deliberately: `force` overwrites where supported, `no-clobber` skips existing targets, and `newer` compares source/destination timestamps rather than proving content equality. The profile may change the default. `--force` and `--no-clobber` cannot be combined; prefer one explicit strategy over mixing aliases.
- `--checkpoint` enables resumable transfer state; `--checkpoint-dir` selects its location and `--checkpoint-threshold` controls when enabled checkpoint transfers use multipart/range transfer. Large-file transfer may also select multipart automatically. Checkpoint support applies to upload, download, and multipart remote copy; local-to-local copying is not a resumable multipart transfer.
- For recursive transfers, tune `--batch-concurrency` for items and `--list-concurrency` for discovery; `--multipart-concurrency` limits parts/ranges of one file. Increase concurrency only after checking bandwidth, service limits, and local resources.

### Planning, failure reports, and verification

- Start with `--dry-run` to inspect the interpreted source, destination, and planned behavior. A preview is not proof of access, remote existence, a complete remote inventory, or successful transfer; inspect any unknown/truncated impact fields.
- Recursive operations support `--manifest-path` for planned items and `--report-path` for outcomes. `--no-manifest` disables manifest output and conflicts with `--manifest-path`; streaming discovery can begin transferring before the full source has been enumerated. `--report-failures-only` reduces report rows, not summary totals. Report/manifest flags and batch/list concurrency are rejected for non-recursive copies.
- A batch may partially succeed and return a nonzero exit code. Inspect failed/skipped counts and the report before retrying; retain checkpoint state and the same intended mapping for recovery. Do not label a skipped item as newly copied.
- Verify the resolved destination with `stat`/`ls` and compare expected names and sizes. For integrity-sensitive work, compare an appropriate checksum or download and hash; timestamps and ETags alone are not a universal content checksum.
