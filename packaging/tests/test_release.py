import argparse
import hashlib
import importlib.util
import json
import os
import platform
import shutil
import subprocess
import tarfile
from datetime import datetime, timezone
from pathlib import Path

import pytest


REPO_ROOT = Path(__file__).resolve().parents[2]
RELEASE_SCRIPT = REPO_ROOT / "packaging" / "scripts" / "release.py"
PUBLIC_CLI_PACKAGES = ("ve-tos-cli", "tos-cli", "ve-adrive-cli")


def load_release_module():
    spec = importlib.util.spec_from_file_location("release_script", RELEASE_SCRIPT)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def current_shell_installer_triple() -> str | None:
    machine = platform.machine().lower()
    if machine in ("x86_64", "amd64"):
        arch = "x86_64"
    elif machine in ("arm64", "aarch64"):
        arch = "aarch64"
    else:
        return None

    system = platform.system()
    if system == "Darwin":
        return f"{arch}-apple-darwin"
    if system == "Linux":
        ldd = shutil.which("ldd")
        if ldd:
            ldd_version = subprocess.run(
                [ldd, "--version"],
                check=False,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
            ).stdout
            if "musl" in ldd_version.lower():
                return None
        if arch == "x86_64":
            return "x86_64-unknown-linux-gnu.2.17"
        if arch == "aarch64":
            return "aarch64-unknown-linux-gnu.2.17"
    return None


def test_release_public_package_set_excludes_internal_dispatcher():
    release = load_release_module()

    assert release.PUBLIC_PACKAGES == PUBLIC_CLI_PACKAGES
    assert release.ENTRY_CRATES == PUBLIC_CLI_PACKAGES


def test_cargo_publish_steps_publish_only_required_public_cli_graph():
    release = load_release_module()

    publish_names = tuple(name for name, _ in release.CARGO_PUBLISH_STEPS)

    assert publish_names == (
        "tos-core",
        "ve-tos-cli-core",
        "tos-cli-core",
        "ve-adrive-cli-core",
        "ve-storage-uni-cli",
        "ve-tos-cli",
        "tos-cli",
        "ve-adrive-cli",
    )
    assert "ve-storage-uni-cli-runtime" not in publish_names


def test_cargo_publish_retries_transient_index_failures(monkeypatch):
    release = load_release_module()
    attempts = []
    sleeps = []

    def fake_run_command(command, execute=True, capture_output=False):
        attempts.append((tuple(command), execute, capture_output))
        if len(attempts) < 3:
            raise subprocess.CalledProcessError(101, command)

    monkeypatch.setattr(release, "run_command", fake_run_command)
    monkeypatch.setattr(release.time, "sleep", lambda delay: sleeps.append(delay))

    release.run_cargo_publish_command(["cargo", "publish", "-p", "tos-core"], execute=True)

    assert attempts == [
        (("cargo", "publish", "-p", "tos-core"), True, True),
        (("cargo", "publish", "-p", "tos-core"), True, True),
        (("cargo", "publish", "-p", "tos-core"), True, True),
    ]
    assert sleeps == list(release.CARGO_PUBLISH_RETRY_DELAYS[:2])


def test_cargo_publish_waits_until_crates_io_rate_limit_expires(monkeypatch):
    release = load_release_module()
    attempts = []
    sleeps = []
    retry_at = datetime(2026, 6, 28, 9, 1, 11, tzinfo=timezone.utc)
    observed_at = datetime(2026, 6, 28, 9, 0, 1, tzinfo=timezone.utc)
    rate_limit_stderr = (
        "the remote server responded with an error (status 429 Too Many Requests): "
        "You have published too many new crates in a short period of time. "
        "Please try again after Sun, 28 Jun 2026 09:01:11 GMT and see "
        "https://crates.io/docs/rate-limits for more details."
    )

    def fake_run_command(command, execute=True, capture_output=False):
        attempts.append((tuple(command), execute, capture_output))
        if len(attempts) == 1:
            raise subprocess.CalledProcessError(101, command, stderr=rate_limit_stderr)

    monkeypatch.setattr(release, "run_command", fake_run_command)
    monkeypatch.setattr(release.time, "time", lambda: observed_at.timestamp())
    monkeypatch.setattr(release.time, "sleep", lambda delay: sleeps.append(delay))

    release.run_cargo_publish_command(["cargo", "publish", "-p", "tos-core"], execute=True)

    assert sleeps == [int(retry_at.timestamp() - observed_at.timestamp()) + 5]


