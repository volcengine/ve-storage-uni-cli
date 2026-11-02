<!--
Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
-->

# Volcengine Storage Unified CLI

Volcengine Storage Unified CLI provides three public command-line tools for
Volcengine storage workflows:

### `ve-tos-cli`

Use `ve-tos-cli` for Volcengine TOS object storage workflows, including bucket
and object operations, multipart transfers, presigned URLs, diagnostics,
capability discovery, and MCP serving.

### `tos-cli`

Use `tos-cli` for the ByteCloud TOS command surface. It shares the same storage
runtime foundations as `ve-tos-cli`, while exposing the dedicated `tos` command
behavior expected by ByteCloud users and scripts.

### `ve-adrive-cli`

Use `ve-adrive-cli` for Volcengine ADrive file workflows, including listing,
upload, download, sync, folder creation, diagnostics, capability discovery, and
MCP serving.

The internal `ve-storage-uni-cli` dispatcher is kept for local development and
cross-surface testing. Public package managers and curl installation expose the
three dedicated commands above.

## Security and privacy

This project takes security seriously.
For vulnerability reporting and supported versions, see [SECURITY.md](SECURITY.md)

HTTPS requests from `tos-cli`, `ve-tos-cli` and `ve-adrive-cli` use reqwest 0.13
with the platform certificate verifier. Install and trust enterprise/private CAs
in the operating system certificate store; Linux environments need an available
system CA bundle. Certificate-chain and hostname validation remain enabled.
The credential SDK still uses reqwest 0.12; its compatibility dependency loads
system roots alongside the SDK's bundled roots, rather than using the platform
verifier's full trust/distrust policy.

三个 CLI 的业务 HTTPS 请求使用系统证书信任机制。企业或私有 CA 应在操作系统中安装
并设为受信任；Linux 运行环境需提供系统 CA 集合。证书链和主机名校验保持开启。
凭据 SDK 内部暂用 reqwest 0.12，兼容配置为其合并系统 CA 与内置 CA，不能等同于
业务请求所用平台验证器的完整信任与不信任策略。

## Installation

Choose one package manager. The commands below assume the package manager itself
is already installed and authenticated when required.

```bash
cargo install ve-tos-cli
cargo install tos-cli
cargo install ve-adrive-cli
```

```bash
npm install -g ve-tos-cli
npm install -g tos-cli
npm install -g ve-adrive-cli
```

```bash
pip install ve-tos-cli
pip install tos-cli
pip install ve-adrive-cli
```

```bash
brew tap volcengine/ve-storage-uni-cli https://github.com/volcengine/ve-storage-uni-cli
brew install ve-tos-cli
brew install tos-cli
brew install ve-adrive-cli
```

```powershell
winget install ve-tos-cli
winget install tos-cli
winget install ve-adrive-cli
```

Install all three CLIs with curl:

```bash
curl -fsSL https://github.com/volcengine/ve-storage-uni-cli/releases/latest/download/install.sh | sh
```

Install one CLI with curl:

```bash
curl -fsSL https://github.com/volcengine/ve-storage-uni-cli/releases/latest/download/install.sh | sh -s -- ve-tos-cli
curl -fsSL https://github.com/volcengine/ve-storage-uni-cli/releases/latest/download/install.sh | sh -s -- tos-cli
curl -fsSL https://github.com/volcengine/ve-storage-uni-cli/releases/latest/download/install.sh | sh -s -- ve-adrive-cli
```

See [packaging/README.md](packaging/README.md) for local build, packaging, and
release publishing details.

## Quick Start

Check that the commands are available:

```bash
ve-tos-cli --version
tos-cli --version
ve-adrive-cli --version
```

Configure TOS credentials with environment variables:

```bash
export TOS_ACCESS_KEY=<your-access-key-id>
export TOS_SECRET_KEY=<your-secret-access-key>
export TOS_SECURITY_TOKEN=<optional-sts-token>
```

`ve-tos-cli` supports `aksk or unified`. Unified
authentication selects the same-name profile managed by the external login
framework. It ignores local AK/SK in `config.toml` and `credentials.toml`; the
SDK supplies fresh signing credentials for each HTTP attempt. Login state is
owned externally, so use `ve login` instead of writing credentials through the
storage CLI.

```bash
ve-tos-cli --profile default --auth-mode unified ls
ve-tos-cli config set auth_mode unified
ve login
```

VeTos mode precedence is `--auth-mode` > `[profile.ve-tos].auth_mode` >
`TOS_AUTH_MODE` > the backward-compatible `aksk` default.

`tos-cli` supports `aksk` (default) and public, built-in `zti`. Select it with
`tos-cli --auth-mode zti ls tos://bucket/` or `tos-cli config set auth_mode zti`.
Its mode precedence is `--auth-mode` > `[profile.tos].auth_mode` >
`BYTETOS_AUTH_MODE` > `aksk`. ZTI uses `SEC_TOKEN_STRING`, a local Agent at
`ZTI_AGENT_SOCKET_PATH` (default `/run/zti-agent.sock`), or `SEC_TOKEN_PATH`,
in that order. It ignores AK/SK and does not fall back to them. Agent access
requires Unix; file Tokens are re-read per request, while Agent Tokens refresh
within 600 seconds or before expiration. `tos-cli doctor --check auth` reports
source availability offline; a storage request verifies remote access. ZTI does
not support `presign`, which requires AK/SK. Never place Token values in command
arguments or logs.

For ByteCloud TOS PSM discovery, `tos-cli --psm <service>` normally checks
bucket routing through BNS first. Set `TOS_FORCE_PSM=true` to skip BNS and
resolve the selected CLI/configured PSM directly through Consul. This setting
only affects `tos-cli` PSM mode; an explicit `--endpoint` takes precedence.
`doctor --check network` reports configuration and does not validate a PSM;
use a bucket request to test service discovery.

Configure ADrive credentials with environment variables:

```bash
export ADRIVE_ACCESS_KEY=<your-adrive-access-key-id>
export ADRIVE_SECRET_KEY=<your-adrive-secret-access-key>
export ADRIVE_SECURITY_TOKEN=<optional-sts-token>
```

ADrive supports `aksk, oauth, or unified`. Existing installations keep using
`aksk` by default.

```bash
ve-adrive-cli config set auth_mode aksk
ve-adrive-cli config set auth_mode oauth
ve-adrive-cli --profile default --auth-mode unified ls
ve-adrive-cli config set auth_mode unified
ve login
ve-adrive-cli config set auth_endpoint https://idsauth.volces.com
ve-adrive-cli auth login --instance inst-1
ve-adrive-cli auth status
ve-adrive-cli auth logout
ve-adrive-cli --dry-run auth logout
```

