#!/usr/bin/env python3
# Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
# http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

"""Orchestrate release packaging for all public install channels."""

from __future__ import annotations

import argparse
import glob
import shlex
import shutil
import subprocess
import sys
import time
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT_DIR = Path(__file__).resolve().parent
PUBLIC_PACKAGES = ("ve-tos-cli", "tos-cli", "ve-adrive-cli")
ENTRY_CRATES = PUBLIC_PACKAGES
CARGO_PUBLISH_RETRY_DELAYS = (15, 30, 60, 120)
CARGO_PUBLISH_STEPS = (
    ("tos-core", ("cargo", "publish", "-p", "tos-core")),
    ("ve-tos-cli-core", ("cargo", "publish", "-p", "ve-tos-cli-core")),
    ("tos-cli-core", ("cargo", "publish", "-p", "tos-cli-core")),
    ("ve-adrive-cli-core", ("cargo", "publish", "-p", "ve-adrive-cli-core")),
    ("ve-storage-uni-cli", ("cargo", "publish", "-p", "ve-storage-uni-cli")),
    (
        "ve-tos-cli",
        ("cargo", "publish", "--manifest-path", "packaging/cargo/ve-tos-cli/Cargo.toml"),
    ),
    (
        "tos-cli",
        ("cargo", "publish", "--manifest-path", "packaging/cargo/tos-cli/Cargo.toml"),
    ),
    (
        "ve-adrive-cli",
        ("cargo", "publish", "--manifest-path", "packaging/cargo/ve-adrive-cli/Cargo.toml"),
    ),
)


def run_command(command: list[str] | tuple[str, ...], execute: bool = True) -> None:
    printable = " ".join(shlex.quote(part) for part in command)
    print(f"$ {printable}")
    if execute:
        subprocess.run(list(command), cwd=ROOT, check=True)


def command_succeeds(command: list[str] | tuple[str, ...]) -> bool:
    printable = " ".join(shlex.quote(part) for part in command)
    print(f"$ {printable}")
    return (
        subprocess.run(
            list(command),
            cwd=ROOT,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        ).returncode
        == 0
    )


def should_retry_cargo_publish(command: list[str] | tuple[str, ...]) -> bool:
    return (
        len(command) >= 2
        and tuple(command[:2]) == ("cargo", "publish")
        and "--dry-run" not in command
    )


def run_cargo_publish_command(command: list[str], execute: bool) -> None:
    if not execute or not should_retry_cargo_publish(command):
        run_command(command, execute=execute)
        return

    for attempt, retry_delay in enumerate((*CARGO_PUBLISH_RETRY_DELAYS, None), start=1):
        try:
            run_command(command, execute=True)
            return
        except subprocess.CalledProcessError:
            if retry_delay is None:
                raise
            # [Review Fix #CargoIndexRetry] Dependent first-time crates can
            # become visible in the crates.io index seconds after upload.
            print(
                f"# cargo publish failed on attempt {attempt}; "
                f"retrying in {retry_delay}s for registry index propagation"
            )
            time.sleep(retry_delay)


def script_command(script_name: str, args: list[str]) -> list[str]:
    return [sys.executable, str(SCRIPT_DIR / script_name), *args]


def release_dir(base_target_dir: Path, target: str) -> Path:
    return base_target_dir / target / "release"


def entry_release_dir(entry_name: str, target: str) -> Path:
    return ROOT / "packaging" / "cargo" / entry_name / "target" / target / "release"


def binary_dirs_for_target(target: str, root_target_dir: Path) -> list[Path]:
    return [entry_release_dir(entry_name, target) for entry_name in ENTRY_CRATES]


def build_release_binaries(target: str, skip_build: bool) -> None:
    if skip_build:
        return

    for entry_name in ENTRY_CRATES:
        manifest = f"packaging/cargo/{entry_name}/Cargo.toml"
        run_command(
            ["cargo", "build", "--release", "--target", target, "--manifest-path", manifest]
        )


def run_archives(args: argparse.Namespace) -> None:
    for target in args.target:
        build_release_binaries(target, args.skip_build)
        archive_args = [
            "--target",
            target,
            "--target-dir",
            str(args.target_dir),
            "--out-dir",
            str(args.out_dir),
            "--metadata",
            str(args.metadata),
        ]
        for binary_dir in binary_dirs_for_target(target, args.target_dir):
            archive_args.extend(["--binary-dir", str(binary_dir)])
        run_command(script_command("archives.py", archive_args))


