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

"""Package built CLI binaries into release archives."""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import tarfile
import zipfile
from pathlib import Path


COMMANDS = ("ve-tos-cli", "tos-cli", "ve-adrive-cli")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True, help="Rust target triple")
    parser.add_argument(
        "--target-dir",
        default="target",
        type=Path,
        help="Root Cargo target directory, default: target",
    )
    parser.add_argument(
        "--binary-dir",
        action="append",
        default=[],
        type=Path,
        help="Directory containing built CLI binaries; can be repeated",
    )
    parser.add_argument(
        "--out-dir",
        default="dist",
        type=Path,
        help="Directory for release archives, default: dist",
    )
    parser.add_argument(
        "--metadata",
        default=Path("packaging/release/platforms.json"),
        type=Path,
        help="Release platform metadata JSON",
    )
    return parser.parse_args()


def release_plan(metadata_path: Path, target: str) -> tuple[str, tuple[str, ...]]:
    metadata = json.loads(metadata_path.read_text(encoding="utf-8"))
    for target_entry in metadata["targets"]:
        if target_entry["triple"] == target:
            commands = tuple(metadata.get("commands", COMMANDS))
            return str(target_entry["archive"]), commands
    raise SystemExit(f"target {target!r} is not listed in {metadata_path}")


def binary_dir(target_dir: Path, target: str) -> Path:
    cross_dir = target_dir / target / "release"
    if cross_dir.exists():
        return cross_dir
    return target_dir / "release"


def find_binary(source_dirs: list[Path], command_name: str, is_windows: bool) -> Path:
    suffix = ".exe" if is_windows else ""
    for source_dir in source_dirs:
        candidate = source_dir / f"{command_name}{suffix}"
        if candidate.exists():
            return candidate

    searched = ", ".join(str(source_dir) for source_dir in source_dirs)
    raise SystemExit(f"missing built binary {command_name}{suffix}; searched: {searched}")


def stage_files(
    source_dirs: list[Path],
    stage_dir: Path,
    commands: tuple[str, ...],
    is_windows: bool,
) -> None:
    bin_dir = stage_dir / "bin"
    bin_dir.mkdir(parents=True, exist_ok=True)
    for command_name in commands:
        source = find_binary(source_dirs, command_name, is_windows)
        dest = bin_dir / source.name
        shutil.copy2(source, dest)
        dest.chmod(0o755)

    for doc_name in ("LICENSE", "README.md"):
        doc_path = Path(doc_name)
        if doc_path.exists():
            shutil.copy2(doc_path, stage_dir / doc_name)


def make_archive(stage_dir: Path, archive_path: Path) -> None:
    if archive_path.suffix == ".zip":
        with zipfile.ZipFile(archive_path, "w", zipfile.ZIP_DEFLATED) as zip_file:
            for path in sorted(stage_dir.rglob("*")):
                zip_file.write(path, path.relative_to(stage_dir))
        return

    if archive_path.name.endswith(".tar.gz"):
        with tarfile.open(archive_path, "w:gz") as tar_file:
            for path in sorted(stage_dir.rglob("*")):
                tar_file.add(path, arcname=path.relative_to(stage_dir))
        return

    raise SystemExit(f"unsupported archive format: {archive_path.name}")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file_obj:
        for chunk in iter(lambda: file_obj.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def copy_shell_installer(out_dir: Path) -> None:
    installer = Path("packaging/install/install.sh")
    if installer.exists():
        shutil.copy2(installer, out_dir / "install.sh")


def update_checksums(out_dir: Path) -> None:
    entries = []
    for archive in sorted(out_dir.iterdir()):
        if archive.name == "SHA256SUMS" or archive.is_dir():
            continue
        entries.append(f"{sha256(archive)}  {archive.name}")
    (out_dir / "SHA256SUMS").write_text("\n".join(entries) + "\n", encoding="utf-8")


def main() -> None:
    args = parse_args()
    archive_name, commands = release_plan(args.metadata, args.target)
    out_dir = args.out_dir
    out_dir.mkdir(parents=True, exist_ok=True)

    stage_dir = out_dir / f".stage-{args.target}"
    if stage_dir.exists():
        shutil.rmtree(stage_dir)
    stage_dir.mkdir(parents=True)

    try:
        source_dirs = args.binary_dir or [binary_dir(args.target_dir, args.target)]
        stage_files(
            source_dirs,
            stage_dir,
            commands,
            is_windows=args.target.endswith("windows-msvc"),
        )
        make_archive(stage_dir, out_dir / archive_name)
        copy_shell_installer(out_dir)
        update_checksums(out_dir)
    finally:
        # [Review Fix #3] Always remove staging files, including when archive
        # creation fails before checksum generation.
        shutil.rmtree(stage_dir, ignore_errors=True)


if __name__ == "__main__":
    main()