Mode precedence is `--auth-mode` > `[profile.adrive].auth_mode` >
`ADRIVE_AUTH_MODE` > the backward-compatible `aksk` default. Once a
mode is selected, credentials from the other mode are not used as fallback.
For one-off automation, pass `--auth-mode` without changing the Profile.
Unified selects the same-name externally managed profile and ignores local
AK/SK and OAuth tokens. `ve-adrive-cli auth login/logout` does not modify
Unified state; use `ve login` / `ve logout` through the external framework.
`auth logout --dry-run` reports whether the selected Profile would be cleared
without rewriting `credentials.toml` or creating local encryption key material.

`auth login` runs the Device Authorization flow in the foreground: it prints
the verification URL and then polls until the user finishes or the grant
expires. It requests the fixed `all` scope and saves
the returned Access/Refresh Token pair under the current Profile in encrypted
`credentials.toml`, along with any returned `user_id` identity metadata. OAuth
user-owned Space creation uses that metadata when `--owner-id` is omitted;
OAuth group-owned Spaces always require an explicit `--owner-id`. The Auth
endpoint priority is `--auth-endpoint` > `[profile.adrive].auth_endpoint` >
`ADRIVE_AUTH_ENDPOINT` > the effective business `endpoint` (CLI > profile >
`ADRIVE_ENDPOINT`). An omitted Auth endpoint therefore reuses the business
endpoint. Auth and Resource may share the same origin. Explicit HTTP and HTTPS
are preserved; bare hosts default to HTTPS. Refresh always uses the Auth endpoint
saved during login, even if connection configuration later changes.

OAuth resource requests use Bearer authentication. Before each command the CLI
reuses a valid Access Token or refreshes file-backed credentials when at most 60
seconds remain. A first resource `401` forces one coordinated refresh and one
replay; `403` never refreshes. The CLI is not a resident process and does not
refresh in the background. If Refresh is no longer possible, scripts receive a
`login_required` error; the CLI never starts an interactive login implicitly.

`ADRIVE_ACCESS_TOKEN` is supported for read-only process-scoped automation when
no OAuth Token exists in `credentials.toml`. Environment credential groups are
never mixed with file credentials and cannot be auto-refreshed because rotated
Refresh Tokens cannot be written back to the parent process environment.

Sensitive credentials are stored separately from normal configuration:

```text
$HOME/.tos/config.toml       # region, endpoint, auth_mode, and other settings
$HOME/.tos/credentials.toml  # encrypted AK/SK, security tokens, and OAuth tokens
```

New AK/SK writes from `tos-cli`, `ve-tos-cli`, and `ve-adrive-cli` go only to
`credentials.toml`. Existing AK/SK values in `config.toml` remain readable for
compatibility and are not migrated or duplicated automatically. Credential
precedence is `credentials.toml` > legacy `config.toml` > environment variables.
Bare credential keys are isolated by command surface:

```bash
tos-cli config set access_key_id <byte-tos-access-key>
tos-cli config set secret_access_key <byte-tos-secret-key>
ve-tos-cli config set access_key_id <volcengine-tos-access-key>
ve-tos-cli config set secret_access_key <volcengine-tos-secret-key>
ve-adrive-cli config set access_key_id <adrive-access-key>
ve-adrive-cli config set secret_access_key <adrive-secret-key>
```

These commands write `[profile.tos]`, `[profile.ve-tos]`, and
`[profile.adrive]`, respectively. Use an explicit key such as
`default.access_key_id` only when shared credentials for `tos-cli` and
`ve-tos-cli` are intended.

Initialize or inspect local configuration:

```bash
ve-tos-cli config init
# config init writes region=cn-beijing and endpoint=tos-cn-beijing.volces.com
ve-tos-cli config show --output json

tos-cli config set region cn-beijing
tos-cli config set endpoint https://your-bytetos-endpoint.example.com
tos-cli config show --output json

ve-adrive-cli config set region cn-beijing
ve-adrive-cli config set endpoint https://ids-cn-beijing.volces.com
ve-adrive-cli config show --output json
```

Run common TOS workflows:

```bash
ve-tos-cli capabilities --view groups
ve-tos-cli ls tos://my-bucket/prefix/ --output table
ve-tos-cli cp ./local.txt tos://my-bucket/local.txt --dry-run
ve-tos-cli cp ./local.txt tos://my-bucket/local.txt
ve-tos-cli stat tos://my-bucket/local.txt --output json
```

Run the ByteCloud TOS command surface:

```bash
tos-cli capabilities --view groups
tos-cli ls tos://my-bucket/prefix/ --output table
tos-cli cp ./local.txt tos://my-bucket/local.txt --dry-run
tos-cli sync ./dir tos://my-bucket/backup/ --recursive --dry-run
```

ADrive V1 share links expose nine commands: `share create`, `get`, `list`,
`update`, `cancel`, `inspect`, `files`, `view`, and `download`. The current Go SDK contract
requires OAuth Bearer / token-source authentication for share management. AK/SK
or Unified access to other resource APIs does not establish share-management
support. Receivers default to anonymous sessions and load no account credentials.
Explicit `--auth-mode oauth` uses the recipient's selected `--profile` for
`inspect`, `files`, `view`, and `download`.
Use `share <action> --help` or `--describe` for the complete typed contract.

| Commands | Target | Compatible explicit form |
| --- | --- | --- |
| `create` | `adrive://instance/space[/path]` (Space root, file or folder) | `--instance --space [--folder/--file]` |
| `list` | `adrive://instance` (optional root slash) | `--instance instance` |
| `get`, `update`, `cancel` | `adrive://instance/share-id` | `share-id --instance instance` |
| `inspect`, `files`, `view`, `download` | Public HTTP(S) `/share/<ShareID>` URL | Same URL; `files`, `view`, and `download` accept `--path` |

```bash
ve-adrive-cli share create adrive://instance/space/docs/ --permission download --expires-in 7d
ve-adrive-cli share create adrive://instance/space --permission download
ve-adrive-cli share create --instance instance --space space --permission download
ve-adrive-cli share get share-id --instance instance
ve-adrive-cli share get adrive://instance/share-id
ve-adrive-cli share list adrive://instance
ve-adrive-cli share list --instance instance --creator user-id --limit 100
ve-adrive-cli share update share-id --instance instance --require-login false --expires-in 0
ve-adrive-cli share cancel share-id --instance instance --yes
ve-adrive-cli share inspect https://instance.example.test/share/share-id
ve-adrive-cli share files https://instance.example.test/share/share-id --path docs/
ve-adrive-cli share files https://instance.example.test/share/share-id --path /
ve-adrive-cli share download https://instance.example.test/share/share-id ./downloads --recursive --checkpoint ./share-checkpoint.json
ve-adrive-cli share download https://instance.example.test/share/share-id ./downloads --path / --recursive
ve-adrive-cli config set share_endpoint https://pages.example.test
```

