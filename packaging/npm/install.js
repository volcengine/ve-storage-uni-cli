'use strict';

const crypto = require('crypto');
const fs = require('fs');
const path = require('path');
const { spawnSync } = require('child_process');
const { download } = require('../lib/download');
const { detectTarget, executableName } = require('../lib/platform');
const manifest = require('../lib/binaries.json');
const metadata = require('../package.json');

const packageRoot = path.resolve(__dirname, '..');

/** Hash a binary or archive before trusting it. */
async function sha256(filename) {
  const digest = crypto.createHash('sha256');
  for await (const chunk of fs.createReadStream(filename)) digest.update(chunk);
  return digest.digest('hex');
}

/** A missing file permits fallback; a present but invalid file fails closed. */
async function isBundledBinaryValid(filename, expected) {
  let status;
  try { status = fs.lstatSync(filename); } catch (error) {
    if (error.code === 'ENOENT') return false;
    throw error;
  }
  if (!status.isFile() || status.isSymbolicLink() || await sha256(filename) !== expected) {
    throw new Error(`checksum mismatch for bundled ${path.basename(filename)}`);
  }
  return true;
}

/** Keep fallback tied to the npm version and its packaged checksum manifest. */
function releaseBaseUrl() {
  const override = process.env.VE_STORAGE_UNI_CLI_VERSION;
  if (override && override !== manifest.version && override !== `v${manifest.version}`) {
    throw new Error('VE_STORAGE_UNI_CLI_VERSION must match the npm package version');
  }
  const repoUrl = process.env.VE_STORAGE_UNI_CLI_REPO_URL || '__RELEASE_REPO__';
  const parsed = new URL(repoUrl);
  if (parsed.protocol !== 'https:' || parsed.username || parsed.password || parsed.search || parsed.hash) {
    throw new Error('release repository must be an HTTPS URL without credentials, query or fragment');
  }
  return `${repoUrl.replace(/\/$/, '')}/releases/download/v${manifest.version}`;
}

/** Extract only the selected command to an explicit staging file. */
function extractBinary(archivePath, destination, command) {
  const member = `bin/${executableName(command)}`;
  let result;
  if (process.platform === 'win32') {
    const quote = (value) => `'${value.replace(/'/g, "''")}'`;
    const script = "$ErrorActionPreference='Stop'; Add-Type -AssemblyName System.IO.Compression.FileSystem; " +
      `$zip=[IO.Compression.ZipFile]::OpenRead(${quote(archivePath)}); try { ` +
      `$entries=@($zip.Entries | Where-Object { $_.FullName -eq ${quote(member)} }); ` +
      "if ($entries.Count -ne 1 -or $entries[0].Length -le 0 -or $entries[0].Length -gt 536870912) { throw 'invalid binary entry' }; " +
      `[IO.Compression.ZipFileExtensions]::ExtractToFile($entries[0], ${quote(destination)}); ` +
      '} finally { $zip.Dispose() }';
    result = spawnSync('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command', script],
      { encoding: 'utf8', timeout: 120000 });
  } else {
    const output = fs.openSync(destination, 'wx', 0o755);
    try {
      result = spawnSync('tar', ['-xOzf', archivePath, '--', member],
        { stdio: ['ignore', output, 'pipe'], timeout: 120000 });
    } finally { fs.closeSync(output); }
  }
  if (result.error || result.status !== 0) throw new Error('release binary extraction failed');
}

/** Download a pinned archive only when a packaged binary is missing. */
async function installMissing(target, entry, commands) {
  const baseUrl = releaseBaseUrl();
  const staging = fs.mkdtempSync(path.join(packageRoot, '.install-'));
  try {
    const archivePath = path.join(staging, entry.archive);
    await download(`${baseUrl}/${entry.archive}`, archivePath);
    if (await sha256(archivePath) !== entry.archiveSha256) throw new Error('release archive checksum mismatch');
    for (const command of commands) {
      const destination = path.join(packageRoot, 'vendor', target, 'bin', executableName(command));
      const extracted = path.join(staging, executableName(command));
      extractBinary(archivePath, extracted, command);
      if (await sha256(extracted) !== entry.binaries[command]) throw new Error('release binary checksum mismatch');
      fs.mkdirSync(path.dirname(destination), { recursive: true });
      fs.chmodSync(extracted, 0o755);
      fs.renameSync(extracted, destination);
    }
  } finally { fs.rmSync(staging, { recursive: true, force: true }); }
}

/** Validate the current platform payload; healthy npm installs never use HTTP. */
async function main() {
  const target = detectTarget();
  const entry = manifest.targets[target];
  if (manifest.version !== metadata.version || !entry) throw new Error('invalid npm binary manifest');
  const missing = [];
  for (const command of Object.keys(metadata.bin)) {
    if (!entry.binaries[command]) throw new Error('missing npm binary checksum');
    const filename = path.join(packageRoot, 'vendor', target, 'bin', executableName(command));
    if (!await isBundledBinaryValid(filename, entry.binaries[command])) missing.push(command);
  }
  if (missing.length) await installMissing(target, entry, missing);
  console.log(`installed ${metadata.name} for ${target}${missing.length ? ' (GitHub fallback)' : ' (bundled)'}`);
}

main().catch((error) => {
  console.error(`[${metadata.name}] ${error.message}`);
  process.exitCode = 1;
});