def test_cargo_publish_dry_run_does_not_retry(monkeypatch):
    release = load_release_module()
    attempts = []

    def fake_run_command(command, execute=True):
        attempts.append((tuple(command), execute))
        raise subprocess.CalledProcessError(101, command)

    monkeypatch.setattr(release, "run_command", fake_run_command)

    with pytest.raises(subprocess.CalledProcessError):
        release.run_cargo_publish_command(
            ["cargo", "publish", "-p", "tos-core", "--dry-run"],
            execute=True,
        )

    assert attempts == [(("cargo", "publish", "-p", "tos-core", "--dry-run"), True)]


def test_release_metadata_and_channel_configs_use_public_cli_packages():
    release_metadata = json.loads(
        (REPO_ROOT / "packaging" / "release" / "platforms.json").read_text(encoding="utf-8")
    )
    assert tuple(release_metadata["commands"]) == PUBLIC_CLI_PACKAGES
    release_targets = tuple(target["triple"] for target in release_metadata["targets"])
    assert "x86_64-unknown-linux-gnu" in release_targets
    assert "x86_64-unknown-linux-gnu.2.17" in release_targets
    assert "aarch64-unknown-linux-gnu" in release_targets
    assert "aarch64-unknown-linux-gnu.2.17" in release_targets
    assert "x86_64-unknown-linux-musl" not in release_targets
    assert "aarch64-pc-windows-msvc" not in release_targets

    npm_config = json.loads(
        (REPO_ROOT / "packaging" / "npm" / "packages.json").read_text(encoding="utf-8")
    )
    assert tuple(package["name"] for package in npm_config["packages"]) == PUBLIC_CLI_PACKAGES

    pip_config = json.loads(
        (REPO_ROOT / "packaging" / "pip" / "packages.json").read_text(encoding="utf-8")
    )
    assert tuple(package["name"] for package in pip_config["packages"]) == PUBLIC_CLI_PACKAGES

    homebrew_config = json.loads(
        (REPO_ROOT / "packaging" / "homebrew" / "formulae.json").read_text(encoding="utf-8")
    )
    assert tuple(formula["name"] for formula in homebrew_config["formulae"]) == PUBLIC_CLI_PACKAGES

    winget_config = json.loads(
        (REPO_ROOT / "packaging" / "winget" / "packages.json").read_text(encoding="utf-8")
    )
    assert tuple(package["moniker"] for package in winget_config["packages"]) == PUBLIC_CLI_PACKAGES
    assert tuple(package["identifier"] for package in winget_config["packages"]) == (
        "Volcengine.VeTosCli",
        "Volcengine.TosCli",
        "Volcengine.VeAdriveCli",
    )


def test_packaging_readme_documents_github_release_upload_and_release_phases():
    readme = (REPO_ROOT / "packaging" / "README.md").read_text(encoding="utf-8")

    assert "## Prerequisites" in readme
    assert "cargo install cargo-zigbuild" in readme
    assert "cargo install cargo-xwin" in readme
    assert "python3 -m pip install --upgrade build twine" in readme
    assert "rustup target add" in readme
    assert "x86_64-unknown-linux-gnu.2.17" in readme
    assert "aarch64-unknown-linux-gnu.2.17" in readme
    assert "x86_64-unknown-linux-musl" not in readme
    assert "aarch64-pc-windows-msvc" not in readme
    assert "cargo zigbuild --target" not in readme
    assert "cargo xwin build" not in readme
    assert "GitHub CLI" in readme
    assert "`gh release create`" in readme
    assert "Manifest generation writes `dist/winget` and can run on macOS" in readme
    assert "microsoft/winget-pkgs" in readme
    assert "gh pr create --repo microsoft/winget-pkgs" in readme
    assert "wingetcreate" not in readme
    assert "release.py checksums --out-dir dist" in readme
    assert "`--version <version>` expects `1.0.0`, not `v1.0.0`" in readme
    assert "For Linux PyPI wheels, use the `.2.17` target" in readme
    wrapper_section = readme[
        readme.index("### 2. Build package wrappers") : readme.index(
            "### 3. Publish package registries"
        )
    ]
    publish_section = readme[readme.index("### 3. Publish package registries") :]
    assert "release.py homebrew" not in wrapper_section
    assert "Homebrew formulae are macOS-only" not in wrapper_section
    assert "Homebrew formulae are macOS-only" in publish_section
    assert "brew tap volcengine/ve-storage-uni-cli https://github.com/volcengine/ve-storage-uni-cli" in readme
    assert "homebrew --version <version> --checksums dist/SHA256SUMS --tap-dir ." in readme
    assert "synchronized to the public GitHub repository" in readme
    assert "--homebrew-commit --homebrew-push" in readme
    assert "--tap-dir <ve-storage-uni-cli-checkout>" not in readme
    assert "volcengine/tap" not in readme
    assert "Build binary archives" in readme
    assert "Build package wrappers" in readme
    assert "Publish package registries" in readme
    assert "release.py github-release" in readme
    assert readme.index("release.py github-release") > readme.index("### 3. Publish package registries")


