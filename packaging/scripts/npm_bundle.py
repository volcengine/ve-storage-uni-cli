"""Build per-CLI npm payloads from the five supported release archives."""

from __future__ import annotations

import hashlib
import re
import shutil
import stat
import tarfile
import tempfile
import zipfile
from pathlib import Path


NPM_TARGETS = (
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-unknown-linux-gnu.2.17",
    "aarch64-unknown-linux-gnu.2.17",
    "x86_64-pc-windows-msvc",
)
MAX_BINARY_BYTES = 512 * 1024 * 1024


def archive_name(target: str) -> str:
    """Return the release filename for a supported target; perform no I/O."""
    suffix = ".zip" if "windows" in target else ".tar.gz"
    return f"ve-storage-uni-cli-{target}{suffix}"


def sha256_file(path: Path) -> str:
    """Hash a file with bounded memory; propagate filesystem errors."""
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def read_checksums(checksum_path: Path) -> dict[str, str]:
    """Read optional checksums; reject malformed or duplicate entries."""
    try:
        lines = checksum_path.read_text(encoding="utf-8").splitlines()
    except FileNotFoundError:
        return {}
    checksums = {}
    for line in lines:
        match = re.fullmatch(r"([0-9a-fA-F]{64}) [ *](\S+)", line)
        if match is None:
            raise ValueError("invalid SHA256SUMS entry")
        digest, filename = match.groups()
        if filename in checksums:
            raise ValueError(f"duplicate SHA256SUMS entry: {filename}")
        checksums[filename] = digest.lower()
    return checksums


def write_checksums(checksum_path: Path, checksums: dict[str, str]) -> None:
    """Atomically publish completed local hashes; propagate filesystem errors."""
    temporary = tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", delete=False,
                                          dir=checksum_path.parent, prefix=".SHA256SUMS-")
    temporary_path = Path(temporary.name)
    try:
        with temporary as output:
            for filename, digest in sorted(checksums.items()):
                output.write(f"{digest}  {filename}\n")
        temporary_path.replace(checksum_path)
    finally:
        temporary_path.unlink(missing_ok=True)


def validate_archives(archive_dir: Path) -> dict[str, str]:
    """Verify all five archives and complete SHA256SUMS without manual setup.

    Existing entries must match. Missing hashes are calculated from local build
    outputs and written only after every required archive has been validated.
    Missing files, checksum mismatches or I/O failures abort package generation.
    """
    checksum_path = archive_dir / "SHA256SUMS"
    checksums = read_checksums(checksum_path)
    completed = dict(checksums)
    selected = {}
    for target in NPM_TARGETS:
        filename = archive_name(target)
        digest = sha256_file(archive_dir / filename)
        if filename in checksums and digest != checksums[filename]:
            raise ValueError(f"checksum mismatch for {filename}")
        completed[filename] = digest
        selected[target] = digest
    # [Review Fix #NpmChecksums] Fill absent hashes without overwriting expected ones.
    if completed != checksums:
        write_checksums(checksum_path, completed)
    return selected


def copy_archive_binary(archive_path: Path, member_name: str, destination: Path) -> None:
    """Copy one regular binary without extracting archive paths or links.

    Reject missing, duplicate, linked, empty or oversized entries with ValueError.
    Filesystem and invalid archive errors propagate to the release command.
    """
    if archive_path.suffix == ".zip":
        copy_zip_binary(archive_path, member_name, destination)
    else:
        copy_tar_binary(archive_path, member_name, destination)
    destination.chmod(0o755)


def copy_tar_binary(archive_path: Path, member_name: str, destination: Path) -> None:
    """Copy exactly one regular tar entry; never materialize archive paths."""
    with tarfile.open(archive_path, "r:gz") as archive:
        members = [member for member in archive if member.name == member_name]
        if len(members) != 1 or not members[0].isreg():
            raise ValueError(f"expected one regular {member_name} in {archive_path.name}")
        member = members[0]
        if not 0 < member.size <= MAX_BINARY_BYTES:
            raise ValueError(f"invalid binary size for {member_name}")
        with archive.extractfile(member) as source, destination.open("wb") as output:
            shutil.copyfileobj(source, output)


def copy_zip_binary(archive_path: Path, member_name: str, destination: Path) -> None:
    """Copy exactly one non-link zip entry, ignoring every other archive path."""
    with zipfile.ZipFile(archive_path) as archive:
        members = [member for member in archive.infolist() if member.filename == member_name]
        if len(members) != 1:
            raise ValueError(f"expected one regular {member_name} in {archive_path.name}")
        member = members[0]
        file_mode = member.external_attr >> 16
        if member.is_dir() or stat.S_IFMT(file_mode) not in (0, stat.S_IFREG):
            raise ValueError(f"expected a regular {member_name}")
        if not 0 < member.file_size <= MAX_BINARY_BYTES:
            raise ValueError(f"invalid binary size for {member_name}")
        with archive.open(member) as source, destination.open("wb") as output:
            shutil.copyfileobj(source, output)


def bundle_binaries(package_dir: Path, commands: list[str], archive_dir: Path,
                    checksums: dict[str, str]) -> dict[str, dict[str, object]]:
    """Bundle only selected commands; return pinned archive and binary hashes.

    Inputs must reference validated release archives. Invalid members or I/O
    errors abort staging; callers must not publish a partially staged package.
    """
    targets = {}
    for target in NPM_TARGETS:
        binaries = {}
        for command in commands:
            filename = f"{command}.exe" if "windows" in target else command
            destination = package_dir / "vendor" / target / "bin" / filename
            destination.parent.mkdir(parents=True, exist_ok=True)
            copy_archive_binary(archive_dir / archive_name(target), f"bin/{filename}", destination)
            binaries[command] = sha256_file(destination)
        targets[target] = {
            "archive": archive_name(target), "archiveSha256": checksums[target],
            "binaries": binaries,
        }
    return targets