Whole-Space sharing requires backend support. BOE V1 integration on 2026-10-27
returned `InvalidParameter.ShareAllFilesNotAllowed` even with valid relative
paths. The CLI wiring and local fixtures below do not establish BOE availability;
creation and real whole-Space receipt remain pending backend support and validation.

Create accepts one file/folder URI, a whole Space root, or explicit selectors.
`adrive://instance/space` and `adrive://instance/space/`, or
`--instance instance --space space`, share the entire Space with
`ShareAllFiles=true` and `FilePathList=[]`. Non-root file/folder requests omit
`ShareAllFiles` (default false) and retain one `FilePathList` entry; explicit empty `--file` values remain invalid. Management
JSON exposes optional `ShareAllFiles` when returned by the service; omission means
unknown. Creation defaults to permission `view` (preview only) and expiration
`0` (never). Allow downloads explicitly
with `--permission download`, including directory receipt. Update retains explicit zero,
false and empty strings and fetches `UpdatedAt` when no guard is supplied. List
requires repeated `--creator` except when OAuth supplies its bound `user_id`;
search returns one page with original pagination fields. Cancel requires global
`--yes` outside ordinary TTY execution. Unknown mutation outcomes return
`share_result_unknown`: reconcile with get/list before retrying.

Management output keeps the public field name `ShareUrl`. Without a page override,
create/get/update/list use the service's `ShareURL`, after validating the URL and
its ShareID. The API endpoint does not determine the delivery page domain. If the
service omits the URL and no override is configured, the operation stays successful:
`ShareID` is retained, `ShareUrl` is omitted and `ShareUrlWarning` explains how to
configure an override or retrieve the share again. Do not repeat a successful
create merely to obtain its URL.

A configured override follows `--share-endpoint` > selected
`[profile.adrive].share_endpoint` > `ADRIVE_SHARE_ENDPOINT` > legacy
`ADRIVE_SHARE_BASE_URL`. It appends `/share/<encoded ShareID>` to the literal origin,
without adding an Instance label or password. For example,
`--share-endpoint http://pages.example.test` and ShareID `2T4U8z0lY1n` produce
`http://pages.example.test/share/2T4U8z0lY1n`. The old CLI option
`--share-base-url`, TOML key `share_base_url`, and environment variable remain
compatibility inputs; help, Describe/MCP, and config serialization use the canonical
names. Create/get may retain a `pwd` query in the service URL for authorized delivery.
Treat that URL as a secret: do not put it in logs or saved plans. List/update omit
password fields and strip all URL query parameters. Account tokens and raw response
URLs are excluded from diagnostics.

On the receiver side, standard `idsapp.volces.com` and
`idsapp-boe.volces.com` page hosts map to `idsapi.volces.com` and
`idsapi-boe.volces.com` APIs, retaining the Instance label, scheme and port.
Share pages, receiver
APIs and download addresses support HTTP and HTTPS; the CLI does not upgrade
protocols. Anonymous receivers derive the API origin
from the page URL without loading account configuration; use explicit `--endpoint`
for a custom receiver API. Explicit OAuth uses the selected profile's API endpoint
or `--endpoint`, so account credentials are not sent to a copied page origin.
Standard page Instance labels must match the OAuth binding. Other hosts are not rewritten. File and folder links both
use `/share/<ShareID>`, with no filename appended. Receivers also accept
`/share/<ShareID>/` and normalize its single trailing slash; it identifies the same
share and download checkpoint. Receiver `--path` is exact for
files and ends in `/` for directory listing; no path lists roots, including
single-file shares. For a whole-Space share, omitted `--path` and explicit
`--path /` list the Space root. Anonymous overview metadata may omit `ShareAllFiles`;
the receiver adapts root listing through read-only metadata requests. Only an
explicit share-scope denial permits the root-list fallback; arbitrary 403 responses
retain their service error. Like `ls`, `files --max-keys` defaults to 1000 and caps files
and folders together at the current directory level. It automatically fetches
service pages until the cap or the listing ends, including when a service page
is shorter than the requested size. Pass the returned `NextMarker` into the next
call's `--marker` to continue; `IsTruncated=false` means the listing ended.
`--max-keys` accepts integers from 1 through 2147483647; `files` has no `--limit`
alias. Exact file selections return one file without directory
pagination. Default table/CSV uses the same `file_path`, `size`, `file_type`,
`updated_at`, `is_folder` columns and folder-first rows as `ls`; empty pages retain
these headers. The table Total counts displayed rows after folder marker normalization.
Use `-H` and `--columns` for table/CSV presentation. Explicit
`--sort key|name|size|last-modified|mtime` sorts the capped result set in every
format/query, retaining raw metadata values and marker boundaries like `ls`.
An explicit `--manifest-path` writes a listing CSV; none is written by default.
JSON and explicit `--query` retain the `Files`, `Folders`, `NextMarker` and
`IsTruncated` field names, with aggregated current-level metadata and normalized
folder markers, self markers and duplicates. `Total` and `pagination.total_returned`
count the same logical entries in all output modes. `share inspect` reads share
overview information; use `share files` for file metadata. Download automatically
enumerates pages and has no `--max-keys`, `--marker` or `--limit` flags. Management
`share list` keeps its existing `--limit`/`--offset` pagination. Download destinations are
directories. `--resume` requires checkpoint state and reuses verified completed files
and partial ranges. Omitted `--path` selects all share roots; folders require
`--recursive`. For a whole-Space share, omitted `--path` or `--path /` with
`--recursive` downloads the entire Space and preserves its full Space-relative
hierarchy. The Space root has no basename, so `--include-parent` adds no wrapper;
subdirectory selections retain the cp contents/parent rules. Download defaults
match `cp`: directory contents under the destination,
force overwrite, 16 concurrent files, 4 directory listings and 4 ranges per file.
Use `--include-parent` to retain the directory name, `--include`/`--exclude` for cp
relative source filters, and `--no-clobber` or `--overwrite-strategy newer` to skip
existing or newer destinations. The default summary shows total/succeeded/failed/
skipped/pending; `Completed` includes resumed and policy-skipped files, so succeeded
is `Completed-Skipped`. `--report-path`, `--manifest-path` and `--no-manifest` control
CSV artifacts; see the download contract below for recovery, progress and safety.

Files without a strong ETag or CRC64 use a single streamed GET to avoid mixing versions.
`Forbidden.ShareDownloadDisabled` may mean the share permission forbids downloads
or its download-count quota is exhausted. The code alone cannot distinguish the
cause; ask the owner to check permission and quota. Service code, HTTP status and
request ID remain available when supplied.
`share view <url> [--path exact/file]` previews a file using the share's view
permission. A single shared file is selected automatically; folders or multiple
roots require `--path` to a file. Default auto mode returns a short-lived PDF
`PreviewURL`, `ExpiresAt` and `PreviewType` for PDF/Office documents, or raw
content metadata for other files. It does not open a browser. Keep capability
URLs private. Each successful preview request consumes `PreviewLimit`, and
preview requests never automatically retry. Deployment support is required:
the previously tested BOE environment returned preview HTTP 404.