def test_root_readme_is_user_facing_and_points_release_docs_to_packaging():
    readme = (REPO_ROOT / "README.md").read_text(encoding="utf-8")

    assert "See [packaging/README.md](packaging/README.md)" in readme
    assert "## Installation" in readme
    assert "brew tap volcengine/ve-storage-uni-cli https://github.com/volcengine/ve-storage-uni-cli" in readme
    assert "brew install ve-adrive-cli" in readme
    assert "volcengine/tap" not in readme
    assert "## Quick Start" in readme
    assert "## Common Options" in readme
    assert "## Skill Installation" in readme
    assert "## Build" not in readme
    assert "cargo build" not in readme
    assert "cargo fmt" not in readme
    assert "install-skill-from-github.py" not in readme
    assert "skill-install " not in readme
    assert "https://github.com/volcengine/ve-storage-uni-cli/tree/main/skills/ve-tos-cli" in readme
    assert "repo: volcengine/ve-storage-uni-cli" in readme
    assert "path: skills/ve-tos-cli" in readme
    for command_name in PUBLIC_CLI_PACKAGES:
        assert command_name in readme
        assert f"### `{command_name}`" in readme


def test_packaging_readme_uses_github_skill_resource_paths():
    readme = (REPO_ROOT / "packaging" / "README.md").read_text(encoding="utf-8")

    assert "install-skill-from-github.py" not in readme
    assert "skill-install " not in readme
    assert "https://github.com/volcengine/ve-storage-uni-cli/tree/main/skills/ve-tos-cli" in readme
    assert "https://github.com/volcengine/ve-storage-uni-cli/tree/main/skills/tos-cli" in readme
    assert "https://github.com/volcengine/ve-storage-uni-cli/tree/main/skills/ve-adrive-cli" in readme
    assert "repo: volcengine/ve-storage-uni-cli" in readme
    assert "path: skills/ve-adrive-cli" in readme


def test_github_release_assets_include_archives_checksums_and_installer(tmp_path):
    release = load_release_module()
    archive = tmp_path / "ve-storage-uni-cli-x86_64-apple-darwin.tar.gz"
    checksum = tmp_path / "SHA256SUMS"
    installer = tmp_path / "install.sh"
    ignored = tmp_path / "npm"
    ignored_file = tmp_path / "ve-storage-uni-cli-not-an-archive.txt"
    archive.write_text("archive", encoding="utf-8")
    checksum.write_text("checksums", encoding="utf-8")
    installer.write_text("installer", encoding="utf-8")
    ignored_file.write_text("ignore me", encoding="utf-8")
    ignored.mkdir()

    assert release.github_release_assets(tmp_path) == [checksum, installer, archive]


def test_release_checksums_rewrites_file_entries_and_skips_directories(tmp_path):
    release = load_release_module()
    archive = tmp_path / "ve-storage-uni-cli-x86_64-apple-darwin.tar.gz"
    installer = tmp_path / "install.sh"
    ignored_dir = tmp_path / "npm"
    archive.write_text("archive", encoding="utf-8")
    installer.write_text("installer", encoding="utf-8")
    ignored_dir.mkdir()
    (tmp_path / "SHA256SUMS").write_text("stale\n", encoding="utf-8")

    args = argparse.Namespace(out_dir=tmp_path, checksums=tmp_path / "SHA256SUMS")
    release.run_checksums(args)

    checksum_lines = (tmp_path / "SHA256SUMS").read_text(encoding="utf-8").splitlines()

    assert checksum_lines == [
        f"{hashlib.sha256(installer.read_bytes()).hexdigest()}  install.sh",
        (
            f"{hashlib.sha256(archive.read_bytes()).hexdigest()}  "
            "ve-storage-uni-cli-x86_64-apple-darwin.tar.gz"
        ),
    ]


def test_release_checksums_parser_accepts_out_dir():
    release = load_release_module()

    args = release.build_parser().parse_args(["checksums", "--out-dir", "dist"])

    assert args.out_dir == Path("dist")
    assert args.checksums is None
    assert args.func == release.run_checksums