def cargo_command(step: tuple[str, ...], args: argparse.Namespace) -> list[str]:
    command = list(step)
    if args.dry_run:
        command.append("--dry-run")
    if args.allow_dirty:
        command.append("--allow-dirty")
    return command


def run_cargo(args: argparse.Namespace) -> None:
    for package_name, step in CARGO_PUBLISH_STEPS:
        print(f"# cargo package: {package_name}")
        run_cargo_publish_command(cargo_command(step, args), execute=args.execute)


def run_npm(args: argparse.Namespace) -> None:
    run_command(
        script_command("npm.py", ["--version", args.version, "--out-dir", str(args.out_dir)])
    )
    for package_name in PUBLIC_PACKAGES:
        command = [
            "npm",
            "publish",
            str(args.out_dir / package_name),
            "--access",
            args.access,
            "--tag",
            args.tag,
        ]
        run_command(command, execute=args.execute_publish)


def pip_binary_dirs(args: argparse.Namespace) -> list[Path]:
    if args.binary_dir:
        return list(args.binary_dir)
    target = getattr(args, "target", None)
    if target:
        return binary_dirs_for_target(target, Path("target"))
    raise SystemExit("pip requires --target or at least one --binary-dir")


def run_pip(args: argparse.Namespace) -> None:
    package_args = ["--version", args.version, "--out-dir", str(args.out_dir)]
    for binary_dir in pip_binary_dirs(args):
        package_args.extend(["--binary-dir", str(binary_dir)])
    run_command(script_command("pip.py", package_args))

    if args.build_wheel:
        wheel_dir = args.out_dir / "wheels"
        for package_name in PUBLIC_PACKAGES:
            run_command(
                [
                    sys.executable,
                    "-m",
                    "build",
                    "--wheel",
                    "--outdir",
                    str(wheel_dir),
                    str(args.out_dir / package_name),
                ]
            )

    wheel_files = sorted(glob.glob(str(args.out_dir / "wheels" / "*.whl")))
    if args.upload:
        if not wheel_files:
            raise SystemExit("no wheels found; run with --build-wheel before --upload")
        run_command([sys.executable, "-m", "twine", "upload", *wheel_files])
    else:
        run_command([sys.executable, "-m", "twine", "upload", str(args.out_dir / "wheels" / "*.whl")], False)


def run_homebrew(args: argparse.Namespace) -> None:
    homebrew_args = [
        "--version",
        args.version,
        "--checksums",
        str(args.checksums),
        "--out-dir",
        str(args.out_dir),
    ]
    homebrew_target = getattr(args, "homebrew_target", None)
    if homebrew_target:
        homebrew_args.extend(["--target", homebrew_target])
    run_command(
        script_command(
            "homebrew.py",
            homebrew_args,
        )
    )
    if args.push and not args.commit:
        raise SystemExit("--push requires --commit so copied Homebrew formulae are included")
    if args.tap_dir is None and (args.commit or args.push):
        raise SystemExit("--tap-dir is required when committing or pushing Homebrew formulae")
    if args.tap_dir is None:
        print("# copy dist/homebrew/Formula/*.rb into the homebrew tap, then commit and push")
        return

    formula_dir = args.tap_dir / "Formula"
    formula_dir.mkdir(parents=True, exist_ok=True)
    for formula_path in sorted(args.out_dir.glob("*.rb")):
        shutil.copy2(formula_path, formula_dir / formula_path.name)
    run_command(["git", "-C", str(args.tap_dir), "status", "--short"], execute=True)
    if args.commit or args.push:
        # [Review Fix #25] The post-GitHub-release flow must publish the tap
        # without a manual copy/commit/push step.
        run_command(["git", "-C", str(args.tap_dir), "add", "Formula"], execute=True)
    if args.commit:
        run_command(
            [
                "git",
                "-C",
                str(args.tap_dir),
                "commit",
                "-m",
                args.commit_message or default_release_commit_message(args.version),
            ],
            execute=True,
        )
    if args.push:
        run_command(["git", "-C", str(args.tap_dir), "push"], execute=True)


