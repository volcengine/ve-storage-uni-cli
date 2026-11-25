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

"""Generate self-contained npm packages, with same-version GitHub recovery."""

from __future__ import annotations

import argparse
import json
import re
import shutil
import tempfile
from pathlib import Path

from npm_bundle import bundle_binaries, validate_archives


TEMPLATE_DIR = Path(__file__).resolve().parents[1] / "npm"


PLATFORM_JS = r"""'use strict';

function detectTarget() {
  const os = process.platform;
  const arch = process.arch;
  if (os === 'darwin' && arch === 'x64') return 'x86_64-apple-darwin';
  if (os === 'darwin' && arch === 'arm64') return 'aarch64-apple-darwin';
  if (os === 'win32' && arch === 'x64') return 'x86_64-pc-windows-msvc';
  if (os === 'linux' && isMusl()) throw new Error(`unsupported platform: linux/${arch} musl`);
  if (os === 'linux' && arch === 'x64') {
    return 'x86_64-unknown-linux-gnu.2.17';
  }
  if (os === 'linux' && arch === 'arm64') return 'aarch64-unknown-linux-gnu.2.17';
  throw new Error(`unsupported platform: ${os}/${arch}`);
}

function isMusl() {
  const report = typeof process.report?.getReport === 'function'
    ? process.report.getReport()
    : undefined;
  return !report?.header?.glibcVersionRuntime;
}

function archiveName(target) {
  const suffix = target.includes('windows') ? '.zip' : '.tar.gz';
  return `ve-storage-uni-cli-${target}${suffix}`;
}

function executableName(commandName) {
  return process.platform === 'win32' ? `${commandName}.exe` : commandName;
}

module.exports = { archiveName, detectTarget, executableName };
"""


BIN_JS = r"""#!/usr/bin/env node
'use strict';

const fs = require('fs');
const path = require('path');
const { spawnSync } = require('child_process');
const { detectTarget, executableName } = require('../lib/platform');

const commandName = '__COMMAND__';
const target = detectTarget();
const binaryPath = path.join(__dirname, '..', 'vendor', target, 'bin', executableName(commandName));

if (!fs.existsSync(binaryPath)) {
  console.error(`${commandName} binary is missing for ${target}; reinstall the npm package`);
  process.exit(1);
}

const result = spawnSync(binaryPath, process.argv.slice(2), { stdio: 'inherit' });
if (result.error) {
  console.error(result.error.message);
  process.exit(1);
}
process.exit(result.status === null ? 1 : result.status);
"""


def parse_args() -> argparse.Namespace:
    """Parse package selection and local archive inputs; reject invalid CLI syntax."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True, help="Unpublished npm package version")
    parser.add_argument("--package", help="Generate only the named npm package")
    parser.add_argument("--archive-dir", default=Path("dist"), type=Path,
                        help="Release archive directory (default: dist); missing checksums are automatic")
    parser.add_argument("--config", default=Path("packaging/npm/packages.json"), type=Path,
                        help="npm package definition file")
    parser.add_argument("--out-dir", default=Path("dist/npm"), type=Path,
                        help="Output directory for generated npm packages")
    return parser.parse_args()


def write_text(path: Path, content: str, mode: int | None = None) -> None:
    """Write a UTF-8 package asset with optional mode; propagate I/O failures."""
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")
    if mode is not None:
        path.chmod(mode)


def write_package_assets(package_dir: Path, package: dict[str, object],
                         version: str, repo_url: str) -> None:
    """Write launchers and metadata after payload staging; propagate I/O errors."""
    commands = list(package["commands"])
    package_json = {
        "name": package["name"], "version": version,
        "description": package["description"], "license": "Apache-2.0",
        "repository": {"type": "git", "url": repo_url},
        "bin": {command: f"bin/{command}.js" for command in commands},
        "scripts": {"postinstall": "node scripts/install.js"},
        "engines": {"node": ">=18"},
        "os": ["darwin", "linux", "win32"], "cpu": ["x64", "arm64"],
        "files": ["bin", "lib", "scripts", "vendor"],
    }
    write_text(package_dir / "lib/platform.js", PLATFORM_JS)
    write_text(package_dir / "lib/download.js", (TEMPLATE_DIR / "download.js").read_text(encoding="utf-8"))
    installer = (TEMPLATE_DIR / "install.js").read_text(encoding="utf-8")
    write_text(package_dir / "scripts/install.js", installer.replace("__RELEASE_REPO__", repo_url))
    for command in commands:
        write_text(package_dir / f"bin/{command}.js",
                   BIN_JS.replace("__COMMAND__", command), mode=0o755)
    write_text(package_dir / "package.json", json.dumps(package_json, indent=2) + "\n")


def generate_package(args: argparse.Namespace, package: dict[str, object],
                     repo_url: str, checksums: dict[str, str]) -> None:
    """Stage a complete package before replacing prior output; fail on bad input.

    Args supplies the version, output and archive directories. Only this package's
    commands are copied. Invalid members and filesystem errors abort generation.
    """
    package_name = str(package["name"])
    args.out_dir.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=f".{package_name}-", dir=args.out_dir) as staging:
        staged_package = Path(staging) / package_name
        targets = bundle_binaries(staged_package, list(package["commands"]),
                                  args.archive_dir, checksums)
        manifest = {"version": args.version, "targets": targets}
        write_text(staged_package / "lib/binaries.json", json.dumps(manifest, indent=2) + "\n")
        write_package_assets(staged_package, package, args.version, repo_url)
        package_dir = args.out_dir / package_name
        if package_dir.exists():
            shutil.rmtree(package_dir)
        staged_package.rename(package_dir)
    print(f"generated {package_name}@{args.version} with {len(targets)} bundled platforms")


def main() -> None:
    """Validate release inputs and emit selected npm packages; exit nonzero on error."""
    args = parse_args()
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?", args.version):
        raise SystemExit("--version must be an npm version such as 1.0.5, without a v prefix")
    config = json.loads(args.config.read_text(encoding="utf-8"))
    packages = [package for package in config["packages"]
                if args.package is None or package["name"] == args.package]
    if not packages:
        raise SystemExit(f"unknown npm package: {args.package}")
    try:
        checksums = validate_archives(args.archive_dir)
        for package in packages:
            generate_package(args, package, config["releaseRepo"], checksums)
    except (OSError, ValueError) as error:
        raise SystemExit(f"npm packaging failed: {error}") from error


if __name__ == "__main__":
    main()
