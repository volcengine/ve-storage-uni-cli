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

# The release script selects these backend tools automatically when needed.
cargo install cargo-zigbuild
cargo install cargo-xwin
# Install Zig through your runner's package manager, for example:
# brew install zig
# apt-get install zig

# Python packages used for wheel build and PyPI upload.
python3 -m pip install --upgrade build twine setuptools wheel

# Authenticate publish tools before running execute/upload flags.
cargo login
gh auth login
npm login

# PyPI uploads use Twine credentials or TWINE_* environment variables.
export TWINE_USERNAME=__token__
export TWINE_PASSWORD=<pypi-api-token>
```

WinGet publishing from macOS uses generated YAML manifests plus a GitHub pull
request to `microsoft/winget-pkgs`. Authenticate `gh` with a GitHub account that
can push to your `winget-pkgs` fork before opening the PR.

| Stage                 | Required tools                                  | Notes                                                                                                                                                              |
|-----------------------|-------------------------------------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Archives and Cargo    | Rust toolchain, `cargo`, requested Rust targets | Install extra targets with `rustup target add <triple>` when cross-building. `release.py archives` selects the required backend tool for each target.             |
| GitHub Release assets | GitHub CLI (`gh`)                               | `github-release --execute` runs `gh release create`, `gh release upload`, and in auto mode `gh release view`. Authenticate with `gh auth login` or `GITHUB_TOKEN`. |
| npm packages          | Node.js, `npm`                                  | `npm --execute-publish` requires npm registry auth.                                                                                                                |
| PyPI wheels           | Python 3, `build`, `twine`, `setuptools`, `wheel` | Install with `python3 -m pip install --upgrade build twine setuptools wheel`; upload uses Twine credentials or `TWINE_*` env vars.                                  |
| Homebrew Formulae     | `git`, access to the source repository checkout | The generator writes Ruby formulae into `Formula/`; use `--tap-dir .` for this repository, and only add commit/push flags when this checkout should publish directly. |
| WinGet manifests      | Python 3, `git`, GitHub CLI (`gh`)              | Manifest generation writes `dist/winget` and can run on macOS; publish by copying those files into a `winget-pkgs` fork and opening a PR.                         |

For local smoke runs, omit execute/upload flags and the script prints the
registry commands without publishing. For `all --publish`, the runner needs all
tools above plus credentials for every registry destination. Homebrew needs
direct Git credentials only when `--homebrew-commit --homebrew-push` is used.

## Release Runbook

Use CI matrix runners for real releases. Cross-compiling from macOS can work for
some targets, but native runners are simpler and more reliable for Linux libc
variants and Windows MSVC targets.

Version arguments in this runbook are package versions without a leading `v`:
`--version <version>` expects `1.0.0`, not `v1.0.0`. GitHub Release commands
derive the tag `v1.0.0` from that value, while package wrappers use the raw
package version.

### 1. Build binary archives

Run this per target triple, preferably on a runner that can build that target:

```bash
python3 packaging/scripts/release.py archives --target x86_64-apple-darwin

python3 packaging/scripts/release.py archives --target aarch64-apple-darwin

python3 packaging/scripts/release.py archives --target x86_64-unknown-linux-gnu

python3 packaging/scripts/release.py archives --target x86_64-unknown-linux-gnu.2.17

python3 packaging/scripts/release.py archives --target aarch64-unknown-linux-gnu

python3 packaging/scripts/release.py archives --target aarch64-unknown-linux-gnu.2.17