def test_github_release_create_command_uses_release_assets(tmp_path, monkeypatch):
    release = load_release_module()
    commands = []
    for name in ("SHA256SUMS", "install.sh", "ve-storage-uni-cli-x86_64-apple-darwin.tar.gz"):
        (tmp_path / name).write_text(name, encoding="utf-8")

    monkeypatch.setattr(
        release,
        "run_command",
        lambda command, execute=True: commands.append((tuple(str(part) for part in command), execute)),
    )

    args = argparse.Namespace(
        version="1.2.3",
        out_dir=tmp_path,
        repo="volcengine/ve-storage-uni-cli",
        title=None,
        notes="release notes",
        notes_file=None,
        draft=False,
        prerelease=False,
        clobber=False,
        mode="create",
        execute=True,
    )

    release.run_github_release(args)

    assert commands == [
        (
            (
                "gh",
                "release",
                "create",
                "v1.2.3",
                str(tmp_path / "SHA256SUMS"),
                str(tmp_path / "install.sh"),
                str(tmp_path / "ve-storage-uni-cli-x86_64-apple-darwin.tar.gz"),
                "--repo",
                "volcengine/ve-storage-uni-cli",
                "--title",
                "v1.2.3",
                "--notes",
                "release notes",
            ),
            True,
        )
    ]


def test_github_release_auto_uploads_when_release_exists(tmp_path, monkeypatch):
    release = load_release_module()
    commands = []
    for name in ("SHA256SUMS", "install.sh", "ve-storage-uni-cli-x86_64-apple-darwin.tar.gz"):
        (tmp_path / name).write_text(name, encoding="utf-8")

    monkeypatch.setattr(release, "github_release_exists", lambda tag, repo: True)
    monkeypatch.setattr(
        release,
        "run_command",
        lambda command, execute=True: commands.append((tuple(str(part) for part in command), execute)),
    )

    args = argparse.Namespace(
        version="1.2.3",
        out_dir=tmp_path,
        repo="volcengine/ve-storage-uni-cli",
        title=None,
        notes="",
        notes_file=None,
        draft=False,
        prerelease=False,
        clobber=True,
        mode="auto",
        execute=True,
    )

    release.run_github_release(args)

    assert commands == [
        (
            (
                "gh",
                "release",
                "upload",
                "v1.2.3",
                str(tmp_path / "SHA256SUMS"),
                str(tmp_path / "install.sh"),
                str(tmp_path / "ve-storage-uni-cli-x86_64-apple-darwin.tar.gz"),
                "--repo",
                "volcengine/ve-storage-uni-cli",
                "--clobber",
            ),
            True,
        )
    ]


def test_release_builds_three_public_entry_crates(monkeypatch):
    release = load_release_module()
    commands = []

    monkeypatch.setattr(
        release,
        "run_command",
        lambda command, execute=True: commands.append(tuple(command)),
    )

    release.build_release_binaries("x86_64-apple-darwin", skip_build=False)

    assert commands == [
        (
            "cargo",
            "build",
            "--release",
            "--target",
            "x86_64-apple-darwin",
            "--manifest-path",
            "packaging/cargo/ve-tos-cli/Cargo.toml",
        ),
        (
            "cargo",
            "build",
            "--release",
            "--target",
            "x86_64-apple-darwin",
            "--manifest-path",
            "packaging/cargo/tos-cli/Cargo.toml",
        ),
        (
            "cargo",
            "build",
            "--release",
            "--target",
            "x86_64-apple-darwin",
            "--manifest-path",
            "packaging/cargo/ve-adrive-cli/Cargo.toml",
        ),
    ]


def test_release_uses_zigbuild_for_linux_gnu_compatibility(monkeypatch):
    release = load_release_module()
    commands = []

    monkeypatch.setattr(
        release,
        "run_command",
        lambda command, execute=True: commands.append(tuple(command)),
    )

    release.build_release_binaries("x86_64-unknown-linux-gnu.2.17", skip_build=False)

    assert commands == [
        (
            "cargo",
            "zigbuild",
            "--release",
            "--target",
            "x86_64-unknown-linux-gnu.2.17",
            "--manifest-path",
            "packaging/cargo/ve-tos-cli/Cargo.toml",
        ),
        (
            "cargo",
            "zigbuild",
            "--release",
            "--target",
            "x86_64-unknown-linux-gnu.2.17",
            "--manifest-path",
            "packaging/cargo/tos-cli/Cargo.toml",
        ),
        (
            "cargo",
            "zigbuild",
            "--release",
            "--target",
            "x86_64-unknown-linux-gnu.2.17",
            "--manifest-path",
            "packaging/cargo/ve-adrive-cli/Cargo.toml",
        ),
    ]


def test_archives_uses_linux_compat_target_as_public_asset_target(tmp_path, monkeypatch):
    release = load_release_module()
    commands = []

    monkeypatch.setattr(
        release,
        "run_command",
        lambda command, execute=True: commands.append(tuple(str(part) for part in command)),
    )

    args = argparse.Namespace(
        target=["x86_64-unknown-linux-gnu.2.17"],
        skip_build=True,
        target_dir=Path("target"),
        out_dir=tmp_path / "dist",
        metadata=Path("packaging/release/platforms.json"),
    )

    release.run_archives(args)

    archive_command = commands[0]
    assert "--target" in archive_command
    assert archive_command[archive_command.index("--target") + 1] == "x86_64-unknown-linux-gnu.2.17"
    for package_name in PUBLIC_CLI_PACKAGES:
        assert (
            str(
                REPO_ROOT
                / "packaging"
                / "cargo"
                / package_name
                / "target"
                / "x86_64-unknown-linux-gnu"
                / "release"
            )
            in archive_command
        )


