"""Opt-in real-service protocol readbacks; each test owns a temporary bucket.

Run with TOS_E2E_PROTOCOL_LIVE=1 and the standard TOS E2E credentials.
Failures are assertions, never converted to successful capability skips.
"""
from __future__ import annotations

import os
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import unquote

import pytest

from _lib import CliRunner

pytestmark = [
    pytest.mark.live,
    pytest.mark.destructive,
    pytest.mark.skipif(
        os.environ.get("TOS_E2E_PROTOCOL_LIVE") != "1",
        reason="requires explicit TOS_E2E_PROTOCOL_LIVE=1 and a test account",
    ),
]


@pytest.fixture
def cli_runner(tos_credentials, tmp_path: Path) -> CliRunner:
    """Use an explicit build under test without changing the shared runner."""
    binary = os.environ.get("TOS_E2E_PROTOCOL_BIN")
    return CliRunner(creds=tos_credentials,
                     binary=Path(binary) if binary else None,
                     home_override=tmp_path)


def execute(runner: CliRunner, bucket: str, action: str, *flags: str) -> dict:
    """Execute an object operation and require a successful envelope."""
    result = runner.run([
        "ve-tos", "object", action, "--bucket", bucket,
        "--key", "protocol/readback.txt", *flags,
    ])
    assert result.exit_code == 0, result.stderr or result.stdout
    envelope = result.require_envelope()
    assert envelope["success"] is True, envelope
    return envelope["data"]


def upload(runner: CliRunner, bucket: str, directory: Path) -> dict:
    """Seed known content, content type and a pre-existing metadata key."""
    payload = directory / "protocol.txt"
    payload.write_text("protocol readback\n", encoding="utf-8")
    return execute(runner, bucket, "upload", "--body", str(payload),
            "--content-type", "text/plain", "--meta", "old=keep")


def headers(runner: CliRunner, bucket: str) -> dict[str, str]:
    """Return normalized raw HEAD headers, decoding metadata exactly once."""
    raw = execute(runner, bucket, "head")["headers"]
    return {name.lower(): unquote(value) if "meta-" in name.lower() else value
            for name, value in raw.items()}


def test_metadata_merge_replace_and_content_type_readback(
    cli_runner: CliRunner, temp_bucket: str, tmp_path: Path,
) -> None:
    """Merge preserves old keys/type; explicit replacement removes old keys."""
    upload(cli_runner, temp_bucket, tmp_path)
    execute(cli_runner, temp_bucket, "set-meta", "--meta", "note=%20&rate=50%")
    actual = headers(cli_runner, temp_bucket)
    assert actual["content-type"] == "text/plain"
    assert actual["x-tos-meta-old"] == "keep"
    assert actual["x-tos-meta-note"] == "%20"
    assert actual["x-tos-meta-rate"] == "50%"
    execute(cli_runner, temp_bucket, "set-meta", "--meta", "owner=test",
            "--metadata-directive", "REPLACE", "--content-type", "text/csv")
    actual = headers(cli_runner, temp_bucket)
    assert actual["content-type"] == "text/csv"
    assert actual["x-tos-meta-owner"] == "test"
    assert "x-tos-meta-old" not in actual
    assert "x-tos-meta-note" not in actual


def test_tagging_kv_round_trip(
    cli_runner: CliRunner, temp_bucket: str, tmp_path: Path,
) -> None:
    """Read tags from the service rather than accepting a write status alone."""
    upload(cli_runner, temp_bucket, tmp_path)
    execute(cli_runner, temp_bucket, "set-tagging", "--tags", "owner=test&env=qa")
    response = execute(cli_runner, temp_bucket, "get-tagging")["body"]
    # [Review Fix #ProtocolReadback] CLI responses normalize service JSON keys to snake_case.
    assert {tag["key"]: tag["value"] for tag in response["tag_set"]["tags"]} == {
        "owner": "test", "env": "qa",
    }


def test_hns_timestamp_readback(
    cli_runner: CliRunner, hns_temp_bucket: str, tmp_path: Path,
) -> None:
    """SetObjectTime requires a service-enabled HNS test bucket."""
    from email.utils import parsedate_to_datetime

    upload(cli_runner, hns_temp_bucket, tmp_path)
    execute(cli_runner, hns_temp_bucket, "set-time", "--time", "2025-01-01T00:00:00Z")
    actual = parsedate_to_datetime(headers(cli_runner, hns_temp_bucket)["last-modified"])
    assert actual == datetime(2025, 1, 1, tzinfo=timezone.utc)


