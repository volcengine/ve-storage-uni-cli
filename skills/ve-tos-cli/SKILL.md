---
name: ve-tos-cli
description: Use when inspecting, configuring, uploading, downloading, copying, synchronizing, deleting, sharing, or diagnosing Volcengine TOS object storage resources with ve-tos-cli.
---

# ve-tos-cli

Use the `ve-tos-cli` binary for Volcengine TOS object storage. Check availability with `ve-tos-cli --version` before
planning commands, and use the first matching executable on `PATH` unless the user provides an explicit path. Do not run
storage operations if the binary is missing.

## CLI installation

Installing this skill does not install the CLI executable or configure credentials.
First run `ve-tos-cli --version`; if the selected executable works, skip installation.
Install only when it is in scope. Choose one channel for the current OS and
available tools. These channels are alternatives; do not run all commands.

| Channel | Suitable environment and behavior | Installation commands |
|---|---|---|
| Homebrew | macOS with Homebrew; installs a prebuilt binary | `brew tap volcengine/ve-storage-uni-cli https://github.com/volcengine/ve-storage-uni-cli`<br>`brew install ve-tos-cli` |
| npm | Node.js and npm; installs a wrapper and downloads a platform binary | `npm install -g ve-tos-cli` |
| pip | Python with pip; installs a wrapper and a matching OS/architecture wheel | `pip install ve-tos-cli` |
| Cargo | Rust toolchain and build tools; compiles the dedicated executable | `cargo install ve-tos-cli` |

<!-- [Review Fix #Install2] Preserve the npm postinstall prerequisite when inlining. -->
npm also needs access to GitHub Releases: its postinstall script downloads and
verifies the binary. Do not use `--ignore-scripts`. For pip, select the intended
Python environment and activate it when needed; `python -m pip install ve-tos-cli`
uses the pip for that interpreter. Ensure the package manager's bin/scripts
directory is on PATH. Each channel requires an available build for the OS and CPU.

**Release install script (Linux/macOS):** choose this for a prebuilt binary without
a package manager. It needs a POSIX shell, curl, archive and checksum tools;
it detects the platform, downloads a GitHub Release archive and verifies its checksum:

```bash
curl -fsSL https://github.com/volcengine/ve-storage-uni-cli/releases/latest/download/install.sh | sh -s -- ve-tos-cli
```

The script installs into `$HOME/.local/bin` by default; add that directory to PATH
or use the executable's full path. `VE_STORAGE_UNI_CLI_INSTALL_DIR` selects another
directory and `VE_STORAGE_UNI_CLI_VERSION` pins a release.

**Verify installation:** run `ve-tos-cli --version` using the selected executable.
If it is missing or reports an unexpected version, check PATH and the active
environment before reinstalling. Keep the verified path for the rest of the task.
A version check verifies executable availability, not authentication; continue
with profile/endpoint setup below and a harmless resource request.

Read `references/safety.md` before commands that write, delete, move, overwrite, sync, change config, or expose signed
URLs.

## Task routing and command contracts

Use this skill to choose and combine commands. Read [Task workflows](references/workflows.md)
when uploading, downloading, synchronizing, deleting, or diagnosing a failed operation.
Read [Safety](references/safety.md) before making changes or sharing URLs.

| User intent                 | Start here                                  | Completion evidence                                             |
|-----------------------------|---------------------------------------------|-----------------------------------------------------------------|
| Inspect or locate resources | `ls`, `stat`, then `find`/`du` if supported | Exact target, metadata, and listing completeness                |
| Upload, download, or copy   | `cp --help`, then a concrete dry-run        | Destination metadata or local file verification                 |
| Reconcile a directory       | `sync --help` and a dry-run                 | Planned versus completed transfers; deletion count if requested |
| Remove or move resources    | Exact source/target and command contract    | Remaining source/target state, including failed items           |
| Diagnose authentication     | Selected profile and mode, then `doctor`    | Successful harmless resource request                            |

Use the installed version's help and structured contract as the authority. If a flag or command is missing, inspect its
help and report the version mismatch; do not guess equivalent flags or automatically change authentication modes. Keep
the same executable, profile, endpoint, and authentication mode throughout the task unless the user requests a change.

```bash
ve-tos-cli cp --help
ve-tos-cli cp --describe ./file.txt tos://bucket/file.txt --output json
ve-tos-cli skill list --output json
ve-tos-cli skill export --help
```

`--help` explains CLI syntax; `--describe` supplies the selected command's structured contract. `capabilities` discovers
supported command groups when the right command is unknown. `skill list` discovers exportable command guides; choose an
identifier from that output and use `skill export` when persistent command documentation is needed. Export writes
documentation; it does not run storage operations or configure an Agent to load it. For a unified CLI session, retain
that session's domain-qualified entry point when requesting an export.

## Discovery

```bash
ve-tos-cli capabilities --view compact --output json
ve-tos-cli doctor --output json
ve-tos-cli config show --output json
```

## Connection configuration

`ve-tos-cli config init` writes the production defaults `cn-beijing` and
`tos-cn-beijing.volces.com`. If they are replaced, configure endpoint explicitly; the CLI never constructs an endpoint
from region. A recognizable endpoint may still supply the signing region, while a custom endpoint needs an explicit
region.

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

Unified selects the same-name profile from the external login framework. It ignores local AK/SK and does not copy
external credentials into TOS config. Run `ve login` to create or refresh that external login; the CLI only asks the SDK
for signing credentials when it sends an HTTP attempt.

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
