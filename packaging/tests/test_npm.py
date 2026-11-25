"""Exercise generated npm packages through real Node and npm entrypoints."""

import hashlib
import io
import json
import os
import shutil
import stat
import subprocess
import tarfile
import zipfile
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[2]
NPM_SCRIPT = ROOT / "packaging/scripts/npm.py"


def generate_package(archive_dir, output_dir, package="ve-adrive-cli"):
    return subprocess.run(
        ["python3", str(NPM_SCRIPT), "--version", "1.0.5", "--package", package,
         "--archive-dir", str(archive_dir), "--out-dir", str(output_dir)],
        capture_output=True, text=True, check=False,
    )


@pytest.fixture
def generated_npm(npm_archives, tmp_path):
    result = generate_package(npm_archives, tmp_path / "npm")
    assert result.returncode == 0, result.stderr
    return tmp_path / "npm/ve-adrive-cli"


def node_script(package_dir, source, **environment):
    bootstrap = "const testBinaryName = require('./lib/platform').executableName('ve-adrive-cli');\n"
    return subprocess.run(
        ["node", "-e", bootstrap + source], cwd=package_dir, capture_output=True, text=True,
        env={**os.environ, **environment}, timeout=20, check=False,
    )


def update_archive_checksum(archive_dir, archive_path):
    checksum_file = archive_dir / "SHA256SUMS"
    digest = hashlib.sha256(archive_path.read_bytes()).hexdigest()
    checksum_file.write_text("\n".join(
        f"{digest}  {archive_path.name}" if line.endswith(archive_path.name) else line
        for line in checksum_file.read_text().splitlines()
    ) + "\n")


@pytest.mark.parametrize("package", ["ve-tos-cli", "ve-adrive-cli"])
def test_package_contains_only_its_own_binary_for_all_five_platforms(npm_archives, tmp_path, package):
    result = generate_package(npm_archives, tmp_path / "npm", package)
    assert result.returncode == 0, result.stderr
    package_dir = tmp_path / "npm" / package
    binaries = sorted((package_dir / "vendor").glob("*/bin/*"))
    assert len(binaries) == 5
    assert all(binary.name in (package, f"{package}.exe") for binary in binaries)
    manifest = json.loads((package_dir / "lib/binaries.json").read_text())
    assert manifest["version"] == "1.0.5"
    for binary in binaries:
        expected = manifest["targets"][binary.parents[1].name]["binaries"][package]
        assert hashlib.sha256(binary.read_bytes()).hexdigest() == expected


@pytest.mark.parametrize("existing_rows", [0, 1])
def test_packaging_automatically_creates_or_completes_checksums(npm_archives, tmp_path, existing_rows):
    checksum_file = npm_archives / "SHA256SUMS"
    expected = checksum_file.read_text()
    if existing_rows:
        checksum_file.write_text(expected.splitlines()[0] + "\n")
    else:
        checksum_file.unlink()
    result = generate_package(npm_archives, tmp_path / "npm")
    assert result.returncode == 0, result.stderr
    assert set(checksum_file.read_text().splitlines()) == set(expected.splitlines())


def test_existing_checksum_mismatch_is_not_overwritten(npm_archives, tmp_path):
    checksum_file = npm_archives / "SHA256SUMS"
    expected = checksum_file.read_bytes()
    archive = next(npm_archives.glob("*.tar.gz"))
    archive.write_bytes(b"modified archive")
    result = generate_package(npm_archives, tmp_path / "npm")
    assert result.returncode != 0
    assert "checksum mismatch" in result.stderr
    assert checksum_file.read_bytes() == expected


def test_missing_archive_does_not_write_partial_checksums(npm_archives, tmp_path):
    checksum_file = npm_archives / "SHA256SUMS"
    checksum_file.unlink()
    next(npm_archives.glob("*.zip")).unlink()
    result = generate_package(npm_archives, tmp_path / "npm")
    assert result.returncode != 0
    assert not checksum_file.exists()


def test_generator_uses_dist_without_explicit_archive_directory(npm_archives, tmp_path):
    shutil.copytree(npm_archives, tmp_path / "dist")
    (tmp_path / "dist/SHA256SUMS").unlink()
    result = subprocess.run(
        ["python3", str(NPM_SCRIPT), "--version", "1.0.5", "--package", "ve-adrive-cli",
         "--config", str(ROOT / "packaging/npm/packages.json")],
        cwd=tmp_path, capture_output=True, text=True, check=False, timeout=20,
    )
    assert result.returncode == 0, result.stderr
    assert (tmp_path / "dist/npm/ve-adrive-cli/package.json").is_file()
    assert (tmp_path / "dist/SHA256SUMS").is_file()


