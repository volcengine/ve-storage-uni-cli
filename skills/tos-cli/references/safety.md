# Safety

- Start with read-only commands such as `capabilities`, `doctor`, `ls`, `stat`,
  or `du` when discovering state.
- Use `--dry-run` before `cp`, `mv`, `sync`, recursive transfers, and bulk
  deletes when supported.
- Do not run destructive commands unless the user clearly requested the exact
  bucket and path. Preserve required confirmation flags.
- Never print access keys, secret keys, session tokens, or authorization
  headers. Keep complete signed URLs out of logs and diagnostic artifacts.
  When the user requests a sharing/download link, deliver the returned URL to
  that authorized user and state its expiry when provided; do not publish it
  elsewhere.
- Do not infer region or endpoint for write operations. Ask or inspect config.
- Quote paths and storage URIs that contain spaces, wildcards, or user input.
