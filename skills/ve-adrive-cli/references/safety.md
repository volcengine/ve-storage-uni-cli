# Safety

- Start with read-only commands such as `capabilities`, `doctor`, `ls`, `stat`,
  or `du` when discovering state.
- Use `--dry-run` before `cp`, `mv`, `sync`, recursive transfers, and bulk
  deletes when supported.
- Do not run destructive commands unless the user clearly requested the exact
  instance, space, and path. Preserve required confirmation flags.
- Never print access keys, secret keys, session tokens, authorization headers,
  or complete signed URLs in logs or final answers.
- Use only the documented public OAuth and Resource endpoints and signing
  region. Do not invent alternate endpoint or region values.
- Quote paths and A-Drive URIs that contain spaces, wildcards, or user input.