Use `share view <url> --path notes.txt --raw | less` for original bytes.
`--raw` requires a pipe or redirected stdout and conflicts with explicit
`--output`/`--query`; failures after streaming starts go to stderr. Default
structured mode supports normal JSON/table/query output and MCP tools.
A typical single-file
download makes five requests: share info, share token, root listing, download URL,
and content. An explicit --path replaces root listing with file metadata lookup.
New checkpoints bind the actual receiver API origin; OAuth also binds the selected
profile, OAuth mode and applied recipient subject when available. Environment/access-only
credentials without `user_id` bind API/profile/mode; the CLI cannot establish whether
an unknown-subject token replaced across resume belongs to another account and does
not decode tokens to infer identity. Legacy anonymous checkpoints can migrate only
when receiving through the URL-derived API. Changing a custom API requires a fresh task.

Passwords use bounded `--password-stdin` input read to EOF (use a pipe), preserving spaces and stripping
one terminal LF or CRLF. Explicit empty input (EOF-only, LF-only or CRLF-only)
fails validation with exit 6 before mutation or share-token exchange; spaces remain
significant. Omitting the flag preserves the current password on update. Explicit
create/update `--password-stdin` is also read and validated during `--dry-run`.
If manually entering into `--password-stdin` on macOS/Linux,
press Enter then Ctrl-D within 30 seconds; Enter alone does not send EOF. A pipe
closes stdin automatically. For files/view/download in an ordinary interactive terminal,
omit the flag to use the hidden prompt and finish with Enter.
Receiver precedence is stdin, URL `pwd`, then a hidden TTY
prompt. JSON/noninteractive calls need stdin or URL password; `inspect` never reads
one. MCP's virtual write-only `password` overrides URL `pwd`, is piped to stdin,
and is removed from planned/executed argv. Each independent files/view/download call,
including subsequent pages, needs the code again; it is never persisted.
Login-required shares deny anonymous file access; explicit OAuth supplies the
recipient identity. `--describe` and `--help` load no config, access no network and
read no stdin. Other share dry-runs remain offline; download dry-run uses read-only
metadata and share-token exchange as described below.
Actual authenticated `create` and `get` results may include the service-returned
`SharePwd` for authorized delivery. The page URL has no password query. Keep that
delivery password out of logs and saved plans; list, update, and receiver output
omit it.

Share passwords are the service's extraction codes: create/update send `SharePwd`,
and files/view/download send the supplied code to `GetShareAccessToken` before browsing
or downloading. `inspect` reads public metadata without a password. Password clearing
is not established by the PRD; an empty value must not be described as confirmed
server-side clearing behavior.

`--require-login` maps to the V1 service's `RequireLogin` field and controls the
recipient's login requirement. Sharing creation and management still require the
owner's authenticated account. `false` permits anonymous receipt subject to password
and share permissions; `true` requires the recipient to log in. Default
files/view/download reject such shares, preserving service denial codes; when public
metadata is available, the CLI returns `share_login_required`. Inspect can still
read public metadata when the service permits it. To receive as a logged-in user:

```bash
ve-adrive-cli --profile receiver auth login --instance instance
ve-adrive-cli --profile receiver auth status
ve-adrive-cli --profile receiver share inspect "$SHARE_URL" --auth-mode oauth
ve-adrive-cli --profile receiver share files "$SHARE_URL" --auth-mode oauth --path docs/ --max-keys 1000
ve-adrive-cli --profile receiver share download "$SHARE_URL" ./downloads --auth-mode oauth --recursive
```

Configure the recipient profile's API endpoint before login/receipt. A cached
token is not proof of service authorization: the API validates identity, instance,
scope and share rules. Existing stored-token refresh is reused; no login or account
fallback occurs automatically. A profile/environment OAuth default alone does not
activate authenticated receipt. Public info and token exchange use OAuth; file APIs
use only `x-share-token`, and signed content uses a credential-free client.
For a protected share, read once without echo and pipe to each command:

```bash
read -r -s -p 'Extraction code: ' SHARE_PASSWORD; printf '\n'
printf '%s\n' "$SHARE_PASSWORD" | ve-adrive-cli share files "$SHARE_URL" --password-stdin --max-keys 1000
printf '%s\n' "$SHARE_PASSWORD" | ve-adrive-cli share files "$SHARE_URL" --password-stdin --max-keys 1000 --marker "$NEXT_MARKER"
printf '%s\n' "$SHARE_PASSWORD" | ve-adrive-cli share download "$SHARE_URL" ./downloads --password-stdin --recursive
unset SHARE_PASSWORD
```

Add `--auth-mode oauth --profile receiver` to each receipt command when login is
required. Extraction codes and OAuth identity are independent checks.

`get`, `update`, and `cancel` accept either `adrive://instance/share-id` or
`share-id --instance instance`. A duplicate `--instance` must match the URI.
The URI contains exactly one ShareID segment and decodes components once;
`--by-name` resolves its Instance name. File/space URIs are interpreted separately
by their command context; sharing creation still selects `instance/space/path`.
`list` accepts `adrive://instance` or `--instance instance`, with the same filters,
pagination and output. The URI may end in one root slash and has no share/space/file
path. Components decode once; a duplicate instance flag must agree. `--by-name`
resolves the selected Instance name in either form.

With no output override, terminals display a table and pipes receive JSON.
Share detail tables include `ShareID`, `ShareUrl`, any authorized delivery
`SharePwd`, and file paths. Use `--output json` for structured results; retrieve
an existing share with `share get` or `share list` rather than creating it again
to change its display format.

Share downloads scan the selected metadata before requesting signed file URLs.
Folders require `--recursive`; the default copies directory contents like `cp`.
For `--path docs/`, `docs/a.txt` becomes `<destination-dir>/a.txt`;
`--include-parent` instead produces `<destination-dir>/docs/a.txt`. Empty directories
are retained. `--include` and `--exclude` match cp relative source paths. Listing
cycles, unsafe paths and case/Unicode or file/directory collisions remain errors;
there is no fixed 20,000-entry or 8 MiB aggregate snapshot limit.
The OS resolves user-selected destination/checkpoint directory aliases before the
CLI pins their actual roots; downloaded descendants reject symlinks/reparse points
with Conflict (exit 9). Native permission and disk errors retain their cause.