def run_winget(args: argparse.Namespace) -> None:
    run_command(
        script_command(
            "winget.py",
            [
                "--version",
                args.version,
                "--checksums",
                str(args.checksums),
                "--out-dir",
                str(args.out_dir),
            ],
        )
    )
    run_command(["wingetcreate", "submit", str(args.out_dir)], execute=args.submit)


def github_release_tag(version: str) -> str:
    return version if version.startswith("v") else f"v{version}"


def is_release_archive(path: Path) -> bool:
    return path.name.endswith(".tar.gz") or path.suffix == ".zip"


def github_release_assets(out_dir: Path) -> list[Path]:
    required_assets = [out_dir / "SHA256SUMS", out_dir / "install.sh"]
    archive_assets = sorted(
        path
        for path in out_dir.glob("ve-storage-uni-cli-*")
        if path.is_file() and is_release_archive(path)
    )
    assets = [*required_assets, *archive_assets]
    missing = [str(path) for path in assets if not path.is_file()]
    if missing:
        raise SystemExit(f"missing GitHub Release assets: {', '.join(missing)}")
    if not archive_assets:
        raise SystemExit(f"no release archives found in {out_dir}")
    return assets


def github_release_create_command(args: argparse.Namespace, tag: str, assets: list[Path]) -> list[str]:
    command = [
        "gh",
        "release",
        "create",
        tag,
        *(str(asset) for asset in assets),
        "--repo",
        args.repo,
        "--title",
        args.title or tag,
    ]
    if args.notes_file:
        command.extend(["--notes-file", str(args.notes_file)])
    else:
        command.extend(["--notes", args.notes])
    if args.draft:
        command.append("--draft")
    if args.prerelease:
        command.append("--prerelease")
    return command


def github_release_upload_command(args: argparse.Namespace, tag: str, assets: list[Path]) -> list[str]:
    command = [
        "gh",
        "release",
        "upload",
        tag,
        *(str(asset) for asset in assets),
        "--repo",
        args.repo,
    ]
    if args.clobber:
        command.append("--clobber")
    return command


def github_release_exists(tag: str, repo: str) -> bool:
    return command_succeeds(["gh", "release", "view", tag, "--repo", repo])


def run_github_release(args: argparse.Namespace) -> None:
    tag = github_release_tag(args.version)
    assets = github_release_assets(args.out_dir)
    create_command = github_release_create_command(args, tag, assets)
    upload_command = github_release_upload_command(args, tag, assets)

    if args.mode == "create":
        run_command(create_command, execute=args.execute)
        return
    if args.mode == "upload":
        run_command(upload_command, execute=args.execute)
        return

    if not args.execute:
        print("# github release auto mode: create if missing, otherwise upload")
        run_command(create_command, execute=False)
        run_command(upload_command, execute=False)
        return

    if github_release_exists(tag, args.repo):
        run_command(upload_command, execute=True)
    else:
        run_command(create_command, execute=True)


def with_args(args: argparse.Namespace, **overrides: object) -> argparse.Namespace:
    values = vars(args).copy()
    values.update(overrides)
    return argparse.Namespace(**values)


def checksums_for_all(args: argparse.Namespace) -> Path:
    default_checksums = Path("dist/SHA256SUMS")
    if args.checksums == default_checksums and args.out_dir != Path("dist"):
        return args.out_dir / "SHA256SUMS"
    return args.checksums


def default_release_commit_message(version: str) -> str:
    return f"Release Volcengine storage CLIs v{version}"


