---
name: tos-cli
description: Use when inspecting, configuring, uploading, downloading, copying, synchronizing, deleting, sharing, or diagnosing ByteCloud TOS object storage resources with tos-cli.
---

# tos-cli

Use the `tos-cli` binary for ByteCloud TOS object storage. Check availability with `tos-cli --version` before planning
commands, and use the first matching executable on `PATH` unless the user provides an explicit path. Do not run storage
operations if the binary is missing.

## CLI installation

Installing this skill does not install the CLI executable or configure credentials.
First run `tos-cli --version`; if the selected executable works, skip installation.
Install only when it is in scope. Choose one channel for the current OS and
available tools. These channels are alternatives; do not run all commands.

| Channel | Suitable environment and behavior | Installation commands |
|---|---|---|
| Homebrew | macOS with Homebrew; installs a prebuilt binary | `brew tap volcengine/ve-storage-uni-cli https://github.com/volcengine/ve-storage-uni-cli`<br>`brew install tos-cli` |
| npm | Node.js and npm; installs a wrapper and downloads a platform binary | `npm install -g tos-cli` |
| pip | Python with pip; installs a wrapper and a matching OS/architecture wheel | `pip install tos-cli` |
| Cargo | Rust toolchain and build tools; compiles the dedicated executable | `cargo install tos-cli` |

<!-- [Review Fix #Install2] Preserve the npm postinstall prerequisite when inlining. -->
npm also needs access to GitHub Releases: its postinstall script downloads and
verifies the binary. Do not use `--ignore-scripts`. For pip, select the intended
Python environment and activate it when needed; `python -m pip install tos-cli`
uses the pip for that interpreter. Ensure the package manager's bin/scripts
directory is on PATH. Each channel requires an available build for the OS and CPU.

**Release install script (Linux/macOS):** choose this for a prebuilt binary without
a package manager. It needs a POSIX shell, curl, archive and checksum tools;
it detects the platform, downloads a GitHub Release archive and verifies its checksum:

```bash
curl -fsSL https://github.com/volcengine/ve-storage-uni-cli/releases/latest/download/install.sh | sh -s -- tos-cli
```

The script installs into `$HOME/.local/bin` by default; add that directory to PATH
or use the executable's full path. `VE_STORAGE_UNI_CLI_INSTALL_DIR` selects another
directory and `VE_STORAGE_UNI_CLI_VERSION` pins a release.

### ByteCloud internal network: direct binaries (1.0.2)

On the ByteCloud internal network, select the binary for the current OS below.
These addresses are fixed at **1.0.2**, apply only to `tos-cli`, and require access
to the internal host. They do not specify CPU architecture; verify compatibility
if the downloaded executable cannot run.

| Operating system | Binary download |
|---|---|
| Linux | [tos-cli for Linux](https://tosv.byted.org/obj/tos-team/toscli/new/1.0.2/linux/tos-cli) |
| macOS | [tos-cli for macOS](https://tosv.byted.org/obj/tos-team/toscli/new/1.0.2/mac/tos-cli) |
| Windows | [tos-cli.exe for Windows](https://tosv.byted.org/obj/tos-team/toscli/new/1.0.2/win/tos-cli.exe) |

Download the selected file into a new directory. On Linux/macOS, run
`chmod +x ./tos-cli`, then `./tos-cli --version`. On Windows PowerShell, run
`.\tos-cli.exe --version`. No package manager or compilation is needed.
Use the verified executable by its full path, or place it in a directory on PATH
when installation is in scope.

**Verify installation:** run `tos-cli --version` using the selected executable.
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
tos-cli cp --help
tos-cli cp --describe ./file.txt tos://bucket/file.txt --output json
tos-cli skill list --output json
tos-cli skill export --help
```

`--help` explains CLI syntax; `--describe` supplies the selected command's structured contract. `capabilities` discovers
supported command groups when the right command is unknown. `skill list` discovers exportable command guides; choose an
identifier from that output and use `skill export` when persistent command documentation is needed. Export writes
documentation; it does not run storage operations or configure an Agent to load it. For a unified CLI session, retain
that session's domain-qualified entry point when requesting an export.

## Discovery

```bash
tos-cli capabilities --view compact --output json
tos-cli doctor --output json
tos-cli config show --output json
```

## Connection configuration

`tos-cli config init` does not choose a region, endpoint, or PSM. Configure an endpoint explicitly; a recognizable
endpoint may supply the signing region, while a custom endpoint also needs an explicit region. For ByteTOS
PSM mode, configure PSM without an endpoint; region is optional.

```bash
tos-cli config set access_key_id <byte-tos-access-key>
tos-cli config set secret_access_key <byte-tos-secret-key>
tos-cli config set endpoint https://your-bytetos-endpoint.example.com
tos-cli config set region cn-beijing
# Alternative PSM mode in another profile (no endpoint or region in that profile):
tos-cli config set psm-profile.psm toutiao.tos.tosapi
```

Bare AK/SK keys are stored under the active profile's `[profile.tos]`
credentials section, independently from `ve-tos-cli` credentials.

`aksk` is the default authentication mode. For ByteCloud ZTI, use
`tos-cli --auth-mode zti ls tos://bucket/` or set
`tos-cli config set auth_mode zti` for the active `[profile.tos]` section.
The mode priority is `--auth-mode` > `[profile.tos].auth_mode` >
`BYTETOS_AUTH_MODE` > `aksk`. ZTI uses `SEC_TOKEN_STRING`, then a local Agent
at `ZTI_AGENT_SOCKET_PATH` (default `/run/zti-agent.sock`), then
`SEC_TOKEN_PATH`. The Agent source requires Unix; a Token file is re-read for
each credential request and the Agent cache refreshes within 600 seconds or
before expiration. ZTI ignores AK/SK and does not fall back to them.
`doctor --check auth` reports the selected mode and configured source offline;
it does not read the Token or verify remote access. Never put Token values in
CLI/MCP arguments, logs, or skill files. ZTI does not support presign; only use
AK/SK for signed URLs when that mode is authorized by the user.

## Common Commands

```bash
tos-cli ls tos://bucket/prefix/ --output json
tos-cli stat tos://bucket/key --output json
tos-cli cp ./file.txt tos://bucket/file.txt --dry-run
tos-cli sync ./dir tos://bucket/prefix/ --dry-run
tos-cli presign tos://bucket/key --expires 3600 --output json
```

Use `tos-cli <command> --help` when flags are uncertain.
