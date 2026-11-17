# Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
# SPDX-License-Identifier: Apache-2.0

"""Prepare and publish dedicated tos-cli archives to the internal TOS bucket."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import importlib
import os
import re
import shlex
import shutil
import stat
import tarfile
import tempfile
import time
import zipfile
from contextlib import nullcontext
from pathlib import Path
from urllib.parse import unquote, urlsplit


ROOT = Path(__file__).resolve().parents[2]
TOS_BUCKET = "tos-team"
TOS_ENDPOINT = "tos-cn-north.byted.org"
TOS_KEY_PREFIX = "toscli/new"
TOS_PREFIX = f"tos://{TOS_BUCKET}/{TOS_KEY_PREFIX}"
DEFAULT_DOWNLOAD_BASE_URL = "https://tosv.byted.org/obj/tos-team/toscli/new"
SDK_INSTALL_COMMAND = "python3 -m pip install bytedtos --index-url=https://bytedpypi.byted.org/simple"
SDK_RETRY_DELAYS = (1, 2)
VERSION_PATTERN = re.compile(
    r"[0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9][A-Za-z0-9.-]*)?"
    r"(?:\+[A-Za-z0-9][A-Za-z0-9.-]*)?"
)
MAX_BINARY_SIZE = 1024 * 1024 * 1024


def _archive_names() -> dict[str, str]:
    metadata = json.loads((ROOT / "packaging/release/platforms.json").read_text())
    return {entry["triple"]: entry["archive"] for entry in metadata["targets"]}


def _positive_timeout(value: str) -> int:
    seconds = int(value)
    if seconds <= 0:
        raise argparse.ArgumentTypeError("timeout must be positive")
    return seconds


def add_options(parser: argparse.ArgumentParser) -> None:
    """Add internal release options to parser; return None.

    Raises OSError/ValueError if repository platform metadata cannot be read.
    """
    parser.add_argument("--source-dir", type=Path, default=Path("dist"))
    parser.add_argument("--out-dir", type=Path, default=Path("dist/byte-tos"))
    parser.add_argument("--target", action="append", choices=tuple(_archive_names()))
    parser.add_argument(
        # [Review Fix #InternalPublicDownload] The public bucket has a stable
        # HTTPS route; releases only need an override for another mirror.
        "--download-base-url", default=DEFAULT_DOWNLOAD_BASE_URL,
        help=f"HTTPS download base (default: {DEFAULT_DOWNLOAD_BASE_URL})",
    )
    parser.add_argument(
        "--timeout", type=_positive_timeout, default=600, help="SDK request timeout in seconds",
    )
    parser.add_argument(
        "--clobber", action="store_true", help="Overwrite existing versioned objects",
    )
    parser.add_argument(
        "--execute", action="store_true", help="Upload and verify objects; default is preview",
    )


def _download_url(value: str) -> str:
    parsed = urlsplit(value)
    decoded = unquote(value)
    if (
        parsed.scheme != "https" or not parsed.hostname or parsed.username is not None
        or parsed.password is not None or "?" in value or "#" in value
        or any(ord(char) <= 32 for char in decoded)
        or any(part in (".", "..") for part in unquote(parsed.path).split("/"))
    ):
        raise ValueError(
            "download base must be a credential-free HTTPS URL "
            "without traversal/query/fragment"
        )
    return parsed._replace(scheme="https").geturl().rstrip("/")


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _source_archives(args: argparse.Namespace) -> list[tuple[str, Path]]:
    names = _archive_names()
    targets = args.target or [
        target for target, name in names.items() if (args.source_dir / name).is_file()
    ]
    if not targets:
        raise ValueError("no release archives found; run release.py archives first")
    sources = [(target, args.source_dir / names[target]) for target in dict.fromkeys(targets)]
    if any(not path.is_file() for _, path in sources):
        raise ValueError("missing requested release archive; run release.py archives first")
    checksums = {}
    for line in (args.source_dir / "SHA256SUMS").read_text(encoding="utf-8").splitlines():
        if line.strip():
            checksum, filename = line.split(maxsplit=1)
            filename = filename.lstrip(" *")
            if filename in checksums:
                raise ValueError("duplicate source checksum entry")
            checksums[filename] = checksum
    for _, path in sources:
        if checksums.get(path.name) != _sha256(path):
            raise ValueError(f"source archive checksum mismatch or missing: {path.name}")
    return sources


def _copy_tar_binary(source: Path, destination: Path) -> None:
    with tarfile.open(source, "r:gz") as bundle:
        members = [member for member in bundle.getmembers() if member.name == "bin/tos-cli"]
        if len(members) != 1 or not members[0].isreg():
            raise ValueError("source archive must contain one regular file bin/tos-cli")
        if not 0 < members[0].size <= MAX_BINARY_SIZE:
            raise ValueError("source binary size is invalid")
        # [Review Fix #InternalArchivePath] Read only the named regular member;
        # never extract archive paths, links, or unrelated CLI binaries.
        with bundle.extractfile(members[0]) as binary, destination.open("wb") as output:
            shutil.copyfileobj(binary, output)


def _copy_zip_binary(source: Path, destination: Path) -> None:
    with zipfile.ZipFile(source) as bundle:
        members = [member for member in bundle.infolist() if member.filename == "bin/tos-cli.exe"]
        if len(members) != 1:
            raise ValueError("source archive must contain one regular file bin/tos-cli.exe")
        member = members[0]
        mode = stat.S_IFMT(member.external_attr >> 16)
        if mode not in (0, stat.S_IFREG) or not 0 < member.file_size <= MAX_BINARY_SIZE:
            raise ValueError("source binary must be a regular file with a valid size")
        with bundle.open(member) as binary, destination.open("wb") as output:
            shutil.copyfileobj(binary, output)


def _write_tar(output: Path, entries: list[tuple[str, Path]]) -> None:
    # Stable archive metadata allows safe retries of an immutable version.
    with (
        output.open("wb") as stream,
        gzip.GzipFile(filename="", fileobj=stream, mode="wb", mtime=0) as compressed,
        tarfile.open(fileobj=compressed, mode="w") as bundle,
    ):
        for name, source in entries:
            member = tarfile.TarInfo(name)
            member.size = source.stat().st_size
            member.mode = 0o755 if name.startswith("bin/") else 0o644
            with source.open("rb") as contents:
                bundle.addfile(member, contents)


def _write_zip(output: Path, entries: list[tuple[str, Path]]) -> None:
    with zipfile.ZipFile(output, "w") as bundle:
        for name, source in entries:
            member = zipfile.ZipInfo(name)
            member.compress_type = zipfile.ZIP_DEFLATED
            member.create_system = 3
            mode = 0o755 if name.startswith("bin/") else 0o644
            member.external_attr = (stat.S_IFREG | mode) << 16
            with source.open("rb") as contents, bundle.open(member, "w") as output_stream:
                shutil.copyfileobj(contents, output_stream)


def _package_archive(source: Path, target: str, stage: Path, version: str) -> Path:
    is_windows = target.endswith("windows-msvc")
    suffix = "zip" if is_windows else "tar.gz"
    binary = stage / ("tos-cli.exe" if is_windows else "tos-cli")
    if is_windows:
        _copy_zip_binary(source, binary)
    else:
        _copy_tar_binary(source, binary)
    output = stage / f"tos-cli-{version}-{target}.{suffix}"
    entries = [(f"bin/{binary.name}", binary), ("LICENSE", ROOT / "LICENSE")]
    if is_windows:
        _write_zip(output, entries)
    else:
        _write_tar(output, entries)
    return output


def _prepare(args: argparse.Namespace) -> tuple[list[Path], Path]:
    if VERSION_PATTERN.fullmatch(args.version) is None:
        raise ValueError("invalid version; use a version such as 1.0.3 without leading v")
    download_base = _download_url(args.download_base_url)
    # [Review Fix #InternalOutputOverlap] Never replace the public install.sh
    # or SHA256SUMS when output paths overlap the source release directory.
    output_paths = (args.out_dir.resolve(), (args.out_dir / args.version).resolve())
    if args.source_dir.resolve() in output_paths:
        raise ValueError("internal output paths must not overlap source release assets")
    sources = _source_archives(args)
    args.out_dir.mkdir(parents=True, exist_ok=True)
    destination = args.out_dir / args.version
    with tempfile.TemporaryDirectory(prefix=".byte-tos-", dir=args.out_dir) as stage_name:
        stage = Path(stage_name)
        archives = [
            _package_archive(source, target, stage, args.version) for target, source in sources
        ]
        sums = stage / "SHA256SUMS"
        sums.write_text(
            "".join(f"{_sha256(path)}  {path.name}\n" for path in archives),
            encoding="utf-8", newline="\n",
        )
        template = (ROOT / "packaging/install/byte-tos-install.sh").read_text(encoding="utf-8")
        installer = stage / "install.sh"
        # [Review Fix #InternalInstallerLineEndings] Windows release runners
        # must generate LF shell scripts too, so POSIX installers can run them.
        installer.write_text(
            template.replace(
                "DEFAULT_DOWNLOAD_BASE_URL=''",
                f"DEFAULT_DOWNLOAD_BASE_URL={shlex.quote(download_base)}", 1,
            ), encoding="utf-8", newline="\n",
        )
        destination.mkdir(parents=True, exist_ok=True)
        assets = []
        for path in [*archives, sums]:
            final_path = destination / path.name
            path.replace(final_path)
            assets.append(final_path)
        installer.replace(args.out_dir / "install.sh")
    return assets, args.out_dir / "install.sh"


def _sdk_error(operation: str, error: Exception) -> str:
    # [Review Fix #InternalSdkRedaction] SDK exception text may contain a server
    # body, signed URL or credentials; report only status and request context.
    status = getattr(error, "code", None)
    status_text = f"HTTP {status}" if type(status) is int else type(error).__name__
    request_id = str(getattr(error, "request_id", "unknown"))
    request_id = re.sub(r"[^A-Za-z0-9_.:-]", "_", request_id)[:128]
    return f"bytedtos {operation} failed: {status_text}, request_id={request_id}"


def _sdk_client(args: argparse.Namespace):
    access_key = os.environ.get("BYTE_TOSTEAM_ACCESS_KEY")
    if not access_key or not access_key.strip():
        raise ValueError("missing environment variable: BYTE_TOSTEAM_ACCESS_KEY")
    if access_key != access_key.strip() or any(ord(char) < 33 or ord(char) == 127 for char in access_key):
        raise ValueError("BYTE_TOSTEAM_ACCESS_KEY contains whitespace or control characters")
    try:
        sdk = importlib.import_module("bytedtos")
    except ImportError:
        raise RuntimeError(f"missing bytedtos SDK or dependency; install with: {SDK_INSTALL_COMMAND}") from None
    try:
        # [Review Fix #InternalBucketAccessKey] This is the bucket's single
        # AccessKey. The SDK maps it to BucketAccessKeyCredentials(bucket, key);
        # StaticCredentials would treat that key as an independent IAM AK.
        # [Review Fix #InternalEndpoint] This bucket authenticates through its
        # endpoint; omitting it routes requests through BNS/PSM instead.
        client = sdk.Client(
            TOS_BUCKET, access_key, endpoint=TOS_ENDPOINT, auth_mode="AKSK",
            timeout=args.timeout, connect_timeout=min(10, args.timeout),
            stream=True, max_retry_count=0,
        )
        # [Review Fix #InternalSdkRedirect] Do not forward signed SDK headers
        # to an endpoint selected by an HTTP redirect.
        client.session.max_redirects = 0
        return client
    except Exception as error:
        raise RuntimeError(_sdk_error("client initialization", error)) from None


def _close_response(response) -> None:
    if response is None:
        return
    try:
        response.raw.close()
        release_connection = getattr(response.raw, "release_conn", None)
        if release_connection is not None:
            release_connection()
    except Exception as error:
        raise RuntimeError(_sdk_error("response close", error)) from None


def _close_failed_response(error: Exception) -> None:
    response = getattr(error, "resp", None)
    if response is not None:
        try:
            response.close()
        except Exception as close_error:
            # [Review Fix #InternalSdkCleanupRedaction] Cleanup errors can carry
            # the same sensitive request context as the original SDK error.
            raise RuntimeError(_sdk_error("error response close", close_error)) from None


def _sdk_request(client, method: str, key: str, asset: Path, is_clobber: bool = False):
    for attempt, retry_delay in enumerate((*SDK_RETRY_DELAYS, None)):
        # Reopen on retries; keep local file-open failures outside SDK handling.
        with asset.open("rb") if method == "PUT" else nullcontext(None) as source:
            try:
                if method == "PUT":
                    headers = {} if is_clobber else {"If-None-Match": "*"}
                    return client.put_object(key, source, headers=headers)
                return client.get_object(key)
            except Exception as error:
                _close_failed_response(error)
                status = getattr(error, "code", None)
                if method == "PUT" and not is_clobber and status in (409, 412):
                    return None
                if status not in (429, 500, 502, 503, 504) or retry_delay is None:
                    raise RuntimeError(_sdk_error(method, error)) from None
                print(f"# retry {method}, attempt {attempt + 1}: HTTP {status}", flush=True)
                time.sleep(retry_delay)
    raise RuntimeError("bytedtos retry budget exhausted")


def _remote_sha256(response, expected_size: int) -> str:
    digest = hashlib.sha256()
    received_size = 0
    try:
        while True:
            try:
                chunk = response.raw.read(1024 * 1024)
            except Exception as error:
                raise RuntimeError(_sdk_error("read-back", error)) from None
            if not chunk:
                return digest.hexdigest()
            received_size += len(chunk)
            if received_size > expected_size:
                raise ValueError("uploaded object checksum mismatch (unexpected size)")
            digest.update(chunk)
    finally:
        _close_response(response)


def _upload_verified(args: argparse.Namespace, client, asset: Path, key: str) -> None:
    is_clobber = args.clobber or key == f"{TOS_KEY_PREFIX}/install.sh"
    strategy = "replace" if is_clobber else "preserve existing"
    print(f"# bytedtos PUT tos://{TOS_BUCKET}/{key} ({strategy})", flush=True)
    if not args.execute:
        return
    expected_hash = _sha256(asset)
    _close_response(_sdk_request(client, "PUT", key, asset, is_clobber))
    response = _sdk_request(client, "GET", key, asset)
    if _remote_sha256(response, asset.stat().st_size) != expected_hash:
        raise ValueError(f"uploaded object checksum mismatch: {asset.name}")
    print(f"# verified tos://{TOS_BUCKET}/{key}", flush=True)


def _publish(args: argparse.Namespace, assets: list[Path], installer: Path) -> None:
    client = _sdk_client(args) if args.execute else None
    try:
        for asset in assets:
            _upload_verified(args, client, asset, f"{TOS_KEY_PREFIX}/{args.version}/{asset.name}")
        _upload_verified(args, client, installer, f"{TOS_KEY_PREFIX}/install.sh")
    finally:
        if client is not None:
            try:
                client.session.close()
            except Exception as error:
                raise RuntimeError(_sdk_error("client close", error)) from None


def run(args: argparse.Namespace) -> None:
    """Prepare dedicated archives and preview or execute uploads from args.

    Args specifies version, source/output directories, HTTPS URL and targets.
    Uploads use bytedtos with the bucket AccessKey in BYTE_TOSTEAM_ACCESS_KEY.
    Returns None. Invalid artifacts/configuration, SDK/I/O failures and timeouts
    raise SystemExit; credentials are never printed or passed to subprocesses.
    """
    try:
        assets, installer = _prepare(args)
        _publish(args, assets, installer)
        status = "published and verified" if args.execute else "prepared; upload preview only"
        print(f"# {status} {TOS_PREFIX}/{args.version}/")
    except (
        OSError, ValueError, RuntimeError, tarfile.TarError, zipfile.BadZipFile,
    ) as error:
        raise SystemExit(f"internal TOS release failed: {error}") from error