def test_installer_uses_bundled_binary_without_even_attempting_network(generated_npm):
    result = node_script(generated_npm, """
        require('https').get = () => { throw Error('unexpected network'); };
        require('./scripts/install.js');
    """)
    assert result.returncode == 0, result.stderr
    assert not (generated_npm / ".download").exists()


def test_corrupt_bundled_binary_is_rejected_without_network(generated_npm):
    result = node_script(generated_npm, """
        const fs = require('fs');
        const target = require('./lib/platform').detectTarget();
        fs.appendFileSync(`vendor/${target}/bin/${testBinaryName}`, 'corruption');
        require('https').get = () => { throw Error('unexpected network'); };
        require('./scripts/install.js');
    """)
    assert result.returncode != 0
    assert "checksum mismatch" in result.stderr
    assert "unexpected network" not in result.stderr


def test_missing_binary_uses_only_the_same_release_and_cleans_failed_download(generated_npm):
    result = node_script(generated_npm, """
        const fs = require('fs');
        const target = require('./lib/platform').detectTarget();
        fs.unlinkSync(`vendor/${target}/bin/${testBinaryName}`);
        require('https').get = (url) => { throw Error(`requested ${url}`); };
        require('./scripts/install.js');
    """, VE_STORAGE_UNI_CLI_VERSION="v1.0.5")
    assert result.returncode != 0
    assert "/releases/download/v1.0.5/ve-storage-uni-cli-" in result.stderr
    assert not (generated_npm / ".download").exists()
    assert not list(generated_npm.glob(".install-*"))


def test_missing_binary_rejects_version_override(generated_npm):
    result = node_script(generated_npm, """
        const fs = require('fs');
        const target = require('./lib/platform').detectTarget();
        fs.unlinkSync(`vendor/${target}/bin/${testBinaryName}`);
        require('https').get = () => { throw Error('unexpected network'); };
        require('./scripts/install.js');
    """, VE_STORAGE_UNI_CLI_VERSION="latest")
    assert result.returncode != 0
    assert "must match" in result.stderr
    assert "unexpected network" not in result.stderr


@pytest.mark.parametrize("is_corrupt", [False, True])
def test_fallback_download_validates_archive_and_installs_selected_binary(generated_npm, npm_archives, is_corrupt):
    result = node_script(generated_npm, """
        const fs = require('fs');
        const path = require('path');
        const { EventEmitter } = require('events');
        const { Readable } = require('stream');
        const target = require('./lib/platform').detectTarget();
        fs.unlinkSync(`vendor/${target}/bin/${testBinaryName}`);
        require('https').get = (url, callback) => {
          if (!String(url).includes('/releases/download/v1.0.5/')) throw Error('wrong version');
          const request = new EventEmitter();
          process.nextTick(() => {
            const archive = path.join(process.env.ARCHIVE_DIR, path.basename(url.pathname));
            const response = Readable.from([process.env.IS_CORRUPT === '1' ? Buffer.from('bad') : fs.readFileSync(archive)]);
            response.statusCode = 200;
            response.headers = {};
            callback(response);
          });
          return request;
        };
        require('./scripts/install.js');
    """, ARCHIVE_DIR=str(npm_archives), IS_CORRUPT=str(int(is_corrupt)))
    assert not list(generated_npm.glob(".install-*"))
    if is_corrupt:
        assert result.returncode != 0
        assert "archive checksum mismatch" in result.stderr
    else:
        assert result.returncode == 0, result.stderr
        assert "GitHub fallback" in result.stdout
        if os.name != "nt":
            execution = subprocess.run(
                ["node", str(generated_npm / "bin/ve-adrive-cli.js"), "--version"],
                capture_output=True, text=True, check=False, timeout=10,
            )
            assert execution.returncode == 0, execution.stderr
            assert execution.stdout.strip() == "fixture --version"


