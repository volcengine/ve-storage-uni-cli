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
`ADRIVE_AUTH_ENDPOINT`; one of these sources is required for every new login.

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

- [packaging/README.md](packaging/README.md): local builds, release archives,
  GitHub Release assets, npm, PyPI, Homebrew, WinGet, curl installer, and
  release publishing.
- [docs/api_implementation_principles.md](docs/api_implementation_principles.md):
  command-surface design and implementation principles.
- [scripts/e2e/README.md](scripts/e2e/README.md): live end-to-end test setup.

## License

Apache-2.0