def test_linux_compat_binary_dirs_use_cargo_zigbuild_artifact_triple():
    release = load_release_module()

    assert release.binary_dirs_for_target(
        "x86_64-unknown-linux-gnu.2.17",
        Path("target"),
    ) == [
        REPO_ROOT
        / "packaging"
        / "cargo"
        / package_name
        / "target"
        / "x86_64-unknown-linux-gnu"
        / "release"
        for package_name in PUBLIC_CLI_PACKAGES
    ]


def test_release_uses_zigbuild_for_base_linux_gnu(monkeypatch):
    release = load_release_module()
    commands = []

    monkeypatch.setattr(
        release,
        "run_command",
        lambda command, execute=True: commands.append(tuple(command)),
    )

    release.build_release_binaries("x86_64-unknown-linux-gnu", skip_build=False)

    assert commands[0] == (
        "cargo",
        "zigbuild",
        "--release",
        "--target",
        "x86_64-unknown-linux-gnu",
        "--manifest-path",
        "packaging/cargo/ve-tos-cli/Cargo.toml",
    )


def test_release_uses_xwin_for_windows_msvc_from_macos(monkeypatch):
    release = load_release_module()
    commands = []

    monkeypatch.setattr(
        release,
        "run_command",
        lambda command, execute=True: commands.append(tuple(command)),
    )

    release.build_release_binaries("x86_64-pc-windows-msvc", skip_build=False)

    assert commands[0] == (
        "cargo",
        "xwin",
        "build",
        "--release",
        "--target",
        "x86_64-pc-windows-msvc",
        "--manifest-path",
        "packaging/cargo/ve-tos-cli/Cargo.toml",
    )


def test_common_installers_do_not_select_unpublished_targets():
    installer = (REPO_ROOT / "packaging" / "install" / "install.sh").read_text(encoding="utf-8")
    npm_script = (REPO_ROOT / "packaging" / "scripts" / "npm.py").read_text(encoding="utf-8")
    winget_script = (REPO_ROOT / "packaging" / "scripts" / "winget.py").read_text(
        encoding="utf-8"
    )

    assert "x86_64-unknown-linux-musl" not in installer
    assert "aarch64-pc-windows-msvc" not in installer
    assert "x86_64-unknown-linux-musl" not in npm_script
    assert "aarch64-pc-windows-msvc" not in npm_script
    assert "aarch64-pc-windows-msvc" not in winget_script
    assert "aarch64-unknown-linux-gnu.2.17" in installer
    assert "aarch64-unknown-linux-gnu.2.17" in npm_script


def test_shell_installer_creates_extract_directory_before_tar():
    installer = (REPO_ROOT / "packaging" / "install" / "install.sh").read_text(encoding="utf-8")

    mkdir_index = installer.index('mkdir -p "$dest_dir"')
    tar_index = installer.index('tar -xzf "$archive_path" -C "$dest_dir"')
    unzip_index = installer.index('unzip -q "$archive_path" -d "$dest_dir"')

    assert mkdir_index < tar_index
    assert mkdir_index < unzip_index


def test_shell_installer_installs_from_local_release_archive(tmp_path):
    if shutil.which("sh") is None or shutil.which("curl") is None:
        pytest.skip("shell installer smoke test requires sh and curl")
    triple = current_shell_installer_triple()
    if triple is None:
        pytest.skip("current platform is not supported by install.sh")

    release_dir = tmp_path / "repo" / "releases" / "latest" / "download"
    release_dir.mkdir(parents=True)
    stage_dir = tmp_path / "stage"
    bin_dir = stage_dir / "bin"
    bin_dir.mkdir(parents=True)
    source_binary = bin_dir / "ve-tos-cli"
    source_binary.write_text("#!/bin/sh\nprintf 'fake ve-tos-cli\\n'\n", encoding="utf-8")
    source_binary.chmod(0o755)

    archive_name = f"ve-storage-uni-cli-{triple}.tar.gz"
    archive_path = release_dir / archive_name
    with tarfile.open(archive_path, "w:gz") as tar_file:
        tar_file.add(source_binary, arcname="bin/ve-tos-cli")
    archive_digest = hashlib.sha256(archive_path.read_bytes()).hexdigest()
    (release_dir / "SHA256SUMS").write_text(
        f"{archive_digest}  {archive_name}\n",
        encoding="utf-8",
    )

    install_dir = tmp_path / "install-bin"
    env = os.environ.copy()
    env.update(
        {
            "VE_STORAGE_UNI_CLI_REPO_URL": (tmp_path / "repo").as_uri(),
            "VE_STORAGE_UNI_CLI_INSTALL_DIR": str(install_dir),
        }
    )

    subprocess.run(
        ["sh", str(REPO_ROOT / "packaging" / "install" / "install.sh"), "ve-tos-cli"],
        check=True,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )

    installed_binary = install_dir / "ve-tos-cli"
    assert installed_binary.read_text(encoding="utf-8") == source_binary.read_text(
        encoding="utf-8"
    )
    assert os.access(installed_binary, os.X_OK)


