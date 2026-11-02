"""Live share lifecycle in an explicitly supplied disposable ADrive instance/space.

No instance/space is created. Cleanup is restricted to this test's unique root and
share IDs. ADRIVE_E2E_BINARY can select either CLI entrypoint.
"""
from __future__ import annotations

import hashlib
import os
from pathlib import Path

import pytest

from _lib import ADriveCredentials, CliRunner, unique_suffix


@pytest.fixture()
def share_environment(adrive_credentials: ADriveCredentials, tmp_path: Path):
    instance = os.environ.get("ADRIVE_E2E_INSTANCE")
    space = os.environ.get("ADRIVE_E2E_SPACE")
    if not instance or not space:
        pytest.skip("live share test needs dedicated ADRIVE_E2E_INSTANCE and ADRIVE_E2E_SPACE")
    binary = os.environ.get("ADRIVE_E2E_BINARY")
    runner = CliRunner(adrive_credentials, binary=Path(binary) if binary else None,
                       home_override=tmp_path / "isolated-home")
    prefix = [] if binary and Path(binary).name == "ve-adrive-cli" else ["ve-adrive"]
    return runner, prefix, instance, space


def checked(runner, arguments, password=None):
    result = runner.run(arguments, stdin=password, timeout=300)
    assert result.exit_code == 0, f"share live command failed: exit={result.exit_code}"
    envelope = result.require_envelope()
    assert envelope["success"] is True
    return result.payload()


def receiver_arguments(prefix, action, share_url):
    arguments = [*prefix, "share", action, share_url]
    endpoint = os.environ.get("ADRIVE_E2E_RECEIVER_ENDPOINT")
    if endpoint:
        arguments += ["--endpoint", endpoint]
    return arguments


def enumerate_pages(runner, prefix, share_url, root, password):
    marker = None
    markers = set()
    found = set()
    for _ in range(100):
        arguments = [*receiver_arguments(prefix, "files", share_url), "--path", root + "/",
                     "--limit", "1", "--password-stdin"]
        if marker:
            arguments += ["--marker", marker]
        page = checked(runner, arguments, password)
        found.update(item["FilePath"] for item in page.get("Files", []))
        marker = page.get("NextMarker")
        if not marker:
            assert not page.get("IsTruncated", False)
            return found
        assert marker not in markers, "pagination marker cycle"
        markers.add(marker)
    pytest.fail("live pagination exceeded bounded test budget")


def exercise_share(runner, prefix, resource, local_root, password):
    instance, root, share_id, share_url = resource
    managed = checked(runner, [*prefix, "share", "get", share_id, "--instance", instance])
    creator = os.environ.get("ADRIVE_E2E_CREATOR") or managed.get("CreatorID")
    listing = [*prefix, "share", "list", "--instance", instance, "--name", root]
    if creator:
        listing += ["--creator", creator]
    checked(runner, listing)
    checked(runner, [*prefix, "share", "update", share_id, "--instance", instance,
                     "--description", "share live updated", "--download-limit", "0"])
    inspected = checked(runner, receiver_arguments(prefix, "inspect", share_url))
    assert inspected["ShareID"] == share_id
    assert f"{root}/one.txt" in enumerate_pages(runner, prefix, share_url, root, password)
    destination = local_root / "received"
    checkpoint = local_root / "share-checkpoint.json"
    arguments = [*receiver_arguments(prefix, "download", share_url), str(destination),
                 "--recursive", "--password-stdin", "--checkpoint", str(checkpoint)]
    outcome = checked(runner, arguments, password)
    assert outcome["Complete"] and outcome["Summary"]["Completed"] == 2
    for relative in ("one.txt", "nested/two.txt"):
        original = (local_root / relative).read_bytes()
        received = (destination / root / relative).read_bytes()
        assert hashlib.sha256(original).digest() == hashlib.sha256(received).digest()
    assert (destination / root / "empty").is_dir()
    resumed = checked(runner, [*arguments, "--resume"], password)
    assert resumed["Complete"] and resumed["Summary"]["Completed"] == 2


def cleanup_root_arguments(prefix, target):
    """Return cleanup argv restricted to the exact test root, without executing it."""
    # [Review Fix #Download8] rm is the file/folder command; del manages instances/spaces.
    return [*prefix, "rm", target, "--recursive", "--force", "--confirm", target, "--yes"]


def test_cleanup_root_arguments_parse_offline(tmp_path):
    adrive_credentials = ADriveCredentials(access_key="offline-ak", secret_key="offline-sk",
                                          endpoint="https://ids.example.invalid", region="cn-beijing",
                                          resource_prefix="share-e2e-offline")
    binary = Path(os.environ.get("ADRIVE_E2E_BINARY", Path(__file__).resolve().parents[3] / "target/debug/ve-storage-uni-cli"))
    runner = CliRunner(adrive_credentials, binary=binary, home_override=tmp_path / "offline-home")
    prefix = [] if binary.name == "ve-adrive-cli" else ["ve-adrive"]
    target = "adrive://dedicated-instance/dedicated-space/share-e2e-offline-root/"
    arguments = cleanup_root_arguments(prefix, target)
    assert arguments.count(target) == 2
    assert all(value not in arguments for value in ("adrive://dedicated-instance", "adrive://dedicated-instance/dedicated-space"))
    result = runner.run([*arguments, "--dry-run"])
    assert result.exit_code == 0, result.stderr
    assert result.require_envelope()["success"] is True


def test_share_lifecycle_and_safe_receiver(share_environment, tmp_path: Path):
    runner, prefix, instance, space = share_environment
    root = "share-e2e-" + unique_suffix(12)
    target = f"adrive://{instance}/{space}/{root}/"
    password = ("test-" + unique_suffix(20) + "\n").encode()
    (tmp_path / "nested").mkdir()
    (tmp_path / "one.txt").write_text("share root file\n", encoding="utf-8")
    (tmp_path / "nested/two.txt").write_text("分享嵌套文件\n", encoding="utf-8")
    share_ids = []
    try:
        checked(runner, [*prefix, "mkdir", target + "empty/", "--parents"])
        checked(runner, [*prefix, "mkdir", target + "nested/", "--parents"])
        for relative in ("one.txt", "nested/two.txt"):
            checked(runner, [*prefix, "cp", str(tmp_path / relative), target + relative])
        arguments = [*prefix, "share", "create", target, "--name", root,
                     "--password-stdin", "--expires-in", "1h", "--permission", "download"]
        page_origin = (os.environ.get("ADRIVE_E2E_SHARE_ENDPOINT")
                       or os.environ.get("ADRIVE_E2E_SHARE_BASE_URL"))
        if page_origin:
            arguments += ["--share-endpoint", page_origin]
        created = checked(runner, arguments, password)
        share_ids.append(created["ShareID"])
        exercise_share(runner, prefix, (instance, root, created["ShareID"], created["ShareUrl"]),
                       tmp_path, password)
    finally:
        cleanup_failures = []
        for share_id in share_ids:
            result = runner.run([*prefix, "share", "cancel", share_id,
                                 "--instance", instance, "--yes"])
            if result.exit_code:
                cleanup_failures.append("cancel test share")
        result = runner.run(cleanup_root_arguments(prefix, target), timeout=300)
        if result.exit_code:
            cleanup_failures.append("delete unique test root")
        assert not cleanup_failures, ", ".join(cleanup_failures)
