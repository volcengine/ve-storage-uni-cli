---
name: ve-adrive-cli
description: Use when managing Volcengine A-Drive resources with ve-adrive-cli, including AK/SK, OAuth, or Unified authentication, login and status, owner-aware Space creation and listing, and file or folder operations.
---

# ve-adrive-cli

Use `ve-adrive-cli` for Volcengine ADrive. Resolve one executable before planning commands and use that exact path for
the whole task:

1. Use an explicit executable path supplied by the user.
2. Otherwise use `command -v ve-adrive-cli` and verify it with `--version`.
3. In a `ve-storage-uni-cli` source checkout, fall back to
   `packaging/cargo/ve-adrive-cli/target/release/ve-adrive-cli` when executable.

Do not run storage operations if the binary is missing or cannot be verified.

## CLI installation

Installing this skill does not install the CLI executable or configure credentials.
First run `ve-adrive-cli --version`; if the selected executable works, skip installation.
Install only when it is in scope. Choose one channel for the current OS and
available tools. These channels are alternatives; do not run all commands.

| Channel | Suitable environment and behavior | Installation commands |
|---|---|---|
| Homebrew | macOS with Homebrew; installs a prebuilt binary | `brew tap volcengine/ve-storage-uni-cli https://github.com/volcengine/ve-storage-uni-cli`<br>`brew install ve-adrive-cli` |
| npm | Node.js and npm; installs a wrapper and downloads a platform binary | `npm install -g ve-adrive-cli` |
| pip | Python with pip; installs a wrapper and a matching OS/architecture wheel | `pip install ve-adrive-cli` |
| Cargo | Rust toolchain and build tools; compiles the dedicated executable | `cargo install ve-adrive-cli` |

