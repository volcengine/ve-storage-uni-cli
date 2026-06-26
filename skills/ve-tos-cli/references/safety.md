# Safety

- Start with read-only commands such as `capabilities`, `doctor`, `ls`, `stat`,
  or `du` when discovering state.
- Use `--dry-run` before `cp`, `mv`, `sync`, recursive transfers, and bulk
  deletes when supported.
- Do not run destructive commands unless the user clearly requested the exact
  bucket and path. Preserve required confirmation flags.
- Never print access keys, secret keys, session tokens, authorization headers,
  or complete presigned URLs in logs or final answers.
- Do not infer region or endpoint for write operations. Ask or inspect config.
- Quote paths and storage URIs that contain spaces, wildcards, or user input.