def test_pip_parser_accepts_target_without_binary_dirs():
    release = load_release_module()

    args = release.build_parser().parse_args(
        [
            "pip",
            "--version",
            "1.2.3",
            "--target",
            "x86_64-apple-darwin",
        ]
    )

    assert args.target == "x86_64-apple-darwin"
    assert args.binary_dir is None


def test_pip_target_expands_to_public_entry_binary_dirs(tmp_path, monkeypatch):
    release = load_release_module()
    commands = []

    monkeypatch.setattr(
        release,
        "run_command",
        lambda command, execute=True: commands.append(
            (tuple(str(part) for part in command), execute)
        ),
    )

    args = argparse.Namespace(
        version="1.2.3",
        binary_dir=None,
        target="x86_64-apple-darwin",
        out_dir=tmp_path / "pip",
        build_wheel=False,
        upload=False,
    )

    release.run_pip(args)

    expected_binary_dirs = [
        REPO_ROOT
        / "packaging"
        / "cargo"
        / package_name
        / "target"
        / "x86_64-apple-darwin"
        / "release"
        for package_name in PUBLIC_CLI_PACKAGES
    ]
    expected_package_command = [
        release.sys.executable,
        str(REPO_ROOT / "packaging" / "scripts" / "pip.py"),
        "--version",
        "1.2.3",
        "--out-dir",
        str(tmp_path / "pip"),
    ]
    for binary_dir in expected_binary_dirs:
        expected_package_command.extend(["--binary-dir", str(binary_dir)])

    assert commands[0] == (
        tuple(expected_package_command),
        True,
    )


def test_pip_linux_compat_target_uses_cargo_zigbuild_artifact_dirs(tmp_path, monkeypatch):
    release = load_release_module()
    commands = []

    monkeypatch.setattr(
        release,
        "run_command",
        lambda command, execute=True: commands.append(
            (tuple(str(part) for part in command), execute)
        ),
    )

    args = argparse.Namespace(
        version="1.2.3",
        binary_dir=None,
        target="x86_64-unknown-linux-gnu.2.17",
        out_dir=tmp_path / "pip",
        build_wheel=False,
        upload=False,
    )

    release.run_pip(args)

    assert "x86_64-unknown-linux-gnu.2.17" not in " ".join(commands[0][0])
    for package_name in PUBLIC_CLI_PACKAGES:
        assert (
            str(
                REPO_ROOT
                / "packaging"
                / "cargo"
                / package_name
                / "target"
                / "x86_64-unknown-linux-gnu"
                / "release"
            )
            in commands[0][0]
        )


def test_root_package_is_publishable_to_crates_io():
    root_manifest = (REPO_ROOT / "Cargo.toml").read_text(encoding="utf-8")

    assert "publish = false" not in root_manifest


def test_public_entry_crates_depend_on_root_package_not_runtime_crate():
    for package_name in PUBLIC_CLI_PACKAGES:
        manifest = (
            REPO_ROOT / "packaging" / "cargo" / package_name / "Cargo.toml"
        ).read_text(encoding="utf-8")

        assert "ve-storage-uni-cli = " in manifest
        assert "ve-storage-uni-cli-runtime" not in manifest