<!-- [Review Fix #Install2] Preserve the npm postinstall prerequisite when inlining. -->
npm also needs access to GitHub Releases: its postinstall script downloads and
verifies the binary. Do not use `--ignore-scripts`. For pip, select the intended
Python environment and activate it when needed; `python -m pip install ve-adrive-cli`
uses the pip for that interpreter. Ensure the package manager's bin/scripts
directory is on PATH. Each channel requires an available build for the OS and CPU.

**Release install script (Linux/macOS):** choose this for a prebuilt binary without
a package manager. It needs a POSIX shell, curl, archive and checksum tools;
it detects the platform, downloads a GitHub Release archive and verifies its checksum:

```bash
curl -fsSL https://github.com/volcengine/ve-storage-uni-cli/releases/latest/download/install.sh | sh -s -- ve-adrive-cli
```

The script installs into `$HOME/.local/bin` by default; add that directory to PATH
or use the executable's full path. `VE_STORAGE_UNI_CLI_INSTALL_DIR` selects another
directory and `VE_STORAGE_UNI_CLI_VERSION` pins a release.

**From a source checkout:** developers can build and verify the dedicated artifact:

```bash
cargo build --release --manifest-path packaging/cargo/ve-adrive-cli/Cargo.toml
./packaging/cargo/ve-adrive-cli/target/release/ve-adrive-cli --version
```

Only copy a verified artifact onto PATH when installation is in scope. Do not
substitute another ADrive, TOS, or general cloud CLI.

**Verify installation:** run `ve-adrive-cli --version` using the selected executable.
If it is missing or reports an unexpected version, check PATH and the active
environment before reinstalling. Keep the verified path for the rest of the task.
A version check verifies executable availability, not authentication; continue
with profile/endpoint setup below and a harmless resource request.

Read `references/safety.md` before commands that write, delete, move, overwrite, sync, change config, or expose
identifiers/URLs.

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
ve-adrive-cli cp --help
ve-adrive-cli cp --describe ./file.txt adrive://instance-id/space-id/file.txt --output json
ve-adrive-cli skill list --output json
ve-adrive-cli skill export --help
```

`--help` explains CLI syntax; `--describe` supplies the selected command's structured contract. `capabilities` discovers
supported command groups when the right command is unknown. `skill list` discovers exportable command guides; choose an
identifier from that output and use `skill export` when persistent command documentation is needed. Export writes
documentation; it does not run storage operations or configure an Agent to load it. For a unified CLI session, retain
that session's domain-qualified entry point when requesting an export.

## Discovery

Start read-only. Resolve and verify the executable first, then inspect the selected Profile. Keep full capability
discovery conditional because its compact JSON output is large; use it only when command support is uncertain:

```bash
ve-adrive-cli --version
ve-adrive-cli config show --output json
ve-adrive-cli --profile default --auth-mode oauth auth status --output json
ve-adrive-cli --profile default --auth-mode oauth doctor --check auth --output json
ve-adrive-cli capabilities --view groups --output json  # only when needed
```

Do not interpret an unset default `auth_mode` as proof that OAuth credentials are missing. Before the Profile selects a
mode, a generic `doctor` falls back to AK/SK and can produce an irrelevant missing-key warning. Use explicit OAuth
status and Auth Doctor during setup; run the generic Doctor after configuration.

## Connection configuration

ADrive has independent Resource and OAuth Authorization Server endpoints. Resource commands require an explicit
endpoint; a recognizable IDS endpoint may supply region, while a custom endpoint also needs an explicit region. New
OAuth logins additionally require an explicit Auth endpoint. The Resource and Auth endpoints must use different origins.

Endpoint values without a scheme default to `https://`; dotless service names with numeric ports, such as
`resource:9000`, are treated as hosts. Resource endpoints with an explicit non-HTTP (S) scheme are rejected before any
request.

Configure AK/SK only when the user selects that mode. Never print the supplied credentials or read them back as
plaintext:

```bash
ve-adrive-cli config set auth_mode aksk
ve-adrive-cli config set access_key_id <adrive-access-key>
ve-adrive-cli config set secret_access_key <adrive-secret-key>
ve-adrive-cli config set region cn-beijing
ve-adrive-cli config set endpoint https://ids-cn-beijing.volces.com
```

Bare AK/SK keys are stored under the active profile's `[profile.adrive]`
credentials section, independently from both TOS command surfaces.

## Authentication modes

<!-- [Review Fix #2] Keep this skill aligned with the ADrive registry and README. -->
ADrive supports `aksk, oauth, or unified`. Mode precedence is
`--auth-mode` > `[profile.adrive].auth_mode` > `ADRIVE_AUTH_MODE` > `aksk`.

Use AK/SK when the user explicitly requests it:

```bash
ve-adrive-cli ls adrive://<instance-id> --output json
```

Unified selects the same-name externally managed profile and ignores local AK/SK and OAuth credentials. The storage CLI
does not modify Unified login state; use `ve login` and `ve logout`:

```bash
ve-adrive-cli config set auth_mode unified
ve login
ve-adrive-cli --profile default --auth-mode unified ls
ve-adrive-cli doctor --check auth --output json
ve-adrive-cli ls adrive://<instance-id> --output json
```

## OAuth login

Default to OAuth for a user's cloud drive. When the user asks to set up or log in, configure the selected Profile once
so later Resource commands stay short. For normal public ADrive, require only the Instance ID and use:

- Auth base URL: `https://<instance-id>.idsauth.volces.com`
- Resource base URL: `https://idsapi.volces.com`
- Resource signing region: `cn-beijing`

The Auth host is Instance-scoped. The Resource host is shared; the CLI appends
`/v1/instances/<instance-id>/...` to Resource requests.

The signing region is a CLI connection input, not a discovered statement about where the Instance is physically
deployed.

Persist the non-secret connection settings in the selected Profile before the first login. `config set` writes one key
at a time, so set every field once and then re-read the Profile to catch a partial setup. If the user explicitly
requests a non-persistent invocation, pass the same values as command or environment overrides instead:

```bash
ve-adrive-cli config set auth_mode oauth
ve-adrive-cli config set auth_endpoint https://<instance-id>.idsauth.volces.com
ve-adrive-cli config set endpoint https://idsapi.volces.com
ve-adrive-cli config set region cn-beijing
ve-adrive-cli config set default_instance <instance-id>
ve-adrive-cli config show --output json
```

Pass only the base URL to `--auth-endpoint`; the CLI appends paths such as
`/v1/oauth/device_authorization` and `/v1/oauth/token`. Keep the foreground process running while the user completes
Device Authorization:

```bash
ve-adrive-cli auth login --output json
```

Login is a mixed-stream foreground operation: authorization instructions arrive as plain text on stderr, while the final
result uses the selected stdout format. Read stderr incrementally, preserve the original process/session, and wait for
that same process after authorization; do not start a second login.

Forward the complete verification URL exactly as the CLI returns it. The CLI accepts HTTPS URLs, plus HTTP only for
loopback development URLs, and rejects userinfo or unsafe schemes before printing. The URL is supplied by the
Authorization Server and its UI origin can differ from the Auth API origin. The CLI may not print a separate User Code;
only display one separately when it is explicitly present in the returned URL. Never print Device Codes, Access Tokens,
Refresh Tokens, authorization headers, or stored secret material.

After authorization, require the original process to succeed, then verify explicit OAuth status and Auth Doctor. Status
plus Doctor validate local credential readiness; finish a real end-to-end check with a harmless Resource command such as
`ls`. A generic Doctor is primarily diagnostic, and even its live-network check only establishes endpoint reachability
rather than business API success.

Treat an OAuth status as reusable only when `ready` is true and its
`instance_id` matches the requested Instance. Otherwise start a new explicit login for the requested Instance.

OAuth login stores returned identity metadata such as `user_id` with the selected profile. It does not decode identity
from a Token.

If a login or refresh response does not provide `user_id`, run
`ve-adrive-cli --auth-mode oauth auth login --instance instance-id` again with the OAuth-authorized Instance ID, or pass
`--owner-id` explicitly for the owner-aware operation.

If Auth returns 404, verify the Instance ID and documented Auth base URL. If a Resource request rejects a Token, inspect
explicit OAuth status, requested Instance, and Resource endpoint before deciding whether to log in again. Do not use the
Auth endpoint as the Resource endpoint.

## Space listing and ownership

After Profile setup, reuse its selected mode, Resource Server, and region. Do not repeat connection flags on normal
Resource commands:

```bash
# Spaces owned by the logged-in user.
ve-adrive-cli ls adrive://<instance-id> --owner-type user --output json

# Spaces owned by groups available to the logged-in user.
ve-adrive-cli ls adrive://<instance-id> --owner-type group --output json
```

For a deliberate non-persistent mode override, the equivalent commands are
`ve-adrive-cli --auth-mode oauth ls adrive://instance-id` and
`ve-adrive-cli --auth-mode oauth ls adrive://instance-id --owner-type group`. Normal Profile-based use should keep the
shorter commands above.

For each owner collection, read `data.is_truncated` and `data.next_marker`. When truncated, pass the opaque marker to
the next request and repeat until
`is_truncated` is false; never construct or modify a marker:

```bash
ve-adrive-cli ls adrive://<instance-id> --owner-type user \
  --max-keys 1000 --marker '<previous-next-marker>' --output json
```

Run the same loop for `--owner-type group`. Count any separately returned
`data.root_space` once across all pages. Report user-owned and group-owned counts separately before the total.

After discovering Spaces, list each Space root with the same pagination loop:

```bash
ve-adrive-cli ls adrive://<instance-id>/<space-id>/ \
  --max-keys 1000 --output json
```

For creation, OAuth user-owned Spaces default `--owner-id` to the stored
`user_id`; group-owned Spaces always require an explicit owner ID:

```bash
ve-adrive-cli crt adrive://<instance-id>/personal --owner-type user

ve-adrive-cli crt adrive://<instance-id>/team \
  --owner-type group --owner-id <group-id>
```

Instance creation retains the authentication-mode-specific service defaults:

```bash
# AK/SK defaults to arkclaw.
ve-adrive-cli crt adrive://<instance-name>

# OAuth defaults to paas after the Profile selects OAuth.
ve-adrive-cli crt adrive://<instance-name> --service-type paas
```

## File and folder commands

<!-- [Review Fix #2] SyncArgs has no --recursive option; sync examples omit it. -->

Use the mode, Resource endpoint, and region already stored in the selected Profile. Add connection flags only for a
deliberate one-off override:

```bash
ve-adrive-cli ls adrive://<instance-id>/<space-id>/path/ --output json

ve-adrive-cli stat \
  adrive://<instance-id>/<space-id>/path/file --output json

ve-adrive-cli cp ./file.txt \
  adrive://<instance-id>/<space-id>/file.txt --dry-run

ve-adrive-cli sync ./dir \
  adrive://<instance-id>/<space-id>/prefix/ --dry-run

ve-adrive-cli mkdir \
  adrive://<instance-id>/<space-id>/new-folder
```

Use `ve-adrive-cli <command> --help` when non-connection flags are uncertain.
