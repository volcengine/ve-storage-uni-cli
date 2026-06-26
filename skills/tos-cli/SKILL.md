---
name: tos-cli
description: Use tos-cli for ByteCloud TOS object storage work. Trigger this skill when a user asks an AI agent to inspect, configure, list, upload, download, sync, delete, presign, diagnose, or automate ByteCloud TOS resources with the tos-cli command-line tool.
---

# tos-cli

Use the `tos-cli` binary for ByteCloud TOS object storage. Check availability
with `tos-cli --version` before planning commands, and use the first matching
executable on `PATH` unless the user provides an explicit path. Do not run storage operations if the binary is missing.

## CLI installation

If `tos-cli --version` fails and the user wants installation help, suggest one of
these installation methods:

```bash
cargo install tos-cli
npm install -g tos-cli
pip install tos-cli
brew install volcengine/tap/tos-cli
winget install Volcengine.TosCli
curl -fsSL https://github.com/volcengine/ve-storage-uni-cli/releases/latest/download/install.sh | sh -s -- tos-cli
```

Read `references/safety.md` before commands that write, delete, move, overwrite,
sync, change config, or expose signed URLs.

## Discovery

```bash
tos-cli capabilities --view compact --output json
tos-cli doctor --output json
tos-cli config show --output json
```

## Common Commands

```bash
tos-cli ls tos://bucket/prefix/ --output json
tos-cli stat tos://bucket/key --output json
tos-cli cp ./file.txt tos://bucket/file.txt --dry-run
tos-cli sync ./dir tos://bucket/prefix/ --dry-run
tos-cli presign tos://bucket/key --expires 3600 --output json
```

Use `tos-cli <command> --help` when flags are uncertain.
