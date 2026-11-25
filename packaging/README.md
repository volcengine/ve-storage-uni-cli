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

## ByteCloud TOS ZTI release contract

The public `tos-cli` source build includes ZTI in `tos-core`; no private ZTI
crate or Cargo registry is required. The three standalone entry crates keep
their `Cargo.lock` files in this repository, and the release archive builder
uses `--locked` so archive builds cannot silently select a different dependency graph.
The archive builder packages the dedicated binaries. npm copies each CLI's
binaries from those archives into one self-contained package per CLI; Homebrew
and WinGet download the release archives. pip copies the build outputs into wheels.
The `ve-tos-cli` and `ve-adrive-cli` binaries continue to reject
`--auth-mode zti`.

On Unix platforms, `tos-cli` supports `SEC_TOKEN_STRING`, a local ZTI Agent
socket, and `SEC_TOKEN_PATH`; on Windows it supports the environment and file
sources but not the Unix Agent. `tos-cli presign` requires AK/SK. A release
runner building its own native target checks the built `tos-cli --describe`
authentication contract before creating an archive, including when
`--skip-build` is used. Cross-target binaries cannot be executed by that
runner; validate them on native CI runners before publishing. This check does
not contact an Agent or ByteTOS service.

For a local source and dedicated-binary check, run:

```bash
cargo test --manifest-path packaging/cargo/tos-cli/Cargo.toml --locked
cargo test --manifest-path packaging/cargo/ve-tos-cli/Cargo.toml --locked
cargo test --manifest-path packaging/cargo/ve-adrive-cli/Cargo.toml --locked
python3 -m pytest packaging/tests/test_cargo_publish_metadata.py packaging/tests/test_release.py
```

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

`packaging/scripts/release.py` uses the Python standard library for packaging.
Executing the internal `byte-tos` upload channel additionally requires the
Python `bytedtos` SDK from the internal package index:

```bash
python3 -m pip install bytedtos --index-url=https://bytedpypi.byted.org/simple
```

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

| Stage                 | Required tools                                    | Notes                                                                                                                                                                 |
|-----------------------|---------------------------------------------------|-----------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Archives and Cargo    | Rust toolchain, `cargo`, requested Rust targets   | Install extra targets with `rustup target add <triple>` when cross-building. `release.py archives` selects the required backend tool for each target.                 |
| GitHub Release assets | GitHub CLI (`gh`)                                 | `github-release --execute` runs `gh release create`, `gh release upload`, and in auto mode `gh release view`. Authenticate with `gh auth login` or `GITHUB_TOKEN`.    |
| npm packages          | Node.js, `npm`                                    | `npm --execute-publish` requires npm registry auth.                                                                                                                   |
| PyPI wheels           | Python 3, `build`, `twine`, `setuptools`, `wheel` | Install with `python3 -m pip install --upgrade build twine setuptools wheel`; upload uses Twine credentials or `TWINE_*` env vars.                                    |
| Homebrew Formulae     | `git`, access to the source repository checkout   | The generator writes Ruby formulae into `Formula/`; use `--tap-dir .` for this repository, and only add commit/push flags when this checkout should publish directly. |
| WinGet manifests      | Python 3, `git`, GitHub CLI (`gh`)                | Manifest generation writes `dist/winget` and can run on macOS; publish by copying those files into a `winget-pkgs` fork and opening a PR.                             |

Source builds use reqwest 0.13 with Rustls/AWS-LC and require the target's native C
build toolchain in addition to Rust; some AWS-LC configurations also require
CMake. Cross-build runners must provide the matching native toolchain. Release
binaries validate HTTPS through platform certificate facilities; Linux images
must include their system CA bundle (usually the `ca-certificates` package).

For TLS regression checks, run `cargo test -p tos-core --test test_tls`. After
building the unified and three dedicated debug binaries into `target/debug`,
run `python3 tests/http_tls_regression.py` to check untrusted-CA rejection on all
six command surfaces. An opt-in, credential-free system-trust probe is available:
`VE_STORAGE_TLS_TEST_URL=https://example.com cargo test -p tos-core --test test_tls platform_and_sdk_clients_accept_system_trusted_https -- --ignored --exact`.

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