def run_all(args: argparse.Namespace) -> None:
    if args.publish and args.tap_dir is None:
        raise SystemExit("--publish requires --tap-dir so Homebrew can be published")
    execute_cargo = args.execute_cargo or args.publish
    execute_github_release = args.execute_github_release or args.publish
    publish_github_release = args.github_release or args.publish
    execute_npm_publish = args.execute_publish or args.publish
    upload_pip = args.upload or args.publish
    build_pip_wheel = args.build_wheel or args.publish
    submit_winget = args.submit or args.publish
    commit_homebrew = args.homebrew_commit or args.publish
    push_homebrew = args.homebrew_push or args.publish

    run_archives(args)
    release_checksums = checksums_for_all(args)
    if publish_github_release:
        run_github_release(
            with_args(
                args,
                out_dir=args.out_dir,
                repo=args.github_repo,
                title=args.github_release_title,
                notes=args.github_release_notes,
                notes_file=args.github_release_notes_file,
                draft=args.github_release_draft,
                prerelease=args.github_release_prerelease,
                clobber=args.github_release_clobber,
                mode=args.github_release_mode,
                execute=execute_github_release,
            )
        )
    run_cargo(
        with_args(
            args,
            execute=execute_cargo,
            dry_run=args.cargo_dry_run,
            allow_dirty=args.allow_dirty,
        )
    )
    run_npm(with_args(args, out_dir=Path("dist/npm"), execute_publish=execute_npm_publish))
    pip_target = args.pip_target or args.target[0]
    run_pip(
        with_args(
            args,
            binary_dir=binary_dirs_for_target(pip_target, args.target_dir),
            out_dir=Path("dist/pip"),
            build_wheel=build_pip_wheel,
            upload=upload_pip,
        )
    )
    run_homebrew(
        with_args(
            args,
            checksums=release_checksums,
            out_dir=Path("dist/homebrew/Formula"),
            commit=commit_homebrew,
            push=push_homebrew,
            commit_message=args.homebrew_commit_message,
        )
    )
    run_winget(
        with_args(
            args,
            checksums=release_checksums,
            out_dir=Path("dist/winget"),
            submit=submit_winget,
        )
    )


