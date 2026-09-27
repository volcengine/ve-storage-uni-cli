## Move workflow

- Moving changes both endpoints and can remove the source. Resolve the exact destination first; a directory-style destination can append the source name. Reject identical or overlapping source/destination scopes before execution.
- Use `--recursive` for a directory/prefix and check `--include-parent`, `--include`, and `--exclude` against the intended relative paths. A move may use rename when supported or copy followed by source deletion; do not assume it is atomic across files or services.
- Inspect `--dry-run` and the command's confirmation requirements. `--force` is not proof of user authorization. Preserve the original source until the intended transfer is verified where the workflow permits.
- Recursive `--report-path` and `--manifest-path` help distinguish copies from source deletions. On failure inspect each operation; a successful copy with a failed deletion can leave both endpoints populated. Do not blindly restart the entire move or delete remaining sources.
- Verify destination names and sizes, then verify the intended source entries were removed. Do not infer completion from destination existence alone.