For an official build that includes the publisher's Doubao application, inject
`STORAGE_CLI_BUILD_DOUBAO_APP_ID` and `STORAGE_CLI_BUILD_DOUBAO_APP_SECRET`
through the protected local/SCM build environment,
then add `--require-ask-credentials` to `archives` or `all`.

| Build environment variable | Value source |
| --- | --- |
| `STORAGE_CLI_BUILD_DOUBAO_APP_ID` | The publisher's App ID. |
| `STORAGE_CLI_BUILD_DOUBAO_APP_SECRET` | The same application's App Secret, supplied through a protected input. |

Both variables must be present and valid. The CLI requests the fixed scopes
`doubao:session:write`, `doubao:session.event:read`, and `offline_access` for
session creation, reply streaming, and refresh credentials. Enable those user
permissions for the publisher application before real authorization testing;
see the [permission sources](../docs/ask-direct.md#21-准备应用).
`STORAGE_CLI_BUILD_DOUBAO_SCOPES` is no longer read and can be removed from CI.

For a local build, run the following in **Bash**, replacing the App ID placeholder
with the actual value. Secret input is hidden and
does not appear in shell command history. Use the target for your build runner;
the example below is for macOS arm64.

```bash
set +x
export STORAGE_CLI_BUILD_DOUBAO_APP_ID='<publisher App ID>'
read -r -s -p 'App Secret: ' STORAGE_CLI_BUILD_DOUBAO_APP_SECRET
printf '\n'
export STORAGE_CLI_BUILD_DOUBAO_APP_SECRET

python3 packaging/scripts/release.py archives \
  --target aarch64-apple-darwin --require-ask-credentials

unset STORAGE_CLI_BUILD_DOUBAO_APP_SECRET
```

For **SCM builds**, configure App ID as a build environment variable.
Store App Secret in SCM's protected secret configuration and map it to the exact
environment variable name `STORAGE_CLI_BUILD_DOUBAO_APP_SECRET` in the Cargo build
step. The build script then runs the same `release.py archives` command above;
no Secret value belongs in the script. Do not enable shell tracing (`set -x`) or
print the build environment. Every target runner must receive the complete
application configuration.

Do not put secret values in source, command arguments, logs, or this runbook.
The build emits a value-free `Doubao ask application: bundled` or `not bundled`
notice. Cargo inherits exported environment variables; it does not automatically
load `.env` files or unexported shell variables. Both inputs are tracked for
rebuilds. Inspect the newly built binary's `ask --describe` output and require
`application_bundled: true`; use the matching debug/release/custom target path.
Later Cargo builds or tests without the inputs may replace the configured
artifact with an unconfigured one.

`--require-ask-credentials` requires a fresh build and cannot be combined with
`--skip-build`. Native binaries are inspected with a read-only describe
operation; cross-built binaries still require acceptance on their target
platform. Keep using those same build outputs when making package wrappers.
Rotating the application Secret requires rebuilding and distributing new
binaries. Customers running the built binaries do not need these build
environment variables. Binaries and compiler caches contain application
credentials; compile-time injection does not prevent their extraction.

The flag verifies embedded application configuration, not provider permissions,
user login, or operating-system support. A customer-side `cargo install` compiles
source without the publisher's private environment. See the
[direct ask guide](../docs/ask-direct.md) for customer usage, internal overrides,
credential rotation, and remaining platform acceptance work.

### 2. Build package wrappers

Collect the five npm platform archives in `dist` before generating npm packages.
The required targets are:

| OS | Architecture | Release target |
| --- | --- | --- |
| macOS | x64 | `x86_64-apple-darwin` |
| macOS | arm64 | `aarch64-apple-darwin` |
| Linux (glibc 2.17+) | x64 | `x86_64-unknown-linux-gnu.2.17` |
| Linux (glibc 2.17+) | arm64 | `aarch64-unknown-linux-gnu.2.17` |
| Windows | x64 | `x86_64-pc-windows-msvc` |

Generate npm packages from the collected release archives with one command:

```bash
python3 packaging/scripts/release.py npm --version <version>
```

The script reads `dist` by default, calculates and atomically creates or completes
`dist/SHA256SUMS`, validates the required files, and generates the packages.
Existing checksum entries must still match; a mismatch fails without overwriting
the expected value. You do not need to run `checksums` or `npm pack` yourself.
`npm publish` performs the actual npm tarball packaging when publication is enabled.
Use `--archive-dir <directory>` only when archives are outside `dist`; it is an
optional override, not part of the normal release command.

All three local packages are generated, but npm publishing includes only
`ve-tos-cli` and `ve-adrive-cli`. The npm name `tos-cli` is already occupied
by another publisher and is excluded from both publish previews and execution,
including `all --publish`.

Each package contains only its own CLI, with all five platform binaries under
`vendor/<target>/bin/`. There are no extra platform npm packages. Generation
verifies archive SHA256 values and copies only regular binary entries; missing
archives, missing binaries or invalid checksums fail before npm publication.
Use a clean, version-specific archive directory, verify every binary's reported
version, and never mix assets from different versions. `--version` labels the
package; it does not rebuild or change the embedded binary versions.

The npm launcher requires Node.js 18 or later. Its installation behavior is:

1. npm downloads the full package, including all supported platform binaries.
2. `postinstall` validates the current platform's bundled binary and does not
   contact GitHub when it is present and valid. Installation and execution also
   work with `npm install --ignore-scripts` because binaries are already included.
3. Only when the current platform's binary is missing does `postinstall` download
   the same version's archive from GitHub. Both archive and extracted binary must
   match hashes pinned in the npm package's `lib/binaries.json`. Downloads have
   HTTPS-only redirects, a 30-second deadline per request, at most five redirects
   and a 256 MiB archive-size limit. Temporary files are removed after success or
   failure.
4. A present but corrupt binary, unsupported platform (including Linux musl),
   permission error or CLI business error does not trigger a download. A missing
   binary after installation is reported by the launcher; repair it by reinstalling
   with installation scripts enabled.

`VE_STORAGE_UNI_CLI_REPO_URL` can select an HTTPS mirror for fallback only; it
must serve the identical archived bytes. `VE_STORAGE_UNI_CLI_VERSION` may be
unset or equal the package version (with an optional `v` prefix). `latest` or a
different version is rejected when fallback is needed. These variables do not
replace a healthy bundled binary.

If npm cannot download the package itself, its scripts cannot run and cannot
automatically switch to GitHub. Use the [curl installer](#curl-installer) as a
separate installation path. The package fallback only covers a missing binary
after npm has obtained the package.

For normal publication, add `--execute-publish` to the same command. The script
checks all required platform payloads before invoking `npm publish`. For a single
CLI, add `--package ve-adrive-cli` or `--package ve-tos-cli`. Without
`--execute-publish`, it generates packages and prints the publish command only.

Packaging regression tests exercise real npm tarballs and offline installs:

```bash
python3 -m pytest packaging/tests/test_npm.py packaging/tests/test_release.py
```

The generated package must include all five binaries for the selected CLI and none
for other CLIs. Native release acceptance should test installation on macOS,
Linux and Windows runners; local fixture tests do not replace native acceptance.
Published npm versions are immutable: existing `1.0.4` packages cannot receive
these files. Publish the new layout under an unused version, for example `1.0.5`.
Retain the old GitHub assets because existing versions still download them.

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

The PyPI channel also generates a `byted-tos-cli` compatibility distribution
that packages the same `tos-cli` command binary.

For Linux PyPI wheels, use the `.2.17` target, for example
`x86_64-unknown-linux-gnu.2.17` or `aarch64-unknown-linux-gnu.2.17`. Those
wheels carry the older-glibc-compatible binaries while the release script hides
the artifact directory mapping.

Use repeated `--binary-dir <dir>` instead of `--target` only when the binaries
come from non-standard directories.

Generate Homebrew Formulae after `SHA256SUMS` exists. Homebrew formulae are macOS-only,
so the generator reads only the `aarch64-apple-darwin` and
`x86_64-apple-darwin` archive checksums:

```bash
python3 packaging/scripts/release.py homebrew --version <version> --checksums dist/SHA256SUMS --tap-dir .
```

This writes `Formula/*.rb` into the current source checkout. For local or
single-architecture tap validation, add `--target <triple>` to limit formula
generation to one macOS archive target.

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

The release script does not prompt for or pass a Cargo token. Cargo obtains
credentials from `CARGO_REGISTRY_TOKEN`, a saved `cargo login` credential
(`$CARGO_HOME/credentials.toml`, usually `~/.cargo/credentials.toml`), or a
configured credential provider such as the OS keychain. With `cargo:token`,
the environment token takes precedence over the saved token. Existing
credentials explain why publishing can succeed without another prompt.
See the [Cargo authentication documentation](https://doc.rust-lang.org/cargo/reference/registry-authentication.html).

Publish npm packages (`ve-tos-cli` and `ve-adrive-cli` only):

```bash
npm login --registry=https://registry.npmjs.org/
npm whoami --registry=https://registry.npmjs.org/
python3 packaging/scripts/release.py npm --version <version> --execute-publish
```

Log in with an account that maintains both packages. npm saves the login
credentials in the user configuration and reuses them on later publishes.
If publishing requires a one-time password, pass the current code:

```bash
python3 packaging/scripts/release.py npm --version <version> --execute-publish --otp <otp-code>
```

If publishing reports `E404` or "could not be found or you do not have
permission", first check `npm whoami --registry=https://registry.npmjs.org/`
and `npm owner ls ve-tos-cli --registry=https://registry.npmjs.org/`.
A `401 Unauthorized` from `whoami` means the current credentials are not
accepted; log in again with `npm login --registry=https://registry.npmjs.org/`
and confirm that the authenticated account maintains both packages.
Adding `--otp` does not repair invalid credentials.

Publish PyPI wheels:

```bash
python3 packaging/scripts/release.py pip --version <version> --upload
```

Build every target first using the commands in step 2, then upload the
accumulated `dist/pip/wheels/*.whl` set with Twine. Upload-only mode needs
neither `--target`, `--binary-dir`, nor local build outputs and does not
regenerate package trees or build wheels. `--target` selects binaries and
the wheel platform tag during generation/building; it never filters uploads.
Combining `--target <triple> --build-wheel --upload` still builds one target
and uploads every accumulated wheel. Pip selects the matching wheel for the
user's OS and architecture during `pip install`.

The upload scans every `.whl` in that directory, including older versions:
`--version` does not filter the upload set. Start each release with a clean
wheel directory or use a separate `--out-dir` consistently for its builds
and upload. An empty wheel directory fails before Twine is invoked.

Publish internal ByteCloud TOS packages from the archives and `SHA256SUMS`
built in step 1. Collect every platform archive for this version into the
same source directory before publishing. Preview preparation and uploads:

```bash
python3 packaging/scripts/release.py byte-tos --version <version>
```

Add `--execute` to upload and read back each object for SHA-256 verification:

```bash
python3 packaging/scripts/release.py byte-tos --version <version> --execute
```

The bucket is public-read. Object key `<prefix>/<key>` maps to
`https://tosv.byted.org/obj/tos-team/<prefix>/<key>`, so the download base defaults
to `https://tosv.byted.org/obj/tos-team/toscli/new`. Neither
`--download-base-url` nor `--profile` is required. Use `--download-base-url`
only to override the download route with a different HTTPS mirror.
The command uses all supported release archives present in `dist/` by default;
repeat `--target <triple>` to select targets or use `--source-dir` to read a
separate collected release. It creates dedicated archives containing only
`bin/tos-cli` (Windows: `bin/tos-cli.exe`) and `LICENSE`, plus `SHA256SUMS` under
`dist/byte-tos/<version>/`. It uploads them to `tos://tos-team/toscli/new/<version>/`
and publishes `install.sh` at `tos://tos-team/toscli/new/install.sh` last.
No manifest or latest JSON is generated. This is a separate explicit channel;
`all --publish` does not upload to the internal bucket.

Uploads and read-back verification call the Python `bytedtos` SDK directly.
No `tos-cli` executable is installed, located, extracted for execution, or
invoked; cross-platform release packages can be published from the same runner.
Install the SDK into the Python environment used to run `release.py`:

```bash
python3 -m pip install bytedtos --index-url=https://bytedpypi.byted.org/simple
```

Set `BYTE_TOSTEAM_ACCESS_KEY` to the single AccessKey generated for the
`tos-team` bucket in that process's environment before publishing. A separate
`BYTE_TOSTEAM_SECRET_KEY` is not required or read. The script constructs
`bytedtos.Client("tos-team", access_key, endpoint="tos-cn-north.byted.org")`
using the SDK's bucket-AccessKey mode. Uploads and read-back verification use
this explicit endpoint instead of BNS/PSM service discovery; the script does
not configure service-discovery IPv6. Run it from an environment that can
resolve and reach this endpoint. The credentials must authorize
writing `tos-team/toscli/new/` and reading back the uploaded objects. The script
does not read CLI profiles or other credential variables; `--profile`,
`--auth-mode`, and `--tos-cli-path` are not used by this channel.
Public-read access makes installation anonymous but does not grant write access.
The release script does not prompt for, accept on the command line, or print
secret values. Previewing does not load the SDK or require credentials.

Supply the version value, for example:
`python3 packaging/scripts/release.py byte-tos --version 1.0.3 --execute`.

Versioned uploads use `If-None-Match: *`; existing objects are read back and must
match the locally prepared hash. Use `--clobber` only to deliberately replace
a version's contents. The root installer is updated after versioned objects
have been verified. SDK requests have a 600-second read timeout and a
10-second connect timeout by default; adjust `--timeout` as needed. Transient
429/5xx responses are retried with bounded backoff and reopened upload streams.
SDK responses and the client session are closed after use.

Users install through the internal HTTPS endpoint with `curl`, `tar` (or
`unzip` for Windows through a POSIX shell), and `sha256sum` or `shasum`:

```bash
curl -fsSL https://tosv.byted.org/obj/tos-team/toscli/new/install.sh -o install.sh
sh install.sh --version <version>
```

For example, the macOS arm64 archive for 1.0.3 is downloaded from
`https://tosv.byted.org/obj/tos-team/toscli/new/1.0.3/tos-cli-1.0.3-aarch64-apple-darwin.tar.gz`.

The installer detects the platform, downloads the versioned archive, verifies
its SHA-256, and installs only `tos-cli` into `~/.local/bin`. Add that directory
to PATH or set `--install-dir <directory>`. Versions are explicit; Linux uses
the `.2.17` glibc archives, and musl Linux is unsupported. No existing `tos-cli`
or TOS credentials are needed for installation through the download endpoint.

Publish the Homebrew Formulae generated in step 2 through the source repository.
This uses `volcengine/ve-storage-uni-cli` itself as a custom tap by keeping
formula files under the repository root `Formula/` directory. If your local
checkout is synchronized to the public GitHub repository by another process,
let that process publish the Formula files. If this checkout pushes directly
to GitHub, commit and push the generated `Formula/*.rb` files; the generator
also supports `--commit --push` for that workflow.

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
`CARGO_REGISTRY_TOKEN`, `gh`/`GITHUB_TOKEN`, npm login, Twine/PyPI token,
git access to the source repository for Homebrew Formulae, and GitHub access to
your `winget-pkgs` fork.

`all --publish` runs the same flow in one entry point: build archives, validate
and stage all npm packages, upload GitHub Release
assets, publish Cargo, publish npm, build and upload PyPI wheels for
`--pip-target`, and write Homebrew formulae into the source repository. Add
`--homebrew-commit --homebrew-push` only when the current checkout should push
Formula changes directly instead of relying on repository synchronization.
For macOS release machines, run the WinGet PR flow above separately after the
GitHub Release assets are available. npm staging runs before any channel is
published, so incomplete npm payloads abort the release early. All five npm
archives must be present in `--out-dir`, either collected beforehand or built
using all five `--target` arguments:

```bash
python3 packaging/scripts/release.py all --version <version> \
  --target x86_64-apple-darwin --target aarch64-apple-darwin \
  --target x86_64-unknown-linux-gnu.2.17 --target aarch64-unknown-linux-gnu.2.17 \
  --target x86_64-pc-windows-msvc --pip-target <triple> \
  --publish --tap-dir . --github-release-clobber
```

### Single-CLI npm and PyPI release (ADrive example)

`ve-adrive-cli` can be released independently to npm and PyPI. Choose an
unpublished version for the selected package; it does not need to match the
versions of `ve-tos-cli` or `tos-cli`. Publish matching GitHub assets to keep
the missing-binary fallback available; normal npm installs use bundled files.
Use `--package ve-adrive-cli` throughout this procedure.
The `all --publish` command and the unfiltered registry commands publish
multiple packages and are outside this procedure.

Before building, update the version in `crates/adrive/Cargo.toml` and
`packaging/cargo/ve-adrive-cli/Cargo.toml`, and refresh the affected lockfiles
and dependency requirements as needed. The binary reports the ADrive core
version. `--version` sets npm/PyPI wrapper metadata and the GitHub release
reference; it does not change the Rust package versions or relabel a binary.
Other CLI and shared crate versions only change when their own changes require
a release. Build from the final reviewed commit after it has reached the
public source repository, and verify the binary and wrapper versions agree.

Replace `<version>` with the selected version and keep it in the release terminal
for the commands below. Use a clean `dist` directory for this release:

```bash
release_version='<version>'
```

Build an ADrive-only `ve-storage-uni-cli-<target>` archive for each of the five
npm targets on its matching runner. Collect the archives into the same release
directory. The npm package will include all five ADrive binaries, and fallback
will use identical archives from the GitHub Release tagged `v<version>`.
Build each target and collect its archive in `dist`:

```bash
python3 packaging/scripts/release.py archives --package ve-adrive-cli --target <triple>
```

The npm targets are `x86_64-apple-darwin`, `aarch64-apple-darwin`,
`x86_64-unknown-linux-gnu.2.17`, `aarch64-unknown-linux-gnu.2.17`, and
`x86_64-pc-windows-msvc`. The Linux `.2.17` targets need `cargo-zigbuild`,
and Windows MSVC needs `cargo-xwin`, as in the general release runbook.
Check each archive contains only `bin/ve-adrive-cli` (or `.exe` on Windows),
plus the packaged documentation, and that its binary reports the selected
version. Generate the package and complete checksums with one preview command:

```bash
python3 packaging/scripts/release.py npm --version "$release_version" --package ve-adrive-cli
```

This validates the five archives, creates or completes `dist/SHA256SUMS`, and
generates `dist/npm/ve-adrive-cli`. It only prints the publish command; no npm
package is uploaded. No separate `checksums` or `npm pack` step is required.

Before uploading, confirm the package versions and GitHub tag are
available; existing versions and tags must not be overwritten or retargeted.
Keep the ADrive-only GitHub Release out of the generic `latest` installer
channel because that installer expects all three CLIs:

```bash
gh release create "v$release_version" dist/ve-storage-uni-cli-* dist/SHA256SUMS \
  --repo volcengine/ve-storage-uni-cli --target <public-commit-sha> --latest=false \
  --title "ve-adrive-cli v$release_version" --notes 'ADrive-only release.'
```

After the fallback assets are available, publish the ADrive npm package with the
same command plus `--execute-publish`:

```bash
python3 packaging/scripts/release.py npm --version "$release_version" --package ve-adrive-cli \
  --execute-publish
```

Without `--execute-publish`, package generation runs and the publish command is
only printed. With it, the same complete package is generated and `npm publish`
is executed. Neither command publishes `ve-tos-cli` or creates platform subpackages.
Do not reuse a published version such as `1.0.4`; choose an unused version such
as `1.0.5`, after checking the selected package's registry history.

Only custom archive layouts need `--archive-dir <directory>`; optionally change
generated package output with `--out-dir <directory>`. Keep these overrides out
of the default workflow unless the release job actually uses custom paths.

Build the PyPI wheel once per supported wheel target. Collect the wheels in
the same release-specific `wheels/` directory, then upload only the selected
package and version. PyPI wheels contain the binary and do not download
GitHub assets, so a PyPI-only release does not require a GitHub Release:

```bash
python3 packaging/scripts/release.py pip --version "$release_version" --package ve-adrive-cli \
  --target <triple> --build-wheel
python3 packaging/scripts/release.py pip --version "$release_version" --package ve-adrive-cli \
  --upload
```

For PyPI Linux wheels use the `.2.17` triples in `platforms.json`; the plain
GNU triples have no default manylinux wheel tag. The `--package` selector also
filters upload files by package and version if the wheel directory contains
unrelated files. This procedure publishes no Cargo, Homebrew, WinGet, or
ByteCloud TOS artifacts.

Choose the version for this package and verify its availability in each
selected registry before publishing.

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

See [Skill Installation and export](../README.md#skill-installation) for the
difference between repository workflow skills, CLI-exported command references
and the registry-backed MCP server. Exports preserve the dedicated or unified
entrypoint that generated them; installing a skill does not install a binary.

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
