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

"""Generate Homebrew Formula files for public CLI install names."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


TARGET_ARCHIVES = {
    "aarch64-apple-darwin": "ve-storage-uni-cli-aarch64-apple-darwin.tar.gz",
    "x86_64-apple-darwin": "ve-storage-uni-cli-x86_64-apple-darwin.tar.gz",
}

TARGETS = {
    "macos_arm": TARGET_ARCHIVES["aarch64-apple-darwin"],
    "macos_intel": TARGET_ARCHIVES["x86_64-apple-darwin"],
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True, help="Formula version")
    parser.add_argument(
        "--checksums",
        default=Path("dist/SHA256SUMS"),
        type=Path,
        help="SHA256SUMS file from release packaging",
    )
    parser.add_argument(
        "--config",
        default=Path("packaging/homebrew/formulae.json"),
        type=Path,
        help="Homebrew formula definition file",
    )
    parser.add_argument(
        "--out-dir",
        default=Path("dist/homebrew/Formula"),
        type=Path,
        help="Output directory for Formula files",
    )
    parser.add_argument(
        "--target",
        choices=sorted(TARGET_ARCHIVES),
        help="Generate formulae from a single macOS target archive instead of both macOS targets",
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


def url(repo_url: str, version: str, archive: str) -> str:
    return f"{repo_url}/releases/download/v{version}/{archive}"


def require_checksums(sums: dict[str, str], archives: dict[str, str]) -> dict[str, str]:
    missing = [archive for archive in archives.values() if archive not in sums]
    if missing:
        raise SystemExit("missing release checksums: " + ", ".join(sorted(missing)))
    return {key: sums[archive] for key, archive in archives.items()}


def require_checksum(sums: dict[str, str], archive: str) -> str:
    if archive not in sums:
        raise SystemExit(f"missing release checksum: {archive}")
    return sums[archive]


def install_lines(commands: list[str]) -> str:
    return "\n".join(f'    bin.install "bin/{command}"' for command in commands)


def formula_text(
    formula: dict[str, object],
    version: str,
    repo_url: str,
    sums: dict[str, str],
    target: str | None = None,
) -> str:
    commands = list(formula["commands"])
    primary_command = commands[0]
    if target is not None:
        archive = TARGET_ARCHIVES[target]
        return f'''class {formula["class"]} < Formula
  desc "{formula["description"]}"
  homepage "{repo_url}"
  version "{version}"
  license "Apache-2.0"

  url "{url(repo_url, version, archive)}"
  sha256 "{require_checksum(sums, archive)}"

  def install
{install_lines(commands)}
  end

  test do
    system "#{{bin}}/{primary_command}", "--version"
  end
end
'''

    required = require_checksums(sums, TARGETS)
    return f'''class {formula["class"]} < Formula
  desc "{formula["description"]}"
  homepage "{repo_url}"
  version "{version}"
  license "Apache-2.0"

  on_macos do
    if Hardware::CPU.arm?
      url "{url(repo_url, version, TARGETS["macos_arm"])}"
      sha256 "{required["macos_arm"]}"
    else
      url "{url(repo_url, version, TARGETS["macos_intel"])}"
      sha256 "{required["macos_intel"]}"
    end
  end

  def install
{install_lines(commands)}
  end

  test do
    system "#{{bin}}/{primary_command}", "--version"
  end
end
'''


def main() -> None:
    args = parse_args()
    config = json.loads(args.config.read_text(encoding="utf-8"))
    sums = checksums(args.checksums)
    args.out_dir.mkdir(parents=True, exist_ok=True)
    for formula in config["formulae"]:
        path = args.out_dir / f"{formula['name']}.rb"
        path.write_text(
            formula_text(formula, args.version, config["releaseRepo"], sums, target=args.target),
            encoding="utf-8",
        )


if __name__ == "__main__":
    main()
