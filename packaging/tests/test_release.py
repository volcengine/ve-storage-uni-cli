import argparse
import importlib.util
import json
import subprocess
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

    def fake_run_command(command, execute=True):
        attempts.append((tuple(command), execute))
        if len(attempts) < 3:
            raise subprocess.CalledProcessError(101, command)

    monkeypatch.setattr(release, "run_command", fake_run_command)
    monkeypatch.setattr(release.time, "sleep", lambda delay: sleeps.append(delay))

    release.run_cargo_publish_command(["cargo", "publish", "-p", "tos-core"], execute=True)

    assert attempts == [
        (("cargo", "publish", "-p", "tos-core"), True),
        (("cargo", "publish", "-p", "tos-core"), True),
        (("cargo", "publish", "-p", "tos-core"), True),
    ]
    assert sleeps == list(release.CARGO_PUBLISH_RETRY_DELAYS[:2])


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
    assert "python3 -m pip install --upgrade build twine" in readme
    assert "rustup target add" in readme
    assert "GitHub CLI" in readme
    assert "`gh release create`" in readme
    assert "`wingetcreate`" in readme
    assert "Build binary archives" in readme
    assert "Build package wrappers" in readme
    assert "Publish package registries" in readme
    assert "release.py github-release" in readme
    assert readme.index("release.py github-release") > readme.index("### 3. Publish package registries")


def test_root_readme_is_user_facing_and_points_release_docs_to_packaging():
    readme = (REPO_ROOT / "README.md").read_text(encoding="utf-8")

    assert "See [packaging/README.md](packaging/README.md)" in readme
    assert "## Installation" in readme
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
        ("homebrew", True, True),
        ("winget", True),
    ]


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