def test_all_publish_enables_external_publish_steps(tmp_path, monkeypatch):
    release = load_release_module()
    calls = []

    monkeypatch.setattr(release, "run_cargo", lambda args: calls.append(("cargo", args.execute)))
    monkeypatch.setattr(release, "run_archives", lambda args: calls.append(("archives", None)))
    monkeypatch.setattr(
        release,
        "run_github_release",
        lambda args: calls.append(("github-release", args.execute, args.mode)),
    )
    monkeypatch.setattr(
        release,
        "run_npm",
        lambda args: calls.append(("npm", args.execute_publish)),
    )
    monkeypatch.setattr(
        release,
        "run_pip",
        lambda args: calls.append(("pip", args.build_wheel, args.upload)),
    )
    monkeypatch.setattr(
        release,
        "run_homebrew",
        lambda args: calls.append(("homebrew", args.commit, args.push)),
    )
    monkeypatch.setattr(release, "run_winget", lambda args: calls.append(("winget", args.submit)))

    args = argparse.Namespace(
        version="1.2.3",
        target=["x86_64-apple-darwin"],
        skip_build=True,
        target_dir=Path("target"),
        out_dir=Path("dist"),
        metadata=Path("packaging/release/platforms.json"),
        checksums=Path("dist/SHA256SUMS"),
        pip_target=None,
        access="public",
        tag="latest",
        execute_publish=False,
        build_wheel=False,
        upload=False,
        tap_dir=tmp_path / "homebrew-tap",
        submit=False,
        execute_cargo=False,
        cargo_dry_run=False,
        allow_dirty=False,
        publish=True,
        homebrew_commit=False,
        homebrew_push=False,
        homebrew_commit_message=None,
        execute_github_release=False,
        github_release=False,
        github_repo="volcengine/ve-storage-uni-cli",
        github_release_mode="auto",
        github_release_title=None,
        github_release_notes="",
        github_release_notes_file=None,
        github_release_draft=False,
        github_release_prerelease=False,
        github_release_clobber=True,
    )

    release.run_all(args)

    assert calls == [
        ("archives", None),
        ("github-release", True, "auto"),
        ("cargo", True),
        ("npm", True),
        ("pip", True, True),
        ("homebrew", False, False),
        ("winget", False),
    ]


def test_all_publish_respects_explicit_homebrew_commit_and_push(tmp_path, monkeypatch):
    release = load_release_module()
    calls = []

    monkeypatch.setattr(release, "run_cargo", lambda args: None)
    monkeypatch.setattr(release, "run_archives", lambda args: None)
    monkeypatch.setattr(release, "run_github_release", lambda args: None)
    monkeypatch.setattr(release, "run_npm", lambda args: None)
    monkeypatch.setattr(release, "run_pip", lambda args: None)
    monkeypatch.setattr(
        release,
        "run_homebrew",
        lambda args: calls.append(("homebrew", args.commit, args.push)),
    )
    monkeypatch.setattr(release, "run_winget", lambda args: None)

    args = argparse.Namespace(
        version="1.2.3",
        target=["x86_64-apple-darwin"],
        skip_build=True,
        target_dir=Path("target"),
        out_dir=Path("dist"),
        metadata=Path("packaging/release/platforms.json"),
        checksums=Path("dist/SHA256SUMS"),
        pip_target=None,
        access="public",
        tag="latest",
        execute_publish=False,
        build_wheel=False,
        upload=False,
        tap_dir=tmp_path / "source-repo",
        submit=False,
        execute_cargo=False,
        cargo_dry_run=False,
        allow_dirty=False,
        publish=True,
        homebrew_commit=True,
        homebrew_push=True,
        homebrew_commit_message=None,
        execute_github_release=False,
        github_release=False,
        github_repo="volcengine/ve-storage-uni-cli",
        github_release_mode="auto",
        github_release_title=None,
        github_release_notes="",
        github_release_notes_file=None,
        github_release_draft=False,
        github_release_prerelease=False,
        github_release_clobber=True,
    )

    release.run_all(args)

    assert calls == [("homebrew", True, True)]