python3 packaging/scripts/release.py archives --target x86_64-pc-windows-msvc
```

The base Linux GNU archives target the normal GNU triples. The `.2.17` Linux
GNU archives are separate compatibility assets for older glibc environments.
Upload both sets if you want current Linux environments and older glibc
environments to have distinct assets.

The archive step writes `dist/ve-storage-uni-cli-*`, `dist/SHA256SUMS`, and
`dist/install.sh`. Archives contain `bin/ve-tos-cli`, `bin/tos-cli`, and
`bin/ve-adrive-cli`.

### 2. Build package wrappers

Generate npm packages:

```bash
python3 packaging/scripts/release.py npm --version <version>
```

Generate PyPI package trees and wheels from already-built CLI binaries. Pass
the release target triple; the release script resolves the corresponding
`packaging/cargo/*/target/<triple>/release` binary directories and writes a
platform-tagged wheel:

```bash
python3 packaging/scripts/release.py pip --version <version> --target <triple> --build-wheel
```

Run this command once per PyPI target. The `dist/pip/<package>` source tree is
regenerated for each target, while release wheels accumulate under
`dist/pip/wheels/*.whl`. Upload the wheel files, not the last regenerated source
tree.

For Linux PyPI wheels, use the `.2.17` target, for example
`x86_64-unknown-linux-gnu.2.17` or `aarch64-unknown-linux-gnu.2.17`. Those
wheels carry the older-glibc-compatible binaries while the release script hides
the artifact directory mapping.

Use repeated `--binary-dir <dir>` instead of `--target` only when the binaries
come from non-standard directories.

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

When only `packaging/install/install.sh` changes after archives are already
built, refresh the release asset without rebuilding binaries:

```bash
cp packaging/install/install.sh dist/install.sh
python3 packaging/scripts/release.py checksums --out-dir dist
python3 packaging/scripts/release.py github-release --version <version> --mode upload --execute --clobber
```

Publish Cargo packages in dependency order. The root `ve-storage-uni-cli` Cargo
package is published so the three public Cargo installer crates can depend on
it. The supported user-facing Cargo installs are `cargo install ve-tos-cli`,
`cargo install tos-cli`, and `cargo install ve-adrive-cli`; npm, pip, Homebrew,
WinGet, and curl also expose only the three dedicated CLIs.

```bash
python3 packaging/scripts/release.py cargo --execute
```

Publish npm packages:

```bash
python3 packaging/scripts/release.py npm --version <version> --execute-publish
```

If your npm account requires two-factor authentication for publishes, either
pass the current one-time password:

```bash
python3 packaging/scripts/release.py npm --version <version> --execute-publish --otp <otp-code>
```

or publish with a granular npm access token that has bypass 2FA enabled.

Publish PyPI wheels:

```bash
python3 packaging/scripts/release.py pip --version <version> --target <triple> --build-wheel --upload
```

For a multi-platform release, build every target first, then upload the
accumulated `dist/pip/wheels/*.whl` set with Twine. Pip selects the matching
wheel for the user's OS and architecture during `pip install`.

Publish Homebrew Formulae into the source repository after `SHA256SUMS` exists.
Homebrew formulae are macOS-only, so the generator reads only the
`aarch64-apple-darwin` and `x86_64-apple-darwin` archive checksums. This uses
`volcengine/ve-storage-uni-cli` itself as a custom tap by keeping formula files
under the repository root `Formula/` directory:

```bash
python3 packaging/scripts/release.py homebrew --version <version> --checksums dist/SHA256SUMS --tap-dir .
```

The command above writes `Formula/*.rb` into the current source checkout. If
your local checkout is synchronized to the public GitHub repository by another
process, let that process publish the Formula files. If this checkout pushes
directly to GitHub, add `--commit --push`.

For local or single-architecture tap validation, add `--target <triple>` to
limit formula generation to one macOS archive target.

Users install through an explicit tap URL once, then install by formula name:

```bash
brew tap volcengine/ve-storage-uni-cli https://github.com/volcengine/ve-storage-uni-cli
brew install ve-tos-cli
brew install tos-cli
brew install ve-adrive-cli
```

Publish WinGet manifests from macOS by opening a PR to `microsoft/winget-pkgs`.
The local `<winget-pkgs-checkout>` should be a clone of your GitHub fork of
`microsoft/winget-pkgs`:

```bash
python3 packaging/scripts/release.py winget --version <version> --checksums dist/SHA256SUMS
git -C <winget-pkgs-checkout> checkout -b volcengine-storage-clis-<version>
mkdir -p <winget-pkgs-checkout>/manifests/v/Volcengine
rsync -a dist/winget/Volcengine/ <winget-pkgs-checkout>/manifests/v/Volcengine/
git -C <winget-pkgs-checkout> add manifests/v/Volcengine
git -C <winget-pkgs-checkout> commit -m "Add Volcengine storage CLIs <version>"
git -C <winget-pkgs-checkout> push -u origin volcengine-storage-clis-<version>
gh pr create --repo microsoft/winget-pkgs --head <github-user>:volcengine-storage-clis-<version> --title "Add Volcengine storage CLIs <version>" --body "Adds ve-tos-cli, tos-cli, and ve-adrive-cli <version>."
```

Registry authentication is handled by the underlying tools: `cargo login` or
`CARGO_REGISTRY_TOKEN`, `gh`/`GITHUB_TOKEN`, npm login/token, Twine/PyPI token,
git access to the source repository for Homebrew Formulae, and GitHub access to
your `winget-pkgs` fork.

For a CI job that has already selected a single build target, `all --publish`
runs the same flow in one entry point: build archives, upload GitHub Release
assets, publish Cargo, publish npm, build and upload PyPI wheels for
`--pip-target`, and write Homebrew formulae into the source repository. Add
`--homebrew-commit --homebrew-push` only when the current checkout should push
Formula changes directly instead of relying on repository synchronization.
For macOS release machines, run the WinGet PR flow above separately after the
GitHub Release assets are available.

```bash
python3 packaging/scripts/release.py all --version <version> --target <triple> --pip-target <triple> --publish --tap-dir . --github-release-clobber
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
