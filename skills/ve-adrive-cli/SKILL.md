---
name: ve-adrive-cli
description: Use ve-adrive-cli for Volcengine A-Drive file storage work. Trigger this skill when a user asks an AI agent to inspect, configure, list, upload, download, sync, delete, create folders, diagnose, or automate A-Drive resources with the ve-adrive-cli command-line tool.
---

# ve-adrive-cli

Use the `ve-adrive-cli` binary for Volcengine ADrive. Check availability with
`ve-adrive-cli --version` before planning commands, and use the first matching
executable on `PATH` unless the user provides an explicit path. Do not run storage operations if the binary is missing.

## CLI installation

If `ve-adrive-cli --version` fails and the user wants installation help, suggest
one of these installation methods:

```bash
cargo install ve-adrive-cli
npm install -g ve-adrive-cli
pip install ve-adrive-cli
brew tap volcengine/ve-storage-uni-cli https://github.com/volcengine/ve-storage-uni-cli
brew install ve-adrive-cli
winget install ve-adrive-cli
curl -fsSL https://github.com/volcengine/ve-storage-uni-cli/releases/latest/download/install.sh | sh -s -- ve-adrive-cli
```

Read `references/safety.md` before commands that write, delete, move, overwrite,
sync, change config, or expose identifiers/URLs.

## Discovery

```bash
ve-adrive-cli capabilities --view compact --output json
ve-adrive-cli doctor --output json
ve-adrive-cli config show --output json
```

## Common Commands

```bash
ve-adrive-cli ls adrive://instance/space/path/ --output json
ve-adrive-cli stat adrive://instance/space/path/file --output json
ve-adrive-cli cp ./file.txt adrive://instance/space/file.txt --dry-run
ve-adrive-cli sync ./dir adrive://instance/space/prefix/ --dry-run
ve-adrive-cli mkdir adrive://instance/space/new-folder
```

Use `ve-adrive-cli <command> --help` when flags are uncertain.