def test_homebrew_publish_commits_and_pushes_tap(tmp_path, monkeypatch):
    release = load_release_module()
    commands = []

    formula_out_dir = tmp_path / "formulae"
    formula_out_dir.mkdir()
    (formula_out_dir / "ve-adrive-cli.rb").write_text("class VeAdriveCli\nend\n", encoding="utf-8")

    def record_command(command, execute=True):
        commands.append((tuple(str(part) for part in command), execute))

    monkeypatch.setattr(release, "run_command", record_command)

    tap_dir = tmp_path / "homebrew-tap"
    (tap_dir / "packaging" / "homebrew").mkdir(parents=True)
    (tap_dir / "packaging" / "homebrew" / "formulae.json").write_text("{}", encoding="utf-8")
    subprocess.run(
        ["git", "init"],
        cwd=tap_dir,
        check=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    subprocess.run(
        [
            "git",
            "remote",
            "add",
            "origin",
            "git@example.internal:storage/tos-uni-cli.git",
        ],
        cwd=tap_dir,
        check=True,
    )
    args = argparse.Namespace(
        version="1.2.3",
        checksums=Path("dist/SHA256SUMS"),
        out_dir=formula_out_dir,
        tap_dir=tap_dir,
        commit=True,
        push=True,
        commit_message=None,
    )

    release.run_homebrew(args)

    assert (tap_dir / "Formula" / "ve-adrive-cli.rb").read_text(encoding="utf-8") == (
        "class VeAdriveCli\nend\n"
    )
    assert commands[-4:] == [
        (("git", "-C", str(tap_dir), "status", "--short"), True),
        (("git", "-C", str(tap_dir), "add", "Formula"), True),
        (
            (
                "git",
                "-C",
                str(tap_dir),
                "commit",
                "-m",
                "Release Volcengine storage CLIs v1.2.3",
            ),
            True,
        ),
        (("git", "-C", str(tap_dir), "push"), True),
    ]


def test_homebrew_publish_rejects_non_source_repository_dir(tmp_path, monkeypatch):
    release = load_release_module()
    formula_out_dir = tmp_path / "formulae"
    formula_out_dir.mkdir()

    monkeypatch.setattr(release, "run_command", lambda command, execute=True: None)

    args = argparse.Namespace(
        version="1.2.3",
        checksums=Path("dist/SHA256SUMS"),
        out_dir=formula_out_dir,
        tap_dir=tmp_path / "not-ve-storage-uni-cli",
        commit=True,
        push=False,
        commit_message=None,
    )

    with pytest.raises(SystemExit, match="ve-storage-uni-cli checkout"):
        release.run_homebrew(args)


def test_homebrew_publish_allows_internal_source_repository_remote(tmp_path, monkeypatch):
    release = load_release_module()
    commands = []
    formula_out_dir = tmp_path / "formulae"
    formula_out_dir.mkdir()
    (formula_out_dir / "ve-tos-cli.rb").write_text("class VeTosCli\nend\n", encoding="utf-8")
    tap_dir = tmp_path / "internal-repo"
    (tap_dir / "packaging" / "homebrew").mkdir(parents=True)
    (tap_dir / "packaging" / "homebrew" / "formulae.json").write_text("{}", encoding="utf-8")
    subprocess.run(
        ["git", "init"],
        cwd=tap_dir,
        check=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    subprocess.run(
        ["git", "remote", "add", "origin", "git@example.internal:storage/tos-uni-cli.git"],
        cwd=tap_dir,
        check=True,
    )

    monkeypatch.setattr(
        release,
        "run_command",
        lambda command, execute=True: commands.append((tuple(str(part) for part in command), execute)),
    )

    args = argparse.Namespace(
        version="1.2.3",
        checksums=Path("dist/SHA256SUMS"),
        out_dir=formula_out_dir,
        tap_dir=tap_dir,
        commit=False,
        push=False,
        commit_message=None,
    )

    release.run_homebrew(args)

    assert (tap_dir / "Formula" / "ve-tos-cli.rb").read_text(encoding="utf-8") == (
        "class VeTosCli\nend\n"
    )
    assert commands[-1] == (("git", "-C", str(tap_dir), "status", "--short"), True)


def test_homebrew_target_is_forwarded_to_generator(tmp_path, monkeypatch):
    release = load_release_module()
    commands = []

    monkeypatch.setattr(
        release,
        "run_command",
        lambda command, execute=True: commands.append(
            (tuple(str(part) for part in command), execute)
        ),
    )

    args = argparse.Namespace(
        version="1.2.3",
        checksums=Path("dist/SHA256SUMS"),
        out_dir=tmp_path / "formulae",
        tap_dir=None,
        commit=False,
        push=False,
        commit_message=None,
        homebrew_target="aarch64-apple-darwin",
    )

    release.run_homebrew(args)

    assert commands[0] == (
        (
            release.sys.executable,
            str(REPO_ROOT / "packaging" / "scripts" / "homebrew.py"),
            "--version",
            "1.2.3",
            "--checksums",
            "dist/SHA256SUMS",
            "--out-dir",
            str(tmp_path / "formulae"),
            "--target",
            "aarch64-apple-darwin",
        ),
        True,
    )


def test_homebrew_parser_accepts_single_target_option():
    release = load_release_module()

    args = release.build_parser().parse_args(
        [
            "homebrew",
            "--version",
            "1.2.3",
            "--target",
            "aarch64-apple-darwin",
        ]
    )

    assert args.homebrew_target == "aarch64-apple-darwin"


def test_homebrew_push_requires_commit(tmp_path, monkeypatch):
    release = load_release_module()
    formula_out_dir = tmp_path / "formulae"
    formula_out_dir.mkdir()
    monkeypatch.setattr(release, "run_command", lambda command, execute=True: None)

    args = argparse.Namespace(
        version="1.2.3",
        checksums=Path("dist/SHA256SUMS"),
        out_dir=formula_out_dir,
        tap_dir=tmp_path / "homebrew-tap",
        commit=False,
        push=True,
        commit_message=None,
    )

    with pytest.raises(SystemExit, match="--push requires --commit"):
        release.run_homebrew(args)
