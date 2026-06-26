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

# Packaging and Distribution

This directory keeps the package-manager wrappers for the three public install
names:

```text
ve-tos-cli
tos-cli
ve-adrive-cli
```

The Rust implementation remains shared. The `ve-storage-uni-cli` dispatcher is
available as the root Cargo package for local development and cross-surface
testing, but npm, pip, Homebrew, WinGet, release archives, and curl-based
installation expose only the three dedicated public CLIs.

## Local Builds

Build the dispatcher:

```bash
cargo build --release --bin ve-storage-uni-cli
```

Build the dedicated TOS entry binary:

```bash
cargo build --release --manifest-path packaging/cargo/ve-tos-cli/Cargo.toml
```

Build the dedicated ByteCloud TOS entry binary:

```bash
cargo build --release --manifest-path packaging/cargo/tos-cli/Cargo.toml
```

Build the dedicated ADrive entry binary:

```bash
cargo build --release --manifest-path packaging/cargo/ve-adrive-cli/Cargo.toml
```

The ADrive binary is written to:

```text
packaging/cargo/ve-adrive-cli/target/release/ve-adrive-cli
```

When building with `--target <triple>`, the binary is written under
`packaging/cargo/ve-adrive-cli/target/<triple>/release/`.

## What Solves What

- `packaging/install/install.sh` is the curl installer. It downloads a GitHub
  Release archive, verifies `SHA256SUMS`, and installs one selected CLI or all
  three public CLIs.
- `skills/{ve-tos-cli,tos-cli,ve-adrive-cli}/SKILL.md` are installable
  AI-agent skills. They teach agents how to use each CLI, but they do not
  install the CLI binaries.
- `packaging/scripts/release.py` builds release archives, uploads GitHub Release
  assets, generates wrapper packages, and invokes registry-specific publish
  commands.

## Prerequisites

`packaging/scripts/release.py` itself uses only the Python standard library.
Full release execution shells out to channel-specific tools, so install the
tools for the stages you plan to run:

```bash
# Rust targets used by release archives. Install only the targets needed by the runner.
rustup target add x86_64-apple-darwin
rustup target add aarch64-apple-darwin
rustup target add x86_64-unknown-linux-gnu
rustup target add aarch64-unknown-linux-gnu
rustup target add x86_64-pc-windows-msvc

# Python packages used for wheel build and PyPI upload.
python3 -m pip install --upgrade build twine

# Authenticate publish tools before running execute/upload flags.
cargo login
gh auth login
npm login

# PyPI uploads use Twine credentials or TWINE_* environment variables.
export TWINE_USERNAME=__token__
export TWINE_PASSWORD=<pypi-api-token>
```

On Windows release runners, install WinGet manifest tooling before `--submit`:

```powershell
winget install Microsoft.WingetCreate
```

| Stage                 | Required tools                                  | Notes                                                                                                                                                              |
|-----------------------|-------------------------------------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Archives and Cargo    | Rust toolchain, `cargo`, requested Rust targets | Install extra targets with `rustup target add <triple>` when cross-building.                                                                                       |
| GitHub Release assets | GitHub CLI (`gh`)                               | `github-release --execute` runs `gh release create`, `gh release upload`, and in auto mode `gh release view`. Authenticate with `gh auth login` or `GITHUB_TOKEN`. |
| npm packages          | Node.js, `npm`                                  | `npm --execute-publish` requires npm registry auth.                                                                                                                |
| PyPI wheels           | Python 3, `build`, `twine`                      | Install with `python3 -m pip install --upgrade build twine`; upload uses Twine credentials or `TWINE_*` env vars.                                                  |
| Homebrew tap          | `git`, access to the tap checkout               | The generator writes Ruby formulae; `--commit --push` runs git in `--tap-dir`.                                                                                     |
| WinGet                | `wingetcreate`                                  | `--submit` runs `wingetcreate submit`.                                                                                                                             |

For local smoke runs, omit execute/upload flags and the script prints the
registry commands without publishing. For `all --publish`, the runner needs all
tools above plus credentials for every destination.

## Release Runbook

Use CI matrix runners for real releases. Cross-compiling from macOS can work for
some targets, but native runners are simpler and more reliable for Linux libc
variants and Windows MSVC targets.

### 1. Build binary archives

Run this per target triple, preferably on a runner that can build that target:

```bash
python3 packaging/scripts/release.py archives --target x86_64-apple-darwin

python3 packaging/scripts/release.py archives --target aarch64-apple-darwin

python3 packaging/scripts/release.py archives --target x86_64-unknown-linux-gnu

python3 packaging/scripts/release.py archives --target aarch64-unknown-linux-gnu

python3 packaging/scripts/release.py archives --target x86_64-pc-windows-msvc
```

