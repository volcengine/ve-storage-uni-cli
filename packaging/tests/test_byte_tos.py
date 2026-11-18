import hashlib
import importlib.util
import io
import os
import subprocess
import sys
import tarfile
import zipfile
from pathlib import Path
from types import SimpleNamespace

import pytest


ROOT = Path(__file__).resolve().parents[2]
MAC_TARGET = "aarch64-apple-darwin"
WINDOWS_TARGET = "x86_64-pc-windows-msvc"
DOWNLOAD_BASE = "https://tosv.byted.org/obj/tos-team/toscli/new"


def load_module(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / "packaging/scripts" / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def source_archive(source_dir, target=MAC_TARGET, *, is_link=False):
    source_dir.mkdir(exist_ok=True)
    is_windows = target == WINDOWS_TARGET
    suffix = "zip" if is_windows else "tar.gz"
    archive = source_dir / f"ve-storage-uni-cli-{target}.{suffix}"
    binary_name = "bin/tos-cli.exe" if is_windows else "bin/tos-cli"
    binary_bytes = b"#!/bin/sh\nprintf 'tos-cli 1.0.3\\n'\n"
    if is_windows:
        with zipfile.ZipFile(archive, "w") as bundle:
            bundle.writestr(binary_name, binary_bytes)
            bundle.writestr("bin/ve-tos-cli.exe", b"other-cli")
    else:
        with tarfile.open(archive, "w:gz") as bundle:
            member = tarfile.TarInfo(binary_name)
            member.size = len(binary_bytes)
            if is_link:
                member.type = tarfile.SYMTYPE
                member.linkname = "../../outside"
            bundle.addfile(member, None if is_link else io.BytesIO(binary_bytes))
            extra = tarfile.TarInfo("bin/ve-tos-cli")
            bundle.addfile(extra)
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    with (source_dir / "SHA256SUMS").open("a") as sums:
        sums.write(f"{digest}  {archive.name}\n")
    return archive


def release_args(work_dir, *extra):
    release = load_module("release")
    return release.build_parser().parse_args([
        "byte-tos", "--version", "1.0.3", "--source-dir", str(work_dir / "source"),
        "--out-dir", str(work_dir / "output"),
        *extra,
    ])


def test_byte_tos_defaults_to_public_bucket_url(tmp_path):
    args = release_args(tmp_path)

    assert args.download_base_url == DOWNLOAD_BASE


def test_byte_tos_can_override_download_mirror(tmp_path):
    source_archive(tmp_path / "source")
    args = release_args(
        tmp_path, "--download-base-url", "https://mirror.example.test/toscli/new/",
    )
    channel = load_module("byte_tos")

    _, installer = channel._prepare(args)

    assert "DEFAULT_DOWNLOAD_BASE_URL=https://mirror.example.test/toscli/new\n" in installer.read_text()


def test_byte_tos_previews_dedicated_archives_without_running_tos(tmp_path, monkeypatch, capsys):
    source_archive(tmp_path / "source")
    args = release_args(tmp_path)
    channel = load_module("byte_tos")
    monkeypatch.setattr(channel, "_sdk_client", lambda options: pytest.fail("unexpected SDK use"))

    channel.run(args)

    destination = tmp_path / "output/1.0.3"
    archive = destination / f"tos-cli-1.0.3-{MAC_TARGET}.tar.gz"
    with tarfile.open(archive) as bundle:
        assert bundle.getnames() == ["bin/tos-cli", "LICENSE"]
        assert bundle.getmember("bin/tos-cli").mode == 0o755
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    assert (destination / "SHA256SUMS").read_text() == f"{digest}  {archive.name}\n"
    installer = tmp_path / "output/install.sh"
    assert f"DEFAULT_DOWNLOAD_BASE_URL={DOWNLOAD_BASE}" in installer.read_text()
    output = capsys.readouterr().out
    assert f"tos://tos-team/toscli/new/1.0.3/{archive.name}" in output
    assert "tos://tos-team/toscli/new/install.sh" in output
    assert "releases/" not in output
    assert not list((tmp_path / "output").rglob("*.json"))


def test_byte_tos_packages_windows_and_is_reproducible(tmp_path):
    source_archive(tmp_path / "source", WINDOWS_TARGET)
    args = release_args(tmp_path)
    channel = load_module("byte_tos")
    assets, _ = channel._prepare(args)
    first_archive = assets[0].read_bytes()
    with zipfile.ZipFile(assets[0]) as bundle:
        assert bundle.namelist() == ["bin/tos-cli.exe", "LICENSE"]

    repeated_assets, _ = channel._prepare(args)

    assert repeated_assets[0].read_bytes() == first_archive


def test_byte_tos_tar_is_reproducible(tmp_path):
    source_archive(tmp_path / "source")
    channel = load_module("byte_tos")
    args = release_args(tmp_path)
    assets, _ = channel._prepare(args)
    first_archive = assets[0].read_bytes()

    repeated_assets, _ = channel._prepare(args)

    assert repeated_assets[0].read_bytes() == first_archive


@pytest.mark.parametrize("output_dir", ["source", "."])
def test_byte_tos_rejects_output_that_would_replace_source_installer(tmp_path, output_dir):
    source_archive(tmp_path / "source")
    args = release_args(tmp_path, "--out-dir", str(tmp_path / output_dir))
    if output_dir == ".":
        args.source_dir = tmp_path / "1.0.3"
        source_archive(args.source_dir)

    with pytest.raises(SystemExit, match="overlap"):
        args.func(args)


@pytest.mark.parametrize("version", ["../escape", "v1.0.3", "1.0.3/extra", "1.0.3;echo secret"])
def test_byte_tos_rejects_unsafe_versions_before_writing(tmp_path, version):
    args = release_args(tmp_path, "--version", version)

    with pytest.raises(SystemExit, match="version"):
        args.func(args)

    assert not (tmp_path / "output").exists()


@pytest.mark.parametrize("url", [
    "http://download.test", "https://user:secret@download.test",
    "https://download.test/?token=secret", "https://download.test/%2e%2e/private",
    "https://download.test/?", "https://download.test/#",
])
def test_byte_tos_rejects_unsafe_download_urls(tmp_path, url, capsys):
    args = release_args(tmp_path, "--download-base-url", url)

    with pytest.raises(SystemExit, match="HTTPS") as error:
        args.func(args)

    assert "secret" not in str(error.value) + capsys.readouterr().out
    assert not (tmp_path / "output").exists()


def test_byte_tos_rejects_modified_source_archive(tmp_path):
    archive = source_archive(tmp_path / "source")
    archive.write_bytes(archive.read_bytes() + b"tampered")
    args = release_args(tmp_path)

    with pytest.raises(SystemExit, match="checksum"):
        args.func(args)


def test_byte_tos_rejects_link_in_place_of_binary(tmp_path):
    source_archive(tmp_path / "source", is_link=True)
    args = release_args(tmp_path)

    with pytest.raises(SystemExit, match="regular file"):
        args.func(args)

    assert not (tmp_path / "outside").exists()


def test_byte_tos_explicit_missing_target_fails_before_upload(tmp_path):
    source_archive(tmp_path / "source")
    args = release_args(tmp_path, "--target", WINDOWS_TARGET)

    with pytest.raises(SystemExit, match="missing.*archive"):
        args.func(args)


class FakeSDKError(Exception):
    def __init__(self, code):
        super().__init__("server error contains test-access-key and test-secret-key")
        self.code = code
        self.request_id = "test-request-id"
        self.resp = SimpleNamespace(close=lambda: None)


class SDKStorage:
    def __init__(self):
        self.objects = {}
        self.calls = []
        self.errors = []
        self.responses = []
        self.closed = False
        self.session = SimpleNamespace(close=self.close)
        self.initialization = None
        self.discovery_families = []

    def close(self):
        self.closed = True

    def put_object(self, key, source, headers=None):
        contents = source.read()
        self.calls.append(("PUT", key, contents, headers))
        if self.errors:
            raise self.errors.pop(0)
        if headers.get("If-None-Match") == "*" and key in self.objects:
            raise FakeSDKError(412)
        self.objects[key] = contents
        return self.response(b"")

    def get_object(self, key):
        self.calls.append(("GET", key, None, None))
        return self.response(self.objects[key])

    def response(self, contents):
        raw = io.BytesIO(contents)
        self.responses.append(raw)
        return SimpleNamespace(raw=raw)


def configure_sdk(monkeypatch):
    monkeypatch.setenv("BYTE_TOSTEAM_ACCESS_KEY", "test-access-key")
    monkeypatch.delenv("BYTE_TOSTEAM_SECRET_KEY", raising=False)
    storage = SDKStorage()

    def client(bucket, credentials, **options):
        storage.initialization = (bucket, credentials, options)
        return storage

    sdk = SimpleNamespace(
        Client=client,
        TosException=FakeSDKError,
    )
    monkeypatch.setitem(sys.modules, "bytedtos", sdk)
    monkeypatch.setitem(sys.modules, "servicediscovery", SimpleNamespace(
        set_address_family=storage.discovery_families.append,
    ))
    return storage


def test_byte_tos_sdk_uses_endpoint_without_service_discovery(tmp_path, monkeypatch):
    storage = configure_sdk(monkeypatch)
    monkeypatch.setitem(sys.modules, "servicediscovery", None)
    channel = load_module("byte_tos")

    client = channel._sdk_client(release_args(tmp_path))

    assert client is storage
    assert storage.initialization[2]["endpoint"] == "tos-cn-north.byted.org"
    assert storage.discovery_families == []


def test_byte_tos_sdk_upload_never_executes_a_binary(tmp_path, monkeypatch, capsys):
    source_archive(tmp_path / "source", WINDOWS_TARGET)
    storage = configure_sdk(monkeypatch)
    args = release_args(tmp_path, "--execute")
    monkeypatch.setattr(subprocess, "run", lambda *a, **kw: pytest.fail("unexpected binary execution"))

    args.func(args)

    bucket, credentials, options = storage.initialization
    assert bucket == "tos-team"
    assert credentials == "test-access-key"
    assert options["stream"] is True
    assert options["timeout"] > 0
    assert storage.session.max_redirects == 0
    assert storage.discovery_families == []
    assert [call[0] for call in storage.calls] == ["PUT", "GET"] * 3
    assert storage.calls[-2][1] == "toscli/new/install.sh"
    assert storage.calls[-2][3] == {}
    assert storage.closed
    assert all(response.closed for response in storage.responses)
    output = capsys.readouterr().out
    assert "test-access-key" not in output and "test-secret-key" not in output


@pytest.mark.parametrize("access_key", [None, "", " ", "test-access-key\n", " test-access-key"])
def test_byte_tos_sdk_requires_a_valid_bucket_access_key(tmp_path, monkeypatch, access_key):
    source_archive(tmp_path / "source")
    storage = configure_sdk(monkeypatch)
    if access_key is None:
        monkeypatch.delenv("BYTE_TOSTEAM_ACCESS_KEY")
    else:
        monkeypatch.setenv("BYTE_TOSTEAM_ACCESS_KEY", access_key)
    args = release_args(tmp_path, "--execute")

    with pytest.raises(SystemExit, match="BYTE_TOSTEAM_ACCESS_KEY") as error:
        args.func(args)

    assert storage.calls == []
    assert "test-access-key" not in str(error.value)


def test_byte_tos_sdk_ignores_legacy_secret_key_environment(tmp_path, monkeypatch):
    source_archive(tmp_path / "source")
    storage = configure_sdk(monkeypatch)
    monkeypatch.setenv("BYTE_TOSTEAM_SECRET_KEY", "legacy-value-must-not-be-used")
    args = release_args(tmp_path, "--execute")

    args.func(args)

    assert storage.initialization[1] == "test-access-key"


def test_byte_tos_requires_version_value_before_sdk_is_used(tmp_path, monkeypatch):
    storage = configure_sdk(monkeypatch)

    with pytest.raises(SystemExit) as error:
        release_args(tmp_path, "--version", "--execute")

    assert error.value.code == 2
    assert storage.discovery_families == []
    assert storage.calls == []


def test_byte_tos_sdk_missing_dependency_has_install_instructions(tmp_path, monkeypatch):
    source_archive(tmp_path / "source")
    configure_sdk(monkeypatch)
    monkeypatch.setitem(sys.modules, "bytedtos", None)
    args = release_args(tmp_path, "--execute")

    with pytest.raises(SystemExit, match="pip install bytedtos --index-url=https://bytedpypi.byted.org/simple"):
        args.func(args)


@pytest.mark.parametrize("is_same", [True, False])
def test_byte_tos_sdk_preserves_and_verifies_existing_version(tmp_path, monkeypatch, is_same):
    source_archive(tmp_path / "source")
    storage = configure_sdk(monkeypatch)
    args = release_args(tmp_path, "--execute")
    channel = load_module("byte_tos")
    assets, _ = channel._prepare(args)
    key = f"toscli/new/1.0.3/{assets[0].name}"
    old_contents = assets[0].read_bytes() if is_same else b"old content"
    storage.objects[key] = old_contents

    if is_same:
        channel.run(args)
    else:
        with pytest.raises(SystemExit, match="checksum mismatch"):
            channel.run(args)

    assert storage.objects[key] == old_contents
    assert storage.closed


def test_byte_tos_sdk_reopens_stream_for_transient_retry(tmp_path, monkeypatch):
    source_archive(tmp_path / "source")
    storage = configure_sdk(monkeypatch)
    storage.errors.append(FakeSDKError(503))
    args = release_args(tmp_path, "--execute")
    channel = load_module("byte_tos")
    monkeypatch.setattr(channel.time, "sleep", lambda seconds: None)

    channel.run(args)

    assert storage.calls[0][2] == storage.calls[1][2]
    assert storage.closed


def test_byte_tos_sdk_auth_failure_is_redacted_and_stops_release(tmp_path, monkeypatch, capsys):
    source_archive(tmp_path / "source")
    storage = configure_sdk(monkeypatch)
    storage.errors.append(FakeSDKError(403))
    args = release_args(tmp_path, "--execute")

    with pytest.raises(SystemExit, match="HTTP 403") as error:
        args.func(args)

    output = str(error.value) + capsys.readouterr().out
    assert "test-access-key" not in output and "test-secret-key" not in output
    assert len(storage.calls) == 1
    assert storage.closed


def test_byte_tos_sdk_transport_error_does_not_expose_credentials(tmp_path, monkeypatch, capsys):
    source_archive(tmp_path / "source")
    storage = configure_sdk(monkeypatch)
    storage.errors.append(OSError("signed URL contains test-access-key and test-secret-key"))
    args = release_args(tmp_path, "--execute")

    with pytest.raises(SystemExit, match="bytedtos PUT failed") as error:
        args.func(args)

    output = str(error.value) + capsys.readouterr().out
    assert "test-access-key" not in output and "test-secret-key" not in output
    assert storage.closed


def test_byte_tos_sdk_cleanup_error_is_redacted(tmp_path, monkeypatch, capsys):
    source_archive(tmp_path / "source")
    storage = configure_sdk(monkeypatch)
    error = FakeSDKError(403)

    def close_response():
        raise OSError("cleanup context contains test-access-key and test-secret-key")

    error.resp.close = close_response
    storage.errors.append(error)
    args = release_args(tmp_path, "--execute")

    with pytest.raises(SystemExit, match="error response close") as failure:
        args.func(args)

    output = str(failure.value) + capsys.readouterr().out
    assert "test-access-key" not in output and "test-secret-key" not in output
    assert storage.closed


def test_byte_tos_sdk_clobber_replaces_existing_version(tmp_path, monkeypatch):
    source_archive(tmp_path / "source")
    storage = configure_sdk(monkeypatch)
    args = release_args(tmp_path, "--execute", "--clobber")
    channel = load_module("byte_tos")
    assets, _ = channel._prepare(args)
    key = f"toscli/new/1.0.3/{assets[0].name}"
    storage.objects[key] = b"old version"

    channel.run(args)

    assert storage.objects[key] == assets[0].read_bytes()
    assert all(not call[3] for call in storage.calls if call[0] == "PUT")


def installer_environment(work_dir):
    tools_dir = work_dir / "tools"
    tools_dir.mkdir()
    curl_script = tools_dir / "curl"
    curl_script.write_text(
        "#!/usr/bin/env python3\nimport shutil, sys\nfrom pathlib import Path\n"
        "from urllib.parse import urlsplit\n"
        f"root = Path({str(work_dir / 'output')!r})\n"
        "args = sys.argv[1:]\nurl = next(arg for arg in args if arg.startswith('https://'))\n"
        "relative = urlsplit(url).path.split('/toscli/new/', 1)[1]\n"
        "shutil.copyfile(root / relative, args[args.index('-o') + 1])\n"
    )
    curl_script.chmod(0o755)
    uname_script = tools_dir / "uname"
    uname_script.write_text("#!/bin/sh\ncase \"$1\" in -s) echo Darwin;; -m) echo arm64;; esac\n")
    uname_script.chmod(0o755)
    return {**os.environ, "PATH": f"{tools_dir}{os.pathsep}{os.environ['PATH']}"}


@pytest.mark.parametrize("is_corrupt", [False, True])
def test_internal_installer_checks_archive_before_replacing_binary(tmp_path, is_corrupt):
    source_archive(tmp_path / "source")
    args = release_args(tmp_path)
    channel = load_module("byte_tos")
    assets, installer = channel._prepare(args)
    if is_corrupt:
        assets[0].write_bytes(assets[0].read_bytes() + b"tampered")
    install_dir = tmp_path / "installed"
    install_dir.mkdir()
    installed_binary = install_dir / "tos-cli"
    installed_binary.write_text("previous binary")

    result = subprocess.run(
        ["sh", str(installer), "--version", "1.0.3", "--install-dir", str(install_dir)],
        env=installer_environment(tmp_path), capture_output=True, text=True, timeout=15,
    )

    if is_corrupt:
        assert result.returncode != 0
        assert "checksum" in result.stderr.lower()
        assert installed_binary.read_text() == "previous binary"
    else:
        assert result.returncode == 0, result.stderr
        assert os.access(installed_binary, os.X_OK)
        assert subprocess.check_output([str(installed_binary)], text=True).strip() == "tos-cli 1.0.3"


def test_internal_installer_requires_explicit_version(tmp_path):
    source_archive(tmp_path / "source")
    args = release_args(tmp_path)
    _, installer = load_module("byte_tos")._prepare(args)

    result = subprocess.run(["sh", str(installer)], capture_output=True, text=True, timeout=5)

    assert result.returncode != 0
    assert "--version" in result.stderr
