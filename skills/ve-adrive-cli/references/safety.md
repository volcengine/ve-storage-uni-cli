# Safety

- Start with read-only commands such as `capabilities`, `doctor`, `ls`, `stat`,
  or `du` when discovering state.
- Use `--dry-run` before `cp`, `mv`, `sync`, recursive transfers, and bulk
  deletes when supported.
- Do not run destructive commands unless the user clearly requested the exact
  instance, space, and path. Preserve required confirmation flags.
- Never print access keys, secret keys, session tokens, or authorization
  headers. Keep complete signed URLs out of logs and diagnostic artifacts.
  When the user requests a sharing/download link, deliver the returned URL to
  that authorized user and state its expiry when provided; do not publish it
  elsewhere. For OAuth login, deliver the verification URL as instructed by
  the authentication workflow, never the device code or tokens.
- Use only the documented public OAuth and Resource endpoints and signing
  region. Do not invent alternate endpoint or region values.
- Quote paths and A-Drive URIs that contain spaces, wildcards, or user input.


- Sharing grants receiver access to one file, one folder or a whole Space. A Space
  root target shares the entire Space (`ShareAllFiles=true`, `FilePathList=[]`).
  Confirm scope and intended
  view/download permission. Cancellation revokes that share; use global `--yes`
  only when the user has authorized cancellation.
- Treat URL `pwd`, password stdin, receiver access tokens and signed download URLs
  as secrets. Deliver the safe `/share/<ShareID>` page URL without the password
  query. MCP accepts a virtual write-only password and never places it in argv.
  Authenticated create/get business results may include `SharePwd` for authorized
  delivery; keep it out of logs and plans. List/update and receiver output omit it.
- An unknown create/update/cancel result is not permission to retry. Reconcile
  with `share get` or `share list`; no automatic mutation retry occurs.
  This also applies to Ctrl-C after submission. Ordinary share interruptions
  restore terminal echo, including during hidden password input.
- Preview requests consume `PreviewLimit`; never automatically retry an unknown
  preview outcome or raw body failure. `PreviewURL` is a short-lived capability
  for intentional delivery only; keep it out of diagnostics, logs and plans.
  `share view --raw` requires piped/redirected stdout, rejects terminal output,
  and never mixes a success envelope into the byte stream. It never launches a
  browser or falls back to download permissions/API calls.
- Public receiver operations default to anonymous. Only explicit `--auth-mode oauth`
  uses the selected recipient profile for public info/token exchange; its configured
  API endpoint is trusted, not the copied page origin. File APIs carry only the
  share token and signed content carries no account credentials. Login-required
  links need recipient login; expired/revoked links need owner action. Never fall
  back to a different identity or try another password after explicit rejection.
- Keep file paths unchanged while selecting receiver content. Directory download
  destinations must remain contained, and checkpoint files must contain no
  passwords, tokens or signed URLs. Resume revalidates completed files and restores only verified partial ranges.

Share download local operations use anchored Unix directory descriptors or pinned
Windows capability directory handles. User-selected local root aliases are first
resolved by the OS; subsequent downloaded components refuse symlinks/reparse
points. Checkpoint parents and destinations use resolved identities for collision
checks and resume. Temporary files are exclusively created in the destination parent and
only verified files are published. Never replace this with check-then-open path
operations. Checkpoint JSON is versioned and strict, protected by a
no-follow `.lock` sidecar, and excludes passwords, tokens, signed URLs and arbitrary
remote metadata. A completed local file is skipped only after both remote
fingerprint and local SHA-256 validation. Partial/unknown outcomes use one failed
aggregate with retained completed files; do not interpret remaining quota locally.
Definite service rejection, including download-disabled 403, is not an unknown
issuance outcome: preserve its error code/status and set `IssuanceOutcomeUnknown=false`.
Transport interruptions, 5xx and invalid successful issuance responses remain
ambiguous and are never automatically replayed.

Share recovery state consists of the checkpoint JSON, `<checkpoint>.completed/`
completion-ledger directory, and `<checkpoint>.parts/` range state. Retain or move them
together; the main JSON completed array is not updated after each file and cannot
replace the completion ledger. Report rows still expose operation-specific request IDs.

Whole-Space sharing requires service support. Current BOE V1 returns
`InvalidParameter.ShareAllFilesNotAllowed`; do not claim end-to-end availability
from the SDK field or local fixtures, and never bypass that policy with invented
paths or silently expanded ordinary shares.