@pytest.mark.parametrize("response_kind", ["timeout", "redirect_loop", "http_redirect", "invalid_redirect", "oversized"])
def test_downloader_bounds_and_https_only_policy(generated_npm, tmp_path, response_kind):
    result = node_script(generated_npm, """
        const { EventEmitter } = require('events');
        const { Readable } = require('stream');
        require('https').get = (url, callback) => {
          const request = new EventEmitter();
          request.destroy = (error) => request.emit('error', error);
          if (process.env.RESPONSE_KIND === 'timeout') return request;
          process.nextTick(() => {
            const response = Readable.from([Buffer.from('too many bytes')]);
            response.statusCode = process.env.RESPONSE_KIND === 'oversized' ? 200 : 302;
            response.headers = { location: process.env.RESPONSE_KIND === 'http_redirect'
              ? 'http://example.invalid/archive' : process.env.RESPONSE_KIND === 'invalid_redirect'
                ? 'https://[' : '/archive' };
            callback(response);
          });
          return request;
        };
        require('./lib/download').download('https://example.invalid/archive', process.env.DESTINATION,
          { timeoutMs: 50, maxBytes: 2 }).then(() => { throw Error('unexpected success'); })
          .catch((error) => { console.error(error.message); process.exitCode = 1; });
    """, RESPONSE_KIND=response_kind, DESTINATION=str(tmp_path / "archive"))
    expected = {"timeout": "timed out", "redirect_loop": "redirect limit",
                "http_redirect": "requires HTTPS", "invalid_redirect": "invalid release redirect",
                "oversized": "size limit"}
    assert result.returncode != 0
    assert expected[response_kind] in result.stderr


def test_failed_download_waits_for_output_close_before_cleanup(generated_npm, tmp_path):
    result = node_script(generated_npm, """
        const fs = require('fs');
        const { EventEmitter } = require('events');
        const { Readable } = require('stream');
        const originalCreate = fs.createWriteStream;
        let request;
        let response;
        let isClosed = false;
        fs.createWriteStream = (...args) => {
          const output = originalCreate(...args);
          const originalDestroy = output._destroy;
          output._destroy = (error, done) => setTimeout(() => originalDestroy.call(output, error, done), 25);
          output.on('close', () => { isClosed = true; });
          output.on('open', () => {
            const error = Error('connection interrupted');
            request.emit('error', error);
            response.destroy(error);
          });
          return output;
        };
        require('https').get = (url, callback) => {
          request = new EventEmitter();
          response = new Readable({ read() {} });
          response.statusCode = 200;
          process.nextTick(() => callback(response));
          return request;
        };
        require('./lib/download').download('https://example.invalid/archive', process.env.DESTINATION)
          .then(() => { throw Error('unexpected success'); })
          .catch(() => { console.log(isClosed ? 'closed' : 'open'); });
    """, DESTINATION=str(tmp_path / "download"))
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == "closed"


@pytest.mark.parametrize("arch", ["x64", "arm64"])
def test_linux_musl_is_rejected_without_network(generated_npm, arch):
    result = node_script(generated_npm, """
        Object.defineProperty(process, 'platform', { value: 'linux' });
        Object.defineProperty(process, 'arch', { value: process.env.ARCH });
        process.report.getReport = () => ({ header: {} });
        require('https').get = () => { throw Error('unexpected network'); };
        require('./scripts/install.js');
    """, ARCH=arch)
    assert result.returncode != 0
    assert "unsupported platform" in result.stderr
    assert "unexpected network" not in result.stderr


def test_missing_binary_fallback_over_real_trusted_https(generated_npm, npm_https_release):
    repository, certificate, requests = npm_https_release
    result = node_script(generated_npm, """
        const fs = require('fs');
        const target = require('./lib/platform').detectTarget();
        fs.unlinkSync(`vendor/${target}/bin/${testBinaryName}`);
        require('./scripts/install.js');
    """, VE_STORAGE_UNI_CLI_REPO_URL=repository, NODE_EXTRA_CA_CERTS=str(certificate))
    assert result.returncode == 0, result.stderr
    assert "GitHub fallback" in result.stdout
    assert len(requests) == 2
    assert requests[0].startswith("/repo/releases/download/v1.0.5/")
    assert requests[1].startswith("/assets/")
    assert not list(generated_npm.glob(".install-*"))


@pytest.mark.parametrize("failure", ["missing_archive", "missing_binary", "bad_checksum"])
def test_incomplete_or_modified_release_cannot_generate_a_publishable_package(npm_archives, tmp_path, failure):
    archive_path = npm_archives / "ve-storage-uni-cli-aarch64-apple-darwin.tar.gz"
    if failure == "missing_archive":
        archive_path.unlink()
    elif failure == "missing_binary":
        with tarfile.open(archive_path, "w:gz"):
            pass
        update_archive_checksum(npm_archives, archive_path)
    else:
        archive_path.write_bytes(b"corruption")
    result = generate_package(npm_archives, tmp_path / "npm")
    assert result.returncode != 0
    assert not (tmp_path / "npm/ve-adrive-cli/package.json").exists()