The archive step writes `dist/ve-storage-uni-cli-*`, `dist/SHA256SUMS`, and
`dist/install.sh`. Archives contain `bin/ve-tos-cli`, `bin/tos-cli`, and
`bin/ve-adrive-cli`.

### 2. Build package wrappers

Generate npm packages:

```bash
python3 packaging/scripts/release.py npm --version <version>
```

Generate PyPI package trees and wheels for the current platform. Pass the same
target triple used by the archive build:

```bash
python3 packaging/scripts/release.py pip --version <version> --target <triple> --build-wheel
```

Use repeated `--binary-dir <dir>` instead of `--target` only when the binaries
come from non-standard directories.

Generate Homebrew formulae after `SHA256SUMS` exists:

```bash
python3 packaging/scripts/release.py homebrew --version <version> --checksums dist/SHA256SUMS
```

For local or single-platform tap validation, limit formula generation to one
archive target:

```bash
python3 packaging/scripts/release.py homebrew --version <version> --checksums dist/SHA256SUMS --target <triple>
```

Generate WinGet manifests after Windows archive checksums exist:

```bash
python3 packaging/scripts/release.py winget --version <version> --checksums dist/SHA256SUMS
```

### 3. Publish package registries

Upload the release archives, `SHA256SUMS`, and `install.sh` to GitHub Releases
through `release.py` before publishing channels that download from GitHub:

```bash
python3 packaging/scripts/release.py github-release --version <version> --mode create --execute
```

If the release already exists:

```bash
python3 packaging/scripts/release.py github-release --version <version> --mode upload --execute --clobber
```

Use `--mode auto` to run `gh release view` first, then create the release if it
is missing or upload assets if it already exists. GitHub upload requires `gh`
plus `gh auth login` locally or `GITHUB_TOKEN` in CI.

Publish Cargo packages in dependency order. The root `ve-storage-uni-cli` Cargo
package is published so the three public Cargo installer crates can depend on
it; npm, pip, Homebrew, WinGet, and curl still expose only the three dedicated
CLIs.

```bash
python3 packaging/scripts/release.py cargo --execute
```

Publish npm packages:

```bash
python3 packaging/scripts/release.py npm --version <version> --execute-publish
```

Publish PyPI wheels:

```bash
python3 packaging/scripts/release.py pip --version <version> --target <triple> --build-wheel --upload
```

Publish the Homebrew tap:

```bash
python3 packaging/scripts/release.py homebrew --version <version> --checksums dist/SHA256SUMS --tap-dir <homebrew-tap-checkout> --commit --push
```

Submit WinGet manifests:

```bash
python3 packaging/scripts/release.py winget --version <version> --checksums dist/SHA256SUMS --submit
```

Registry authentication is handled by the underlying tools: `cargo login` or
`CARGO_REGISTRY_TOKEN`, `gh`/`GITHUB_TOKEN`, npm login/token, Twine/PyPI token,
git access to the Homebrew tap, and `wingetcreate` authentication.

For a CI job that has already selected a single build target, `all --publish`
runs the same flow in one entry point: build archives, upload GitHub Release
assets, publish Cargo, publish npm, build and upload PyPI wheels for
`--pip-target`, commit/push Homebrew formulae, and submit WinGet manifests.

```bash
python3 packaging/scripts/release.py all --version <version> --target <triple> --pip-target <triple> --publish --tap-dir <homebrew-tap-checkout> --github-release-clobber
```

## curl Installer

Install all three CLIs:

```bash
curl -fsSL https://github.com/volcengine/ve-storage-uni-cli/releases/latest/download/install.sh | sh
```

Install one CLI:

```bash
curl -fsSL https://github.com/volcengine/ve-storage-uni-cli/releases/latest/download/install.sh | sh -s -- ve-tos-cli
curl -fsSL https://github.com/volcengine/ve-storage-uni-cli/releases/latest/download/install.sh | sh -s -- tos-cli
curl -fsSL https://github.com/volcengine/ve-storage-uni-cli/releases/latest/download/install.sh | sh -s -- ve-adrive-cli
```

Override version or install directory:

```bash
VE_STORAGE_UNI_CLI_VERSION=v<version> VE_STORAGE_UNI_CLI_INSTALL_DIR="$HOME/.local/bin" sh packaging/install/install.sh ve-adrive-cli
```

## AI-Agent Skills

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
Restart Codex after installing skills. The generated `skill/SKILL.md` MCP
catalog is not checked in; regenerate it with `scripts/generate_skill_md.py`
only when you need a local MCP tool catalog for development.
