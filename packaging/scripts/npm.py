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

"""Generate npm thin packages for the public CLI install names."""

from __future__ import annotations

import argparse
import json
import shutil
from pathlib import Path


PLATFORM_JS = r"""'use strict';

function detectTarget() {
  const os = process.platform;
  const arch = process.arch;
  if (os === 'darwin' && arch === 'x64') return 'x86_64-apple-darwin';
  if (os === 'darwin' && arch === 'arm64') return 'aarch64-apple-darwin';
  if (os === 'win32' && arch === 'x64') return 'x86_64-pc-windows-msvc';
  if (os === 'linux' && arch === 'x64') {
    if (isMusl()) throw new Error('unsupported platform: linux/x64 musl');
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


INSTALL_JS = r"""'use strict';

const crypto = require('crypto');
const fs = require('fs');
const https = require('https');
const path = require('path');
const { spawnSync } = require('child_process');
const { archiveName, detectTarget } = require('../lib/platform');

const commandName = '__PACKAGE__';
const packageVersion = '__VERSION__';
const repoUrl = process.env.VE_STORAGE_UNI_CLI_REPO_URL || '__RELEASE_REPO__';
const releaseVersion = process.env.VE_STORAGE_UNI_CLI_VERSION || `v${packageVersion}`;

function releaseBaseUrl() {
  if (releaseVersion === 'latest') return `${repoUrl}/releases/latest/download`;
  return `${repoUrl}/releases/download/${releaseVersion}`;
}

function download(url, dest) {
  return new Promise((resolve, reject) => {
    const request = https.get(url, (response) => {
      if ([301, 302, 303, 307, 308].includes(response.statusCode)) {
        response.resume();
        download(response.headers.location, dest).then(resolve, reject);
        return;
      }
      if (response.statusCode !== 200) {
        response.resume();
        reject(new Error(`download failed (${response.statusCode}): ${url}`));
        return;
      }
      const file = fs.createWriteStream(dest);
      response.pipe(file);
      file.on('finish', () => file.close(resolve));
      file.on('error', reject);
    });
    request.on('error', reject);
  });
}

function sha256(filePath) {
  const hash = crypto.createHash('sha256');
  const data = fs.readFileSync(filePath);
  hash.update(data);
  return hash.digest('hex');
}

function expectedChecksum(sumsText, archive) {
  const line = sumsText.split(/\r?\n/).find((entry) => entry.endsWith(` ${archive}`));
  if (!line) throw new Error(`SHA256SUMS does not contain ${archive}`);
  return line.trim().split(/\s+/)[0];
}

function extract(archivePath, destDir) {
  fs.rmSync(destDir, { recursive: true, force: true });
  fs.mkdirSync(destDir, { recursive: true });
  if (archivePath.endsWith('.tar.gz')) {
    const result = spawnSync('tar', ['-xzf', archivePath, '-C', destDir], { stdio: 'inherit' });
    if (result.status !== 0) throw new Error('tar extraction failed');
    return;
  }
  const escapedArchive = archivePath.replace(/'/g, "''");
  const escapedDest = destDir.replace(/'/g, "''");
  const result = spawnSync(
    'powershell.exe',
    ['-NoProfile', '-Command', `Expand-Archive -Force -Path '${escapedArchive}' -DestinationPath '${escapedDest}'`],
    { stdio: 'inherit' }
  );
  if (result.status !== 0) throw new Error('zip extraction failed');
}

async function main() {
  const target = detectTarget();
  const archive = archiveName(target);
  const packageRoot = path.resolve(__dirname, '..');
  const cacheDir = path.join(packageRoot, '.download');
  const vendorDir = path.join(packageRoot, 'vendor', target);
  fs.mkdirSync(cacheDir, { recursive: true });

  const baseUrl = releaseBaseUrl();
  const archivePath = path.join(cacheDir, archive);
  const sumsPath = path.join(cacheDir, 'SHA256SUMS');
  await download(`${baseUrl}/${archive}`, archivePath);
  await download(`${baseUrl}/SHA256SUMS`, sumsPath);

  const actual = sha256(archivePath);
  const expected = expectedChecksum(fs.readFileSync(sumsPath, 'utf8'), archive);
  if (actual !== expected) {
    throw new Error(`checksum mismatch for ${archive}`);
  }

  extract(archivePath, vendorDir);
  console.log(`installed ${commandName} for ${target}`);
}

main().catch((error) => {
  console.error(`[${commandName}] ${error.message}`);
  process.exit(1);
});
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
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True, help="Package version")
    parser.add_argument(
        "--config",
        default=Path("packaging/npm/packages.json"),
        type=Path,
        help="npm package definition file",
    )
    parser.add_argument(
        "--out-dir",
        default=Path("dist/npm"),
        type=Path,
        help="Output directory for generated npm packages",
    )
    return parser.parse_args()


def write_text(path: Path, content: str, mode: int | None = None) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")
    if mode is not None:
        path.chmod(mode)


def command_bins(commands: list[str]) -> dict[str, str]:
    return {command: f"bin/{command}.js" for command in commands}


def generate_package(out_dir: Path, package: dict[str, object], version: str, repo_url: str) -> None:
    package_name = str(package["name"])
    commands = list(package["commands"])
    package_dir = out_dir / package_name
    if package_dir.exists():
        shutil.rmtree(package_dir)
    package_dir.mkdir(parents=True)

    package_json = {
        "name": package_name,
        "version": version,
        "description": package["description"],
        "license": "Apache-2.0",
        "repository": {"type": "git", "url": repo_url},
        "bin": command_bins(commands),
        "scripts": {"postinstall": "node scripts/install.js"},
        "os": ["darwin", "linux", "win32"],
        "cpu": ["x64", "arm64"],
        "files": ["bin", "lib", "scripts", "vendor"],
    }
    write_text(
        package_dir / "package.json",
        json.dumps(package_json, indent=2, sort_keys=True) + "\n",
    )
    write_text(package_dir / "lib/platform.js", PLATFORM_JS)
    write_text(
        package_dir / "scripts/install.js",
        INSTALL_JS.replace("__PACKAGE__", package_name)
        .replace("__VERSION__", version)
        .replace("__RELEASE_REPO__", repo_url),
    )
    for command in commands:
        write_text(
            package_dir / "bin" / f"{command}.js",
            BIN_JS.replace("__COMMAND__", command),
            mode=0o755,
        )


def main() -> None:
    args = parse_args()
    config = json.loads(args.config.read_text(encoding="utf-8"))
    for package in config["packages"]:
        generate_package(args.out_dir, package, args.version, config["releaseRepo"])


if __name__ == "__main__":
    main()
