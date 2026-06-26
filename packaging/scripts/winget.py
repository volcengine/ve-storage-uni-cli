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

"""Generate WinGet manifests for public CLI install names."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


WINDOWS_ARCHIVES = {
    "x64": "ve-storage-uni-cli-x86_64-pc-windows-msvc.zip",
    "arm64": "ve-storage-uni-cli-aarch64-pc-windows-msvc.zip",
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True, help="Package version")
    parser.add_argument(
        "--checksums",
        default=Path("dist/SHA256SUMS"),
        type=Path,
        help="SHA256SUMS file from release packaging",
    )
    parser.add_argument(
        "--config",
        default=Path("packaging/winget/packages.json"),
        type=Path,
        help="WinGet package definition file",
    )
    parser.add_argument(
        "--out-dir",
        default=Path("dist/winget"),
        type=Path,
        help="Output directory for WinGet manifests",
    )
    return parser.parse_args()


def checksums(path: Path) -> dict[str, str]:
    values: dict[str, str] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        digest, filename = line.split(maxsplit=1)
        values[filename.strip()] = digest
    return values


def release_url(repo_url: str, version: str, archive: str) -> str:
    return f"{repo_url}/releases/download/v{version}/{archive}"


def require_checksum(sums: dict[str, str], archive: str) -> str:
    if archive not in sums:
        raise SystemExit(f"missing release checksum: {archive}")
    return sums[archive]


def nested_files(commands: list[str], indent: str = "  ") -> str:
    lines: list[str] = []
    for command in commands:
        lines.append(f"{indent}- RelativeFilePath: bin/{command}.exe")
        lines.append(f"{indent}  PortableCommandAlias: {command}")
    return "\n".join(lines)


def installer_manifest(
    package: dict[str, object],
    config: dict[str, object],
    version: str,
    sums: dict[str, str],
) -> str:
    installers: list[str] = []
    for architecture, archive in WINDOWS_ARCHIVES.items():
        installers.append(
            f"""- Architecture: {architecture}
  InstallerUrl: {release_url(str(config["releaseRepo"]), version, archive)}
  InstallerSha256: {require_checksum(sums, archive)}
  NestedInstallerFiles:
{nested_files(list(package["commands"]), "  ")}"""
        )
    return f"""PackageIdentifier: {package["identifier"]}
PackageVersion: {version}
InstallerType: zip
NestedInstallerType: portable
Installers:
{chr(10).join(installers)}
ManifestType: installer
ManifestVersion: {config["manifestVersion"]}
"""


def locale_manifest(package: dict[str, object], config: dict[str, object], version: str) -> str:
    return f"""PackageIdentifier: {package["identifier"]}
PackageVersion: {version}
PackageLocale: en-US
Publisher: {config["publisher"]}
PackageName: {package["name"]}
License: Apache-2.0
ShortDescription: {package["description"]}
Moniker: {package["moniker"]}
ManifestType: defaultLocale
ManifestVersion: {config["manifestVersion"]}
"""


def version_manifest(package: dict[str, object], config: dict[str, object], version: str) -> str:
    return f"""PackageIdentifier: {package["identifier"]}
PackageVersion: {version}
DefaultLocale: en-US
ManifestType: version
ManifestVersion: {config["manifestVersion"]}
"""


def package_dir(out_dir: Path, identifier: str, version: str) -> Path:
    parts = identifier.split(".")
    return out_dir.joinpath(*parts, version)


def main() -> None:
    args = parse_args()
    config = json.loads(args.config.read_text(encoding="utf-8"))
    sums = checksums(args.checksums)
    for package in config["packages"]:
        target_dir = package_dir(args.out_dir, str(package["identifier"]), args.version)
        target_dir.mkdir(parents=True, exist_ok=True)
        base = str(package["identifier"])
        (target_dir / f"{base}.installer.yaml").write_text(
            installer_manifest(package, config, args.version, sums),
            encoding="utf-8",
        )
        (target_dir / f"{base}.locale.en-US.yaml").write_text(
            locale_manifest(package, config, args.version),
            encoding="utf-8",
        )
        (target_dir / f"{base}.yaml").write_text(
            version_manifest(package, config, args.version),
            encoding="utf-8",
        )


if __name__ == "__main__":
    main()