Overwrite defaults to `force`, matching `cp`; `--force` and legacy `--overwrite`
select that policy explicitly. `--no-clobber` skips existing ordinary files;
`--overwrite-strategy newer` overwrites only older destinations. Replacements are
published after size, available CRC64 and strong ETag verification; SHA-256 is
recorded locally. Content HTTP(S) requests carry no account/share-token credentials,
follow no redirects, and retry using the issued URL. Ordinary file failures allow
other files to continue. An unknown URL-issuance outcome stops new dispatch and is
never automatically replayed; issuance may already have consumed download quota.
Explicit rejections preserve service code/status/request ID and set
`IssuanceOutcomeUnknown=false` (download-disabled 403 exits 5).

`--checkpoint` enables a stable task checkpoint under
`~/.tos/checkpoints/ve-adrive`; `--checkpoint-dir` overrides the directory.
`--checkpoint <FILE>` remains available for explicit/legacy paths. `--resume`
reauthorizes the saved selection, validates current remote fingerprints and
completed local size/SHA-256, and restores verified partial ranges. Remote/local
changes fail safely; no new remote files are added. Keep selection, filters,
layout, overwrite policy and explicit checkpoint path consistent. Older checkpoints
that retained the selected directory name require `--include-parent`. A pre-existing
task checkpoint requires `--resume`; checkpoint/lock paths cannot overlap downloads
or CSV artifacts, including directory aliases. State contains no passwords, tokens
or signed URLs.

Defaults are 16 files, 4 directory listings and 4 ranges per file. Use
`--batch-concurrency`, `--list-concurrency` and `--multipart-concurrency` to change
them. Files at least 20MB with a strong ETag/CRC64 automatically use 16MiB ranges;
`--checkpoint-threshold` changes the threshold. Files without either identity use
one streamed GET. `--bandwidth-limit 100MB` shares a bytes-per-second budget across
files/ranges; `0` is unlimited. `--list-echo`/`--no-list-echo` control listing stderr,
`--progress`/`--no-progress` control transfer stderr, and `--quiet` suppresses both.
`--progress-granularity part|byte` selects progress units. Local transfer settings use CLI > selected profile adrive/shared transfer fields >
ADRIVE environment > defaults, without loading account credentials or changing
receiver authentication.

Reports and planned-manifest CSVs default to `~/.tos/reports/ve-adrive`.
`--report-path` and `--manifest-path` override those paths;
`--report-failures-only` filters report rows, while `--no-manifest` disables only the
manifest CSV and retains the safety scan/recovery snapshot. Report rows carry their own
`request_id` and no passwords, tokens or signed URLs. Aggregate data exposes
`report_path`, `manifest_path`, `checkpoint_path`, `Complete`, `PlanComplete`,
`CheckpointAvailable`, `Summary` and per-file `completed`/`failed`/`pending`/`skipped`
states. `Summary.Skipped` includes verified resumed files and overwrite-policy skips;
it is included in `Completed`, and table succeeded is `Completed-Skipped`.

`share download <url> <destination-dir> --dry-run` performs read-only share
info/token/list/get metadata requests and enumerates every selected page and recursive
folder before returning `selection.Manifest` with root paths and remote-to-local
mappings. `selection` also contains `PlanComplete=true`, `FileCount`,
`DirectoryCount`, `TotalBytes`, `SkippedCount` and `Conflicts`. Missing or negative file
sizes fail validation (exit 6); explicit zero is a valid empty file. With `--resume`, it
reads the checkpoint and validates its identity, current remote metadata and completed
local file digests without locking or saving the checkpoint. A completed plan with
local conflicts exits 9 and retains the plan in the response. It issues no download
URLs, fetches no content and creates no directories, temporary files, checkpoints
or locks/reports/manifests: `DownloadsIssued=0`, `LocalWrites=false`. `MetadataResponses` counts received
metadata responses; `NetworkRequests=null` because wire attempts are not counted.
`SnapshotGuaranteed=false`: remote content and local paths can change after inspection.
OAuth dry-run disables automatic refresh to avoid credential/auth-lock writes.
For an expired token, a token within 60 seconds of expiry, or a 401 response, first run
`ve-adrive-cli --profile <profile> share inspect <share-url> --auth-mode oauth`
as a normal OAuth read to refresh, then retry. Planning never acquires the checkpoint lock; an existing
regular sidecar may be active. Actual execution rechecks state and acquires its lock.
Expected `report_path`, `manifest_path` and `checkpoint_path` are returned without
creating those artifacts. Describe exposes `dry_run_output_schema` separately from
the real download `output_schema`. MCP checkpoint values are boolean or nonempty
filename strings: `true` emits a bare flag, `false` disables it, and a string selects FILE.

整 Space 分享依赖服务端支持。2026-10-27 的 BOE V1 联调在传入合法相对路径后仍返回
`InvalidParameter.ShareAllFilesNotAllowed`，明确禁止整空间分享。以下内容为 CLI 对接及本地
夹具验证范围，不代表 BOE 已可用；真实创建及整空间接收链路仍需服务端开放后验证。

整 Space 分享可用 `share create adrive://instance/space`（可带尾斜杠）或
`--instance instance --space space` 创建，对应 `ShareAllFiles=true`、`FilePathList=[]`。
非根文件/目录仍为单路径分享，显式空 `--file` 无效。整 Space 接收省略 `--path`
或使用 `--path /` 选择根；下载须带 `--recursive` 并保留完整空间相对层级，Space
根没有 basename，`--include-parent` 不增加包装目录；选中子目录仍遵循 cp 布局。
匿名概况可能不含 `ShareAllFiles`，仅明确分享 scope 拒绝可进行只读根列举适配，
不把所有 403 视为可重试。

