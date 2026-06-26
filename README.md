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
brew install volcengine/tap/ve-tos-cli
brew install volcengine/tap/tos-cli
brew install volcengine/tap/ve-adrive-cli
```

```powershell
winget install Volcengine.VeTosCli
winget install Volcengine.TosCli
winget install Volcengine.VeAdriveCli
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

Configure ADrive credentials with environment variables:

```bash
export ADRIVE_ACCESS_KEY=<your-adrive-access-key-id>
export ADRIVE_SECRET_KEY=<your-adrive-secret-access-key>
export ADRIVE_SECURITY_TOKEN=<optional-sts-token>
```

Initialize or inspect local configuration:

```bash
ve-tos-cli config init
ve-tos-cli config set region cn-beijing
ve-tos-cli config set endpoint https://tos-cn-beijing.volces.com
ve-tos-cli config show --output json

tos-cli config set region cn-beijing
tos-cli config set endpoint https://tos-cn-beijing.volces.com
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

## Common Options

Most commands share these options:

| Option                      | Env           | Description                                               |
|-----------------------------|---------------|-----------------------------------------------------------|
| `-P, --profile <PROFILE>`   | `TOS_PROFILE` | Configuration profile, default `default`.                 |
| `--config-path <PATH>`      | `TOS_CONFIG_PATH` | Config TOML path, default `$HOME/.tos/config.toml`.   |
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
| `ADRIVE_ACCESS_KEY`     | ADrive access key ID.                                                          |
| `ADRIVE_SECRET_KEY`     | ADrive secret access key.                                                      |
| `ADRIVE_SECURITY_TOKEN` | Optional ADrive STS security token.                                            |
| `ADRIVE_REGION`         | ADrive region, used to derive the IDS endpoint when no endpoint is configured. |
| `ADRIVE_ENDPOINT`       | ADrive IDS endpoint override.                                                  |

## Skill Installation

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

## More Documentation

- [packaging/README.md](packaging/README.md): local builds, release archives,
  GitHub Release assets, npm, PyPI, Homebrew, WinGet, curl installer, and
  release publishing.
- [docs/api_implementation_principles.md](docs/api_implementation_principles.md):
  command-surface design and implementation principles.
- [scripts/e2e/README.md](scripts/e2e/README.md): live end-to-end test setup.

## License

Apache-2.0
