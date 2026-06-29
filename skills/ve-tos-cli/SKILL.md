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

## Common Commands

```bash
ve-tos-cli ls tos://bucket/prefix/ --output json
ve-tos-cli stat tos://bucket/key --output json
ve-tos-cli cp ./file.txt tos://bucket/file.txt --dry-run
ve-tos-cli sync ./dir tos://bucket/prefix/ --dry-run
ve-tos-cli presign tos://bucket/key --expires 3600 --output json
```

Use `ve-tos-cli <command> --help` when flags are uncertain.