分享下载先安全预扫描，再申请文件下载地址；目录必须 `--recursive`，默认与 cp
一样复制目录内容，`--include-parent` 才保留选中目录名；空目录仍保留。
`--include`/`--exclude` 按源相对路径过滤。默认 force 覆盖；可用 `--no-clobber`
跳过现有普通文件，或 `--overwrite-strategy newer` 仅覆盖较旧目标，兼容 `--overwrite`。
默认并发 16 个文件、4 个目录、每文件 4 个分片，三类 concurrency 参数均可覆盖。
`--bandwidth-limit 0` 不限速；非零值为全部文件/Range 共享的每秒字节限速。
用户根目录和 checkpoint 目录别名由操作系统解析，下载子路径拒绝穿越、链接及
大小写/Unicode/文件目录冲突；校验大小、CRC64、ETag 后才原子发布。
`--checkpoint` 使用默认目录中的稳定任务路径，`--checkpoint-dir` 覆盖目录，
`--checkpoint FILE` 保留显式文件写法。`--resume` 复用已验证文件和部分 Range，
不加入新增远端文件；选择、过滤、目录布局及覆盖策略必须与保存状态一致，旧目录名布局需
`--include-parent`。本地大小/摘要变化时返回 Conflict（退出码 9），保留文件和状态。
报告和清单 CSV 默认写入 `~/.tos/reports/ve-adrive`；`--report-path`、`--manifest-path`
覆盖路径，`--report-failures-only` 仅过滤报告，`--no-manifest` 只关闭清单 CSV，
不关闭安全预扫描或恢复快照。data 回显三类产物路径，CSV 每行保留自身 request_id，
不保存密码、token 或签名 URL。Skipped 包含恢复复用和策略跳过，属于 Completed 子集；
表格 succeeded=Completed-Skipped。普通文件失败继续其他文件，未知签发结果停止新派发且不自动重签。
管理结果默认使用服务端 `ShareURL`，仍输出 `ShareUrl`；配置 `share_endpoint` 时按既有规则拼接。
缺少服务端 URL 且未配置覆盖值时保留成功的 `ShareID`，省略 `ShareUrl` 并返回 `ShareUrlWarning`，不按 API endpoint 猜页面域名。
create/get 可保留服务端 URL 的 `pwd` 用于授权交付；update/list 去掉全部查询参数及密码字段。
接收端标准 IDS 公网及 BOE 页面可推导 API；自定义接收 API 用 `--endpoint` 覆盖。明确 403 拒绝保留服务端错误码，退出码为 5，
`IssuanceOutcomeUnknown=false`；只有无法确定签发结果时才标记未知。
`Forbidden.ShareDownloadDisabled` 可能由分享权限或下载次数配额引起，不能仅凭
错误码断言唯一原因。下载 `--dry-run` 只读取 info/token/list/get 元数据并完整枚举
选中内容，恢复预演会只读校验 checkpoint 和已完成本地摘要。返回 selection.Manifest、
文件/目录数、总字节数、跳过数及冲突；完成计划有本地冲突时退出码为 9 并保留计划。
不签发下载 URL、不取文件内容、不创建目录/临时文件、不写 checkpoint/锁/report/manifest；
DownloadsIssued=0、LocalWrites=false、SnapshotGuaranteed=false。MetadataResponses
仅统计收到的元数据响应，NetworkRequests=null 不伪装为网络尝试次数。
显式空密码（EOF/LF/CRLF）校验失败并退出 6，空格保留；create/update 的预演也会
读取校验显式密码输入。--describe/--help 始终不读取配置、网络或 stdin。

Directory errors appear separately in `DirectoryFailures` (LocalPath/Path/Message)
and failed `mkdir` report rows; `Summary` continues to count files.

Multiple shared roots can collide when copying their contents. Use
`--include-parent` to preserve distinct root names; the CLI rejects ambiguous
mappings and never guesses replacement names.

Share recovery state consists of the checkpoint JSON, `<checkpoint>.completed/`
completion-ledger directory, and `<checkpoint>.parts/` range state. Retain or move them
together; the main JSON completed array is not updated after each file and cannot
replace the completion ledger. Report rows still expose operation-specific request IDs.

For reproducible OAuth validation, documentation/discovery checks and local receiver
regressions, follow [ADrive sharing test steps](docs/adrive-share-testing.md).

Dedicated live share tests: set `ADRIVE_E2E_INSTANCE` and `ADRIVE_E2E_SPACE`
for existing disposable test resources, optionally `ADRIVE_E2E_CREATOR` for
explicit creator filtering, `ADRIVE_E2E_SHARE_ENDPOINT` for page origin, and
`ADRIVE_E2E_RECEIVER_ENDPOINT` when the receiver API differs from the page origin.
`ADRIVE_E2E_BINARY` selects a built unified or standalone CLI. Run
`python3 -m pytest scripts/e2e/adrive/test_share.py`; missing dedicated resources
skip the live test. It creates and cleans only its unique test folder and shares.
The current live harness injects AK/SK credentials. On the BOE service verified
on 2026-10-13, ordinary file operations succeeded but share creation returned
`AccessDenied`; a share search returned `unable to determine user identity`.
An authorized OAuth-backed runner is needed for deployments following the
current SDK share-management contract. This failure is not a passing live-share
validation and must not be converted into a skip.


Run common ADrive workflows:

```bash
ve-adrive-cli capabilities --view groups
ve-adrive-cli ls adrive://instance/space/path/ --output table
ve-adrive-cli cp ./local.txt adrive://instance/space/local.txt --dry-run
ve-adrive-cli mkdir adrive://instance/space/new-folder
ve-adrive-cli sync ./dir adrive://instance/space/backup/ --recursive --dry-run
```

Inspect command contracts before executing:

```bash
ve-tos-cli cp --describe --output json
tos-cli cp --describe --output json
ve-adrive-cli sync --describe --output json
```

Start MCP servers for agent clients:

```bash
ve-tos-cli serve --mcp
tos-cli serve --mcp
ve-adrive-cli serve --mcp
```

`stdio` is the default transport and is intended for an Agent that starts the
CLI as a child process. To use the local SSE transport, the Agent must run on
the same machine (and in the same network namespace) as the CLI:

```bash
ve-tos-cli serve --mcp --transport sse --port 9090
```

After the loopback listener binds, the command writes a fresh credential once
to stderr:

```text
MCP SSE listening on http://127.0.0.1:9090/sse
Authorization: Bearer <TOKEN>
```

Configure the MCP client to send that value in the `Authorization` header on
both the `GET /sse` stream and every `POST /message` request. Do not put the
credential in a URL, query parameter, or MCP payload. The accepted Host values
are exactly `127.0.0.1:9090` and `localhost:9090`. Native clients may omit
`Origin`; if present, it must be an HTTP loopback origin on the same port.
`--dry-run` and `--describe` report this contract without creating a token or
opening a listener.

### Preserve directories during uploads

Local-directory uploads with `cp --recursive` and `sync` preserve selected directories, including empty directories, by default. `--disable-dir-object` skips explicit directory uploads; it is valid only for local-directory uploads. Files can still create required parents in HNS/ADrive. FNS directory markers are zero-byte keys ending in `/`; HNS directory objects use STANDARD rather than a file-only storage-class override. Both manifest and `--no-manifest` execution preserve the same directory policy. An empty source creates the effective destination folder when a non-root path exists; `--include-parent` retains the source basename. Directory tasks count as items with zero bytes. The switch does not authorize deleting source-present directories during `sync --delete`.

```bash
ve-tos-cli cp ./src/ tos://bucket/backup/ --recursive
ve-tos-cli cp ./src/ tos://bucket/backup/ --recursive --disable-dir-object
tos-cli sync ./src/ tos://bucket/backup/ --disable-dir-object
ve-adrive-cli sync ./src/ adrive://instance/space/backup/
```

