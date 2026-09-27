## Deletion workflow

- Confirm the concrete object/file or recursive prefix/folder before deleting. Inspect `--dry-run`, target normalization, filters, and confirmation requirements. A preview with unknown or truncated impact is not a complete deletion inventory.
- Use `--recursive` only for the intended subtree. Filter support depends on the backend and deletion mode; do not assume a direct recursive delete honors include/exclude filters. Read the parameter contract before selecting a deletion strategy.
- Do not add `--include-uploads` unless cleanup of unfinished multipart uploads is part of the requested task; it expands the resources affected.
- For batch deletion, preserve `--report-path`, inspect failures and skips, and re-list the target. A nonzero exit code may follow partial deletion; rerun only the remaining authorized scope. File/object deletion does not imply deletion of its containing bucket or space.
