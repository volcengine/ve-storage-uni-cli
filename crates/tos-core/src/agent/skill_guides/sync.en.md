## Synchronization decisions

- Treat the source as authoritative for this one-way operation; this is not bidirectional reconciliation. Check source/destination direction, recursive relative paths, `--include-parent`, and include/exclude filters before execution.
- Without `--delete`, destination-only entries are retained. `--delete` enables destination cleanup and raises the confirmation requirements; inspect the exact destination scope with `--dry-run` before enabling it. A preview may have incomplete discovery and cannot prove every deletion candidate.
- Set `--overwrite-strategy` explicitly when existing files matter. `newer` uses timestamps, not a byte-for-byte comparison; the effective profile and backend determine transfer behavior.
- Transfer failures prevent the subsequent destination cleanup phase. A partially successful run can leave copied entries and retained extras; inspect the summary and report before retrying.
- Use `--report-path` to retain outcomes and `--manifest-path` for planned entries; `--no-manifest` trades the full manifest for streaming discovery. Verify desired destination entries, failure counts, and (only with authorized `--delete`) removal of destination-only entries.