def test_expiration_readback(
    cli_runner: CliRunner, temp_bucket: str, tmp_path: Path,
) -> None:
    """Read expiry-date from the public expiration header, then clear the TTL."""
    import re
    from email.utils import parsedate_to_datetime

    upload(cli_runner, temp_bucket, tmp_path)
    created = parsedate_to_datetime(headers(cli_runner, temp_bucket)["last-modified"])
    execute(cli_runner, temp_bucket, "set-expires", "--expires", "7")
    actual = headers(cli_runner, temp_bucket)
    # [Review Fix #ProtocolExpiresDays] Public HEAD exposes a lifecycle date, not raw TTL.
    expiry = re.search(r'expiry-date="([^"]+)"', actual["x-tos-expiration"])
    assert expiry, actual
    expiry_date = parsedate_to_datetime(expiry.group(1))
    # The service rounds to the next local-calendar midnight; allow timezone/DST boundaries.
    assert 6 * 86400 < (expiry_date - created).total_seconds() < 9 * 86400
    execute(cli_runner, temp_bucket, "set-expires", "--expires", "0")
    assert "x-tos-expiration" not in headers(cli_runner, temp_bucket)


def test_fetch_metadata_and_storage_class_readback(
    cli_runner: CliRunner, temp_bucket: str,
) -> None:
    """Use an explicitly supplied small, public source accessible by TOS."""
    source_url = os.environ.get("TOS_E2E_PROTOCOL_FETCH_URL")
    if not source_url:
        pytest.skip("set TOS_E2E_PROTOCOL_FETCH_URL to enable remote fetch readback")
    execute(cli_runner, temp_bucket, "fetch", "--source-url", source_url,
            "--meta", "owner=fetch-test", "--storage-class", "IA")
    actual = headers(cli_runner, temp_bucket)
    assert actual["x-tos-meta-owner"] == "fetch-test"
    assert actual["x-tos-storage-class"] == "IA"




def version_headers(runner: CliRunner, bucket: str, version: str) -> dict:
    """Allow bounded visibility delay for a freshly written version, never other errors."""
    import time

    for _attempt in range(15):
        result = runner.run([
            "ve-tos", "object", "head", "--bucket", bucket,
            "--key", "protocol/readback.txt", "--version-id", version,
        ])
        if result.exit_code == 0:
            return result.require_envelope()["data"]["headers"]
        assert result.exit_code == 4, result.stderr or result.stdout
        time.sleep(1)
    raise AssertionError(result.stderr or result.stdout)


def wait_for_versioning(runner: CliRunner, bucket: str) -> None:
    """Require the new bucket configuration to be observable before version writes."""
    import time

    consecutive = 0
    for _attempt in range(30):
        result = runner.run(["ve-tos", "versioning", "get", "--bucket", bucket])
        assert result.exit_code == 0, result.stderr or result.stdout
        state = result.require_envelope()["data"]["body"]
        consecutive = consecutive + 1 if state.get("status") == "Enabled" else 0
        if consecutive == 3:
            return
        time.sleep(1)
    raise AssertionError(f"versioning configuration did not become visible: {state}")

def upload_version(runner: CliRunner, bucket: str, directory: Path,
                   previous: str | None = None) -> str:
    """Wait for new-bucket versioning propagation, always checking stored versions."""
    import time

    # [Review Fix #ProtocolVersionReadback] Enabling versioning may propagate after the write response.
    for _attempt in range(30):
        result = upload(runner, bucket, directory)
        version = result["headers"].get("x-tos-version-id")
        if version and version != "null" and version != previous:
            return version
        time.sleep(1)
    raise AssertionError("temporary bucket did not create distinct versions within 30 attempts")

def test_versioned_metadata_preserves_current_version(
    cli_runner: CliRunner, temp_bucket: str, tmp_path: Path,
) -> None:
    """Mutate the first version and independently read both stored versions."""
    setting = cli_runner.run([
        "ve-tos", "versioning", "set", "--bucket", temp_bucket,
        "--config", '{"Status":"Enabled"}',
    ])
    assert setting.exit_code == 0, setting.stderr or setting.stdout
    try:
        wait_for_versioning(cli_runner, temp_bucket)
        first = upload_version(cli_runner, temp_bucket, tmp_path)
        current = upload_version(cli_runner, temp_bucket, tmp_path, first)
        assert first != current
        version_headers(cli_runner, temp_bucket, first)
        version_headers(cli_runner, temp_bucket, current)
        execute(cli_runner, temp_bucket, "set-meta", "--meta", "owner=old-version",
                "--version-id", first)
        old = version_headers(cli_runner, temp_bucket, first)
        assert old["x-tos-meta-owner"] == "old-version"
        assert "x-tos-meta-owner" not in headers(cli_runner, temp_bucket)
        # [Review Fix #ProtocolVersionReadback] Ordinary HEAD need not echo a version ID.
        current_head = version_headers(cli_runner, temp_bucket, current)
        assert "x-tos-meta-owner" not in current_head
    finally:
        # [Review Fix #ProtocolCleanup] Versioned test buckets need every version purged.
        cleanup = cli_runner.run([
            "ve-tos", "rm", f"tos://{temp_bucket}/", "--recursive", "--all-versions",
            "--force", "--confirm", f"tos://{temp_bucket}/",
        ])
        assert cleanup.exit_code == 0, cleanup.stderr or cleanup.stdout