@pytest.mark.parametrize("kind", ["tar_symlink", "tar_hardlink", "tar_duplicate", "zip_symlink", "zip_device", "zip_duplicate"])
def test_nonregular_or_duplicate_binary_entries_are_rejected(npm_archives, tmp_path, kind):
    is_zip = kind.startswith("zip")
    target = "x86_64-pc-windows-msvc" if is_zip else "aarch64-apple-darwin"
    suffix = ".zip" if is_zip else ".tar.gz"
    archive_path = npm_archives / f"ve-storage-uni-cli-{target}{suffix}"
    member_name = "bin/ve-adrive-cli.exe" if is_zip else "bin/ve-adrive-cli"
    if is_zip:
        with zipfile.ZipFile(archive_path, "w") as archive:
            member = zipfile.ZipInfo(member_name)
            file_kind = stat.S_IFLNK if kind == "zip_symlink" else stat.S_IFCHR
            member.external_attr = (file_kind | 0o755) << 16
            archive.writestr(member, b"binary")
            if kind == "zip_duplicate":
                with pytest.warns(UserWarning, match="Duplicate name"):
                    archive.writestr(member_name, b"another binary")
    else:
        with tarfile.open(archive_path, "w:gz") as archive:
            member = tarfile.TarInfo(member_name)
            if kind == "tar_duplicate":
                member.size = 6
                archive.addfile(member, io.BytesIO(b"binary"))
                archive.addfile(member, io.BytesIO(b"binary"))
            else:
                member.type = tarfile.SYMTYPE if kind == "tar_symlink" else tarfile.LNKTYPE
                member.linkname = "../../outside"
                archive.addfile(member)
    update_archive_checksum(npm_archives, archive_path)
    result = generate_package(npm_archives, tmp_path / "npm")
    assert result.returncode != 0
    assert "regular" in result.stderr
    assert not (tmp_path / "npm/ve-adrive-cli").exists()


def test_unrelated_traversal_member_is_neither_extracted_nor_packaged(npm_archives, tmp_path):
    archive_path = npm_archives / "ve-storage-uni-cli-aarch64-apple-darwin.tar.gz"
    with tarfile.open(archive_path, "w:gz") as archive:
        for name in ("bin/ve-adrive-cli", "../../outside"):
            member = tarfile.TarInfo(name)
            member.size = 6
            archive.addfile(member, io.BytesIO(b"binary"))
    update_archive_checksum(npm_archives, archive_path)
    result = generate_package(npm_archives, tmp_path / "npm")
    assert result.returncode == 0, result.stderr
    assert not list(tmp_path.rglob("outside"))
    assert len(list((tmp_path / "npm/ve-adrive-cli/vendor").glob("*/bin/*"))) == 5


@pytest.mark.parametrize("is_ignore_scripts", [False, True])
@pytest.mark.skipif(os.name == "nt", reason="executable fixture is a POSIX shell script")
def test_real_npm_pack_and_offline_install(generated_npm, tmp_path, is_ignore_scripts):
    if shutil.which("npm") is None:
        pytest.skip("npm is unavailable")
    cache_dir = tmp_path / "npm-cache"
    common = ["--cache", str(cache_dir), "--offline", "--no-audit", "--no-fund"]
    packed = subprocess.run(
        ["npm", "pack", "--json", *common], cwd=generated_npm,
        capture_output=True, text=True, check=True, timeout=30,
    )
    tarball = generated_npm / json.loads(packed.stdout)[0]["filename"]
    with tarfile.open(tarball) as archive:
        assert len([name for name in archive.getnames() if "/vendor/" in name]) == 5
    prefix = tmp_path / "installed"
    subprocess.run(
        ["npm", "install", "--global", "--prefix", str(prefix),
         *(["--ignore-scripts"] if is_ignore_scripts else []), *common, str(tarball)],
        capture_output=True, text=True, check=True, timeout=30,
    )
    result = subprocess.run(
        [str(prefix / "bin/ve-adrive-cli"), "--version"],
        capture_output=True, text=True, check=False, timeout=10,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == "fixture --version"