本地目录通过 `cp --recursive` 或 `sync` 上传时，默认保留匹配的目录，包括空目录。`--disable-dir-object` 跳过显式目录上传，仅适用于本地目录上传方向；HNS/ADrive 的文件仍可能自动创建所需父目录。FNS 用以 `/` 结尾的零字节对象表示目录，HNS 目录使用 STANDARD，不套用文件的存储类型覆盖。普通清单和 `--no-manifest` 模式遵循相同目录规则。空源目录会创建可表达的目标文件夹，桶或空间根不创建空 Key；`--include-parent` 保留源目录名。目录计入项目数，字节数为零。关闭目录上传不会授权 `sync --delete` 删除源端仍存在的目录。

An empty `bin/` directory is stored as one zero-byte object whose key is `bin/`. ByteTOS raw listing can return the directory object itself when listing that prefix; `tos-cli ls` filters this self entry. If a console shows `bin/bin`, inspect the exact object Key or compare raw listing and HEAD results before attributing the display to either uploader or console.

空目录 `bin/` 保存为 key 为 `bin/` 的一个零字节对象。字节云 TOS 原始列举可能返回目录对象自身，`tos-cli ls` 会过滤自身条目。若控制台显示 `bin/bin`，需要核对对象详情中的完整 Key，或对照原始列举与 HEAD 结果后定位，不能只凭显示名称判断是上传端还是控制台问题。

### Request IDs in summaries and reports

Across tos, ve-tos and ve-adrive, the command-level `service_request_ids` list retains at most the first 100 usable response IDs in completion order, in every output format including JSON. `service_request_ids_omitted` counts subsequent usable IDs. The top-level `request_id` still follows the command's existing success/error rules.

Each execution report CSV row has a trailing `request_id` column. Successful and failed items retain their own service response ID when available; items without a usable response ID leave the cell empty. File-level report IDs are independent of the 100-ID command summary limit. Cleanup requests do not replace the original failed operation's ID. Report rows are matched to their own operations during concurrent execution. TOS rejects appending new rows to an old CSV header; use a new report path.

tos、ve-tos、ve-adrive 的命令级 `service_request_ids` 在所有输出格式（包括 JSON）中，均只保留按响应完成顺序收集的前 100 个有效 ID，后续数量计入 `service_request_ids_omitted`。顶层 `request_id` 继续遵循现有成功／错误规则。

执行结果 report CSV 的最后一列为 `request_id`。成功、失败项有自身服务响应 ID 就填写，没有有效 ID 则留空；逐项报告不受命令摘要前 100 个的限制。失败后的清理请求不会替换原始失败 ID，并发执行也按操作逐项对应。TOS 拒绝向旧 CSV 表头追加新记录，应使用新的报告路径。

For concurrent multipart uploads, remote copies and range downloads, a failed file row uses the ID of the part that caused the failure, even when another part succeeds later. Pure local copies and directory creation on the local filesystem have no service ID. During streaming batches, a later listing failure retains completed report rows and returns their summary and artifact paths in one failed aggregate; the command still exits with the original error code. Preflight failures before execution starts follow the existing validation/error behavior.

并发分片上传、远端复制和 Range 下载失败时，文件行使用触发失败的分片 ID，后到的成功分片不会覆盖它。纯本地复制、仅在本地创建目录的条目没有服务 ID。流式批量执行过程中，后续列举失败仍保留已完成的报告行，并在一个失败汇总中返回统计及产物路径，退出码保持原错误语义；执行前的校验或预扫描失败继续遵循既有错误行为。

Filtered recursive `mv` copies and deletes selected files only, preserving excluded source files. Effective destinations that resolve to the same source prefix or lie inside it are rejected before copying; `--include-parent` participates in this check. For TOS/ve-tos `rm --include-uploads`, planned upload-abort actions count toward the summary and appear as `multipart_upload` manifest items, including all-version deletion. HNS direct recursive deletion has one directory action in its manifest.

带筛选条件的递归 `mv` 只复制并删除选中的文件，保留排除的源文件。实际目标与源前缀相同或位于源内时，在复制前拒绝执行，校验包括 `--include-parent` 的影响。TOS/ve-tos 的 `rm --include-uploads` 将终止上传任务计入计划数，在清单中以 `multipart_upload` 类型记录；删除所有版本时也执行这些任务。HNS direct 递归删除的清单记录一个目录动作。

For HNS sources, filtered recursive moves select files only when `mv --recursive` uses an `--include` or `--exclude` filter, preserving source directories, including empty directories, and excluded files. The plan and result report contain only file copy/delete actions in both manifest modes. Without filters, HNS whole-directory rename and recursive empty-directory moves remain available.

ve-tos 以 HNS 为源时，与 ADrive 一样：`mv --recursive` 使用 `--include` 或 `--exclude` 时，只移动选中的文件，保留源目录（包括空目录）及排除的文件。两种清单模式的计划和报告只包含文件复制、删源动作；不再额外清理源目录。无筛选条件时，仍保留 HNS 整目录 rename 和空目录移动能力。

## Common Options

Most commands share these options:

| Option                      | Env           | Description                                               |
|-----------------------------|---------------|-----------------------------------------------------------|
| `-P, --profile <PROFILE>`   | `TOS_PROFILE` | Configuration profile, default `default`.                 |
| `--config-path <PATH>`      | `TOS_CONFIG_PATH` | Config TOML path, default `$HOME/.tos/config.toml`.   |
| `--credentials-path <PATH>` | `TOS_CREDENTIALS_PATH` | Credentials TOML path; defaults beside the effective config file. |
| `-r, --region <REGION>`     |               | Region override.                                          |
| `-e, --endpoint <URL>`      |               | Data-plane endpoint override.                             |
| `--control-endpoint <URL>`  |               | Control-plane endpoint override for TOS.                  |
| `--account-id <ACCOUNT_ID>` |               | Account ID for control-plane operations.                  |
| `-o, --output <FORMAT>`     | `TOS_OUTPUT`  | `json`, `yaml`, `table`, `csv`, or `markdown`.            |
| `--query <JMESPATH>`        |               | Filter structured output.                                 |
| `--dry-run`                 |               | Preview without executing supported write operations.     |
| `--describe`                |               | Print a structured command contract.                      |
| `-y, --yes`                 |               | Auto-confirm supported destructive prompts.               |
| `--confirm <RESOURCE>`      |               | Confirm critical delete operations with the exact target. |
| `--no-color [<BOOL>]`       | `NO_COLOR`    | Disable colored output.                                   |
| `-v, --verbose`             |               | Verbose logs to stderr.                                   |
| `-q, --quiet`               |               | Suppress non-error output.                                |
| `--trace-dir <DIR>`         |               | Write trace diagnostics.                                  |
| `--trace-redact <LEVEL>`    |               | `strict`, `relaxed`, or `off`; default `strict`.          |

Credential variables are resolved by the config layer:

