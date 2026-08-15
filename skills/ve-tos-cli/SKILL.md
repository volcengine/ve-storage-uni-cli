---
name: ve-tos-cli
description: Use ve-tos-cli for Volcengine TOS object storage work. Trigger this skill when a user asks an AI agent to inspect, configure, list, upload, download, sync, delete, presign, diagnose, or automate Volcengine TOS resources with the ve-tos-cli command-line tool.
---

# ve-tos-cli

Use the `ve-tos-cli` binary for Volcengine TOS object storage. Check
availability with `ve-tos-cli --version` before planning commands, and use the
first matching executable on `PATH` unless the user provides an explicit path.
Do not run storage operations if the binary is missing.

## CLI installation

If `ve-tos-cli --version` fails and the user wants installation help, suggest one
of these installation methods:

```bash
cargo install ve-tos-cli
npm install -g ve-tos-cli
pip install ve-tos-cli
brew tap volcengine/ve-storage-uni-cli https://github.com/volcengine/ve-storage-uni-cli
brew install ve-tos-cli
winget install ve-tos-cli
curl -fsSL https://github.com/volcengine/ve-storage-uni-cli/releases/latest/download/install.sh | sh -s -- ve-tos-cli
```

Read `references/safety.md` before commands that write, delete, move, overwrite,
sync, change config, or expose signed URLs.

## Discovery

```bash
ve-tos-cli capabilities --view compact --output json
ve-tos-cli doctor --output json
ve-tos-cli config show --output json
```

## Connection configuration

`ve-tos-cli config init` writes the production defaults `cn-beijing` and
`tos-cn-beijing.volces.com`. If they are replaced, configure endpoint
explicitly; the CLI never constructs an endpoint from region. A recognizable
endpoint may still supply the signing region, while a custom endpoint needs an
explicit region.

```bash
ve-tos-cli config init
ve-tos-cli config set access_key_id <volcengine-tos-access-key>
ve-tos-cli config set secret_access_key <volcengine-tos-secret-key>
ve-tos-cli config set endpoint https://tos-cn-shanghai.volces.com
```

Bare AK/SK keys are stored under the active profile's `[profile.ve-tos]`
credentials section, independently from `tos-cli` credentials.

## Authentication mode

`ve-tos-cli` supports `aksk or unified`. The precedence is `--auth-mode` >
`[profile.ve-tos].auth_mode` > `TOS_AUTH_MODE` > `aksk`.

Unified selects the same-name profile from the external login framework. It
ignores local AK/SK and does not copy external credentials into TOS config.
Run `ve login` to create or refresh that external login; the CLI only asks the
SDK for signing credentials when it sends an HTTP attempt.

```bash
ve-tos-cli --profile default --auth-mode unified ls
ve-tos-cli config set auth_mode unified
ve login
ve-tos-cli doctor --check auth --profile default
```

## Common Commands

```bash
ve-tos-cli ls tos://bucket/prefix/ --output json
ve-tos-cli stat tos://bucket/key --output json
ve-tos-cli cp ./file.txt tos://bucket/file.txt --dry-run
ve-tos-cli sync ./dir tos://bucket/prefix/ --dry-run
ve-tos-cli presign tos://bucket/key --expires 3600 --output json
```

Use `ve-tos-cli <command> --help` when flags are uncertain.