def add_common_version(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--version", required=True, help="Release version without leading v")


def add_archive_options(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--target", action="append", required=True, help="Rust target triple")
    parser.add_argument("--skip-build", action="store_true", help="Use already-built binaries")
    parser.add_argument("--target-dir", default=Path("target"), type=Path, help="Root target dir")
    parser.add_argument("--out-dir", default=Path("dist"), type=Path, help="Release archive dir")
    parser.add_argument(
        "--metadata",
        default=Path("packaging/release/platforms.json"),
        type=Path,
        help="Release platform metadata JSON",
    )


def add_checksum_options(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--checksums", default=Path("dist/SHA256SUMS"), type=Path)


def add_github_release_options(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--out-dir", default=Path("dist"), type=Path, help="Release asset directory")
    parser.add_argument("--repo", default="volcengine/ve-storage-uni-cli", help="GitHub repo")
    parser.add_argument(
        "--mode",
        choices=("auto", "create", "upload"),
        default="auto",
        help="Create release, upload to existing release, or auto-detect",
    )
    parser.add_argument("--title", help="GitHub Release title; defaults to the tag")
    parser.add_argument("--notes", default="", help="Inline GitHub Release notes")
    parser.add_argument("--notes-file", type=Path, help="GitHub Release notes file")
    parser.add_argument("--draft", action="store_true", help="Create the release as a draft")
    parser.add_argument("--prerelease", action="store_true", help="Mark the release as prerelease")
    parser.add_argument(
        "--clobber",
        action="store_true",
        help="Overwrite existing assets when uploading to an existing release",
    )
    parser.add_argument("--execute", action="store_true", help="Run gh commands")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="channel", required=True)

    cargo = subparsers.add_parser("cargo", help="Print or run cargo publish steps")
    cargo.add_argument("--execute", action="store_true", help="Run cargo publish commands")
    cargo.add_argument("--dry-run", action="store_true", help="Pass --dry-run to cargo publish")
    cargo.add_argument("--allow-dirty", action="store_true", help="Pass --allow-dirty")
    cargo.set_defaults(func=run_cargo)

    archives = subparsers.add_parser("archives", help="Build binaries and release archives")
    add_archive_options(archives)
    archives.set_defaults(func=run_archives)

    npm = subparsers.add_parser("npm", help="Generate npm packages")
    add_common_version(npm)
    npm.add_argument("--out-dir", default=Path("dist/npm"), type=Path)
    npm.add_argument("--access", default="public")
    npm.add_argument("--tag", default="latest")
    npm.add_argument("--execute-publish", action="store_true", help="Run npm publish")
    npm.set_defaults(func=run_npm)

    pip = subparsers.add_parser("pip", help="Generate PyPI package trees")
    add_common_version(pip)
    pip_binary_input = pip.add_mutually_exclusive_group(required=True)
    pip_binary_input.add_argument("--binary-dir", action="append", type=Path)
    pip_binary_input.add_argument(
        "--target",
        help="Rust target triple whose wrapper binaries seed wheels",
    )
    pip.add_argument("--out-dir", default=Path("dist/pip"), type=Path)
    pip.add_argument("--build-wheel", action="store_true")
    pip.add_argument("--upload", action="store_true", help="Upload wheels with twine")
    pip.set_defaults(func=run_pip)

    homebrew = subparsers.add_parser("homebrew", help="Generate Homebrew Formula files")
    add_common_version(homebrew)
    add_checksum_options(homebrew)
    homebrew.add_argument(
        "--target",
        dest="homebrew_target",
        help="Generate formulae from one target archive",
    )
    homebrew.add_argument("--out-dir", default=Path("dist/homebrew/Formula"), type=Path)
    homebrew.add_argument("--tap-dir", type=Path, help="Optional local homebrew tap checkout")
    homebrew.add_argument("--commit", action="store_true", help="Commit copied formulae in --tap-dir")
    homebrew.add_argument("--push", action="store_true", help="Push the Homebrew tap after commit")
    homebrew.add_argument("--commit-message", help="Homebrew tap commit message")
    homebrew.set_defaults(func=run_homebrew)

    winget = subparsers.add_parser("winget", help="Generate WinGet manifests")
    add_common_version(winget)
    add_checksum_options(winget)
    winget.add_argument("--out-dir", default=Path("dist/winget"), type=Path)
    winget.add_argument("--submit", action="store_true", help="Run wingetcreate submit")
    winget.set_defaults(func=run_winget)

    github_release = subparsers.add_parser(
        "github-release",
        help="Create or upload GitHub Release assets",
    )
    add_common_version(github_release)
    add_github_release_options(github_release)
    github_release.set_defaults(func=run_github_release)

    all_channels = subparsers.add_parser("all", help="Generate all release artifacts")
    add_common_version(all_channels)
    add_archive_options(all_channels)
    add_checksum_options(all_channels)
    all_channels.add_argument("--pip-target", help="Target whose binaries seed PyPI wheels")
    all_channels.add_argument("--access", default="public")
    all_channels.add_argument("--tag", default="latest")
    all_channels.add_argument("--execute-publish", action="store_true", help="Run npm publish")
    all_channels.add_argument("--build-wheel", action="store_true")
    all_channels.add_argument("--upload", action="store_true", help="Upload wheels with twine")
    all_channels.add_argument("--tap-dir", type=Path, help="Optional local homebrew tap checkout")
    all_channels.add_argument("--homebrew-target", help="Generate Homebrew formulae from one target archive")
    all_channels.add_argument(
        "--homebrew-commit",
        action="store_true",
        help="Commit copied formulae in --tap-dir",
    )
    all_channels.add_argument("--homebrew-push", action="store_true", help="Push the Homebrew tap")
    all_channels.add_argument("--homebrew-commit-message", help="Homebrew tap commit message")
    all_channels.add_argument("--submit", action="store_true", help="Run wingetcreate submit")
    all_channels.add_argument("--execute-cargo", action="store_true", help="Run cargo publish")
    all_channels.add_argument("--cargo-dry-run", action="store_true", help="Pass --dry-run to cargo publish")
    all_channels.add_argument("--allow-dirty", action="store_true", help="Pass --allow-dirty to cargo publish")
    all_channels.add_argument(
        "--github-release",
        action="store_true",
        help="Create or upload GitHub Release assets after building archives",
    )
    all_channels.add_argument(
        "--execute-github-release",
        action="store_true",
        help="Run gh release commands",
    )
    all_channels.add_argument("--github-repo", default="volcengine/ve-storage-uni-cli")
    all_channels.add_argument(
        "--github-release-mode",
        choices=("auto", "create", "upload"),
        default="auto",
    )
    all_channels.add_argument("--github-release-title")
    all_channels.add_argument("--github-release-notes", default="")
    all_channels.add_argument("--github-release-notes-file", type=Path)
    all_channels.add_argument("--github-release-draft", action="store_true")
    all_channels.add_argument("--github-release-prerelease", action="store_true")
    all_channels.add_argument("--github-release-clobber", action="store_true")
    all_channels.add_argument(
        "--publish",
        action="store_true",
        help="Publish Cargo, GitHub Release assets, npm, PyPI, Homebrew tap, and WinGet",
    )
    all_channels.set_defaults(
        func=run_all,
        out_dir=Path("dist"),
    )

    return parser


def main() -> None:
    args = build_parser().parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