| Variable                | Description                                                                    |
|-------------------------|--------------------------------------------------------------------------------|
| `TOS_ACCESS_KEY`        | TOS access key ID.                                                             |
| `TOS_SECRET_KEY`        | TOS secret access key.                                                         |
| `TOS_SECURITY_TOKEN`    | Optional TOS STS security token.                                               |
| `TOS_AUTH_MODE`         | VeTos authentication mode: `aksk` or `unified`; ignored by `tos-cli`.          |
| `BYTETOS_AUTH_MODE`     | ByteCloud TOS authentication mode: `aksk` or `zti`.                            |
| `SEC_TOKEN_STRING`      | Highest-priority ZTI Token source; keep its value out of logs.                |
| `ZTI_AGENT_SOCKET_PATH` | Optional Unix Agent socket path; default `/run/zti-agent.sock`.              |
| `SEC_TOKEN_PATH`        | ZTI Token file, used if no environment Token or Agent is available.          |
| `ADRIVE_ACCESS_KEY`     | ADrive access key ID.                                                          |
| `ADRIVE_SECRET_KEY`     | ADrive secret access key.                                                      |
| `ADRIVE_SECURITY_TOKEN` | Optional ADrive STS security token.                                            |
| `ADRIVE_REGION`         | ADrive signing region; required when it cannot be parsed from the configured endpoint. |
| `ADRIVE_ENDPOINT`       | Required ADrive IDS resource endpoint.                                        |
| `ADRIVE_SHARE_ENDPOINT` | Safe HTTP(S) share-page origin override (after CLI/profile config).            |
| `ADRIVE_AUTH_MODE`      | Process-scoped ADrive authentication mode: `aksk`, `oauth`, or `unified`.        |
| `ADRIVE_AUTH_ENDPOINT`  | OAuth Authorization Server used by the next explicit login.                      |
| `ADRIVE_DEFAULT_INSTANCE` | Default IDS Instance used by OAuth login.                                      |
| `ADRIVE_DEVICE_NAME`    | Device name displayed during OAuth authorization.                                |
| `ADRIVE_ACCESS_TOKEN`   | Read-only process-scoped OAuth Access Token.                                      |
| `ADRIVE_REFRESH_TOKEN`  | Process-scoped OAuth group field; it is informational unless an Access Token is also supplied. |

## Skill Installation

Choose the artifact according to the task:

| Entry | Purpose | How it is used |
|---|---|---|
| `skills/*/SKILL.md` in this repository | Agent workflows: setup, command selection, execution, recovery and verification | Install the matching CLI skill with its `references/` directory |
| `<cli> skill list --output json` | Live registry metadata and MCP input schemas | Discover tool IDs and contracts for the installed CLI version |
| `<cli> skill export` | A command reference pack with an index and individual command skills | Export locally, then install or read the selected Markdown artifact |
| `<cli> serve --mcp` | MCP server backed by the live command registry | Configure an MCP client to launch the CLI; it does not read exported Markdown |

Installing an agent skill does not install the CLI executable. Repository skills
teach multi-step workflows; exported command skills document individual command
syntax, parameters, examples and operational behavior. Use current `--help` and
`--describe` contracts if a skill and the installed binary differ.

The repository provides one installable AI-agent skill per public CLI:

```text
skills/ve-tos-cli
skills/tos-cli
skills/ve-adrive-cli
```

Each skill is a standard skill directory with `SKILL.md` plus optional
resources. Use any agent skill installer that can install from a GitHub folder
URL:

```text
https://github.com/volcengine/ve-storage-uni-cli/tree/main/skills/ve-tos-cli
https://github.com/volcengine/ve-storage-uni-cli/tree/main/skills/tos-cli
https://github.com/volcengine/ve-storage-uni-cli/tree/main/skills/ve-adrive-cli
```

For Codex skill installers, use this repo/path input:

```text
repo: volcengine/ve-storage-uni-cli
path: skills/ve-tos-cli
path: skills/tos-cli
path: skills/ve-adrive-cli
```

Install one skill by passing only the matching path or GitHub folder URL.
Restart Codex after installing skills so agents pick up the new instructions.

### Export command references

Exported commands follow the entrypoint used to generate them. Registry IDs and
MCP tool names do not change:

| Export invocation | Example inside the export |
|---|---|
| `tos-cli skill export --name tos_cp` | `tos-cli cp ...` |
| `ve-storage-uni-cli tos skill export --name tos_cp` | `ve-storage-uni-cli tos cp ...` |
| `ve-tos-cli skill export --name ve_tos_cp` | `ve-tos-cli cp ...` |
| `ve-storage-uni-cli ve-tos skill export --name ve_tos_cp` | `ve-storage-uni-cli ve-tos cp ...` |
| `ve-adrive-cli skill export --name ve_adrive_cp` | `ve-adrive-cli cp ...` |
| `ve-storage-uni-cli ve-adrive skill export --name ve_adrive_cp` | `ve-storage-uni-cli ve-adrive cp ...` |

For example, discover the exact tool ID, preview paths, then export:

```bash
tos-cli skill list --output json
tos-cli skill export --name tos_cp --language zh --dir ./tos-cp-skills --dry-run --output json
tos-cli skill export --name tos_cp --language zh --dir ./tos-cp-skills --output json
```

`--language` accepts `en` (default) or `zh`. Omit `--name` to export all command
skills. Use an exact tool ID from `skill list` for portable selection:

| CLI | Additional `--name` matching |
|---|---|
| `tos-cli` | Exact canonical command, such as `"tos cp"` |
| `ve-tos-cli` | Canonical command or suffix, such as `"ve-tos cp"` or `cp`; a suffix can match multiple commands |
| `ve-adrive-cli` | Canonical command, short command, domain, or supported legacy name |

Export creates `DIR/SKILL.md` as the index and
`DIR/{domain}/{skill_name}/SKILL.md` as command references. Both have skill
metadata. Read the root index with its children available, or install a single
command directory independently. Export itself does not install or register
anything with an agent. Keep the installed directory name consistent with the
skill's `name` frontmatter when your installer requires it.

The command refuses to overwrite existing target files, including the root
index. To update a pack, export into a fresh directory, review the result and
replace the installed copy. `--dry-run` previews paths without writing files.
The parameter table describes CLI syntax; the JSON schema describes MCP
arguments, so fields such as `execute` are not CLI flags.

## More Documentation

- [docs/README.md](docs/README.md): maintained documentation index.
- [packaging/README.md](packaging/README.md): local builds, release archives,
  GitHub Release assets, npm, PyPI, Homebrew, WinGet, curl installer, and
  release publishing.
- [docs/api_implementation_principles.md](docs/api_implementation_principles.md):
  command-surface design and implementation principles.
- [scripts/e2e/README.md](scripts/e2e/README.md): live end-to-end test setup.

## License

Apache-2.0
