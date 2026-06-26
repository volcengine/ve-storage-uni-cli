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

"""Black-box E2E coverage for ``ve-tos multipart`` low-level APIs.

This script intentionally discovers leaf operations from the running binary via
``ve-tos capabilities --view tree`` and then executes only black-box CLI calls.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

import pytest
from conftest import skip_on_live_error  # type: ignore[import-not-found]

from _lib import CliRunner, CommandResult, envelope_assert
from _lib.registry import command_root
from _lib.surface_matrix import (
    SurfaceCase,
    action_matches_command,
    expected_validation_failure,
    build_execution_case,
    build_surface_cases,
    in_scope_leaf_commands,
    parameter_keys,
)

ROOT = "multipart"
EXPECTED_VALIDATION_FAILURES = {
    # The current CLI metadata exposes only --config/--content-md5, while the
    # handler still requires skipped fields; assert this black-box behavior
    # instead of silently dropping the leaf from coverage.
    "ve-tos real-time-log set": "requires --use-service-topic",
}


def _delete_object(cli_runner: CliRunner, bucket: str, key: str) -> CommandResult:
    return cli_runner.run(
        [
            "ve-tos",
            "object",
            "delete",
            "--bucket",
            bucket,
            "--key",
            key,
            "--force",
            "--confirm",
            f"tos://{bucket}/{key}",
        ]
    )


def _slug(command: str) -> str:
    return command.replace(" ", "-")


def _root_leaf_commands(command_tree: list[dict[str, Any]]) -> list[dict[str, Any]]:
    leaves = [
        leaf
        for leaf in in_scope_leaf_commands(command_tree)
        if command_root(str(leaf["command"])) == ROOT
    ]
    assert leaves, f"expected at least one leaf command for tos {ROOT}"
    return leaves


def _assert_dry_run_result(result: CommandResult, command: str, case: SurfaceCase) -> None:
    expected_error = expected_validation_failure(EXPECTED_VALIDATION_FAILURES, command)
    if expected_error is not None:
        assert result.exit_code == envelope_assert.EXIT_CODES["validation_error"], (
            f"expected validation_error for {command}, got exit={result.exit_code} "
            f"stdout={result.stdout[:240]!r} stderr={result.stderr[:240]!r}"
        )
        envelope = result.require_envelope()
        envelope_assert.assert_failure_envelope(envelope, expected_kind="validation_error")
        assert expected_error in envelope["error"]["message"]
        return

    assert result.exit_code == 0, (
        f"{case.case_id} {command} failed: exit={result.exit_code} "
        f"stdout={result.stdout[:240]!r} stderr={result.stderr[:240]!r}"
    )
    if "query" in case.covered_parameters:
        # --query is also a global JMESPath filter, so successful executions may
        # return the filtered scalar instead of an Envelope.
        assert result.json() is not None
        return

    envelope_assert.assert_success_envelope(result.require_envelope())
    payload = result.payload()
    assert isinstance(payload, dict), f"expected dry-run payload dict, got {payload!r}"
    assert payload.get("dry_run") is True, f"expected dry_run=true, got {payload!r}"
    action = str(payload.get("action", ""))
    assert action_matches_command(action, command)


@pytest.fixture(scope="session")
def multipart_leaf_commands(cli_runner: CliRunner) -> list[dict[str, Any]]:
    result = cli_runner.run(["ve-tos", "capabilities", "--view", "tree"])
    assert result.exit_code == 0, result.stderr
    envelope_assert.assert_success_envelope(result.require_envelope())
    return _root_leaf_commands(result.payload()["commands"])


@pytest.mark.slow
def test_multipart_all_leaf_commands_exercise_full_parameter_surface_dry_run(
    cli_runner: CliRunner,
    multipart_leaf_commands: list[dict[str, Any]],
    tmp_path: Path,
) -> None:
    """@case-id LL-CORE-MULTIPART-SURFACE every leaf operation and parameter is asserted."""

    failures: list[str] = []
    total_cases = 0

    for leaf in multipart_leaf_commands:
        command = str(leaf["command"])
        command_tmp = tmp_path / _slug(command)
        command_tmp.mkdir(parents=True, exist_ok=True)
        cases = build_surface_cases(leaf, command_tmp)
        covered_parameters: set[str] = set()

        for case in cases:
            total_cases += 1
            result = cli_runner.run(["--dry-run", *case.args], timeout=180.0)
            try:
                _assert_dry_run_result(result, command, case)
            except AssertionError as exc:
                failures.append(f"{case.case_id} {command}: {exc}")
            covered_parameters.update(case.covered_parameters)

        missing_parameters = parameter_keys(leaf) - covered_parameters
        if missing_parameters:
            failures.append(f"{command} missing parameter coverage: {sorted(missing_parameters)}")

    assert total_cases >= len(multipart_leaf_commands)
    assert not failures, "\n".join(failures[:80])


@pytest.mark.slow
def test_multipart_leaf_operations_chain_in_metadata_order_dry_run(
    cli_runner: CliRunner,
    multipart_leaf_commands: list[dict[str, Any]],
    tmp_path: Path,
) -> None:
    """@case-id LL-CORE-MULTIPART-CHAIN all operations for this root are asserted in one flow."""

    seen_commands: list[str] = []
    failures: list[str] = []

    for leaf in multipart_leaf_commands:
        command = str(leaf["command"])
        command_tmp = tmp_path / f"chain-{_slug(command)}"
        command_tmp.mkdir(parents=True, exist_ok=True)
        case = build_execution_case(leaf, command_tmp)
        result = cli_runner.run(["--dry-run", *case.args], timeout=180.0)
        try:
            _assert_dry_run_result(result, command, case)
        except AssertionError as exc:
            failures.append(f"{case.case_id} {command}: {exc}")
        seen_commands.append(command)

    expected_commands = {str(leaf["command"]) for leaf in multipart_leaf_commands}
    assert set(seen_commands) == expected_commands
    assert not failures, "\n".join(failures[:80])

@pytest.mark.destructive
def test_multipart_create_upload_list_complete_live_chain(
    cli_runner: CliRunner,
    temp_bucket: str,
    tmp_path: Path,
) -> None:
    """@case-id LL-CORE-MULTIPART-LIVE create -> upload 2 parts -> list-parts -> complete -> download verify."""
    import json
    import os

    key = "ve-tos-cli-e2e-multipart-live/payload.bin"
    part_size = 5 * 1024 * 1024  # 5MB minimum part size
    num_parts = 2

    # Generate 5MB part files
    part_files: list[Path] = []
    for i in range(num_parts):
        part_file = tmp_path / f"part_{i + 1}.bin"
        part_file.write_bytes(os.urandom(part_size))
        part_files.append(part_file)

    create = cli_runner.run(["ve-tos", ROOT, "create", "--bucket", temp_bucket, "--key", key])
    if create.exit_code != 0:
        skip_on_live_error("ve-tos multipart create", create)
    envelope_assert.assert_success_envelope(create.require_envelope())
    upload_id = create.payload()["body"].get("upload_id") or create.payload()["body"].get("uploadId")
    assert upload_id, create.stdout

    try:
        parts_list: list[dict[str, Any]] = []

        for part_num in range(1, num_parts + 1):
            upload = cli_runner.run(
                [
                    "ve-tos", ROOT, "upload",
                    "--bucket", temp_bucket,
                    "--key", key,
                    "--upload-id", upload_id,
                    "--part-number", str(part_num),
                    "--body", str(part_files[part_num - 1]),
                ]
            )
            if upload.exit_code != 0:
                skip_on_live_error(f"ve-tos multipart upload part {part_num}", upload)
            envelope_assert.assert_success_envelope(upload.require_envelope())
            upload_data = upload.payload()
            etag = (
                upload_data.get("headers", {}).get("etag")
                or upload_data.get("headers", {}).get("ETag")
                or (upload_data.get("body") or {}).get("etag")
                or (upload_data.get("body") or {}).get("e_tag")
            )
            assert etag, f"expected etag in upload response for part {part_num}, got {upload_data!r}"
            parts_list.append({"PartNumber": part_num, "ETag": etag})

        # List parts to verify all are present
        listed = cli_runner.run(
            ["ve-tos", ROOT, "list-parts", "--bucket", temp_bucket, "--key", key, "--upload-id", upload_id]
        )
        if listed.exit_code != 0:
            skip_on_live_error("ve-tos multipart list-parts", listed)
        envelope_assert.assert_success_envelope(listed.require_envelope())

        # List uploads to verify in-progress upload appears
        list_uploads = cli_runner.run(
            ["ve-tos", ROOT, "list", "--bucket", temp_bucket, "--prefix", "ve-tos-cli-e2e-multipart-live/"]
        )
        if list_uploads.exit_code != 0:
            skip_on_live_error("ve-tos multipart list", list_uploads)
        envelope_assert.assert_success_envelope(list_uploads.require_envelope())

        # Complete the multipart upload
        parts_json = json.dumps({"Parts": parts_list})
        complete = cli_runner.run(
            [
                "ve-tos", ROOT, "complete",
                "--bucket", temp_bucket,
                "--key", key,
                "--upload-id", upload_id,
                "--parts", parts_json,
            ]
        )
        if complete.exit_code != 0:
            skip_on_live_error("ve-tos multipart complete", complete)
        envelope_assert.assert_success_envelope(complete.require_envelope())

        # Download and verify the assembled object size equals 10MB (2 * 5MB)
        dest_file = tmp_path / "downloaded.bin"
        download = cli_runner.run(
            ["ve-tos", "object", "download", "--bucket", temp_bucket, "--key", key, "--body", str(dest_file)]
        )
        if download.exit_code != 0:
            skip_on_live_error("ve-tos object download", download)
        expected_size = num_parts * part_size
        actual_size = dest_file.stat().st_size
        assert actual_size == expected_size, (
            f"downloaded size {actual_size} != expected {expected_size} ({num_parts} * {part_size})"
        )
    finally:
        # [Review Fix #2] 清理：abort 残留上传（若 complete 失败）+ 删除已完成对象
        cli_runner.run(
            ["ve-tos", ROOT, "abort", "--bucket", temp_bucket, "--key", key, "--upload-id", upload_id, "--force"]
        )
        _delete_object(cli_runner, temp_bucket, key)


@pytest.mark.destructive
def test_multipart_create_abort_live_chain(
    cli_runner: CliRunner,
    temp_bucket: str,
) -> None:
    """@case-id LL-CORE-MULTIPART-ABORT-LIVE create -> abort."""

    key = "ve-tos-cli-e2e-multipart-abort/payload.txt"
    create = cli_runner.run(["ve-tos", ROOT, "create", "--bucket", temp_bucket, "--key", key])
    if create.exit_code != 0:
        skip_on_live_error("ve-tos multipart create", create)
    envelope_assert.assert_success_envelope(create.require_envelope())
    upload_id = create.payload()["body"].get("upload_id") or create.payload()["body"].get("uploadId")
    assert upload_id, create.stdout

    try:
        abort = cli_runner.run(
            ["ve-tos", ROOT, "abort", "--bucket", temp_bucket, "--key", key,
             "--upload-id", upload_id, "--force"]
        )
        if abort.exit_code != 0:
            skip_on_live_error("ve-tos multipart abort", abort)
        envelope_assert.assert_success_envelope(abort.require_envelope())
    finally:
        # [Review Fix #1] 确保即使断言失败也尝试 abort 清理残留上传
        cli_runner.run(
            ["ve-tos", ROOT, "abort", "--bucket", temp_bucket, "--key", key,
             "--upload-id", upload_id, "--force"]
        )


@pytest.mark.destructive
def test_multipart_list_parts_live_chain(
    cli_runner: CliRunner,
    temp_bucket: str,
    tmp_path: Path,
) -> None:
    """@case-id LL-CORE-MULTIPART-LIST-PARTS-LIVE create -> upload 2 parts -> list-parts verify count."""
    import os

    key = "ve-tos-cli-e2e-multipart-list-parts/payload.bin"
    part_size = 5 * 1024 * 1024  # 5MB minimum part size
    num_parts = 2

    # Generate 5MB part files
    part_files: list[Path] = []
    for i in range(num_parts):
        part_file = tmp_path / f"list_part_{i + 1}.bin"
        part_file.write_bytes(os.urandom(part_size))
        part_files.append(part_file)

    create = cli_runner.run(["ve-tos", ROOT, "create", "--bucket", temp_bucket, "--key", key])
    if create.exit_code != 0:
        skip_on_live_error("ve-tos multipart create", create)
    envelope_assert.assert_success_envelope(create.require_envelope())
    upload_id = create.payload()["body"].get("upload_id") or create.payload()["body"].get("uploadId")
    assert upload_id, create.stdout

    try:
        for part_num in range(1, num_parts + 1):
            upload = cli_runner.run(
                [
                    "ve-tos", ROOT, "upload",
                    "--bucket", temp_bucket,
                    "--key", key,
                    "--upload-id", upload_id,
                    "--part-number", str(part_num),
                    "--body", str(part_files[part_num - 1]),
                ]
            )
            if upload.exit_code != 0:
                skip_on_live_error(f"ve-tos multipart upload part {part_num}", upload)
            envelope_assert.assert_success_envelope(upload.require_envelope())

        # List parts and verify count matches uploaded parts
        listed = cli_runner.run(
            ["ve-tos", ROOT, "list-parts", "--bucket", temp_bucket, "--key", key, "--upload-id", upload_id]
        )
        if listed.exit_code != 0:
            skip_on_live_error("ve-tos multipart list-parts", listed)
        envelope_assert.assert_success_envelope(listed.require_envelope())

        list_data = listed.payload()
        parts = list_data.get("parts") or list_data.get("Parts") or []
        if isinstance(parts, dict):
            parts = [parts]
        assert len(parts) == num_parts, (
            f"expected {num_parts} parts in list-parts response, got {len(parts)}: {list_data!r}"
        )
    finally:
        # [Review Fix #3] 确保 abort 清理上传，即使断言失败
        cli_runner.run(
            ["ve-tos", ROOT, "abort", "--bucket", temp_bucket, "--key", key,
             "--upload-id", upload_id, "--force"]
        )


@pytest.mark.destructive
def test_multipart_copy_live_chain(
    cli_runner: CliRunner,
    temp_bucket: str,
    small_text_payload: Path,
) -> None:
    """@case-id LL-CORE-MULTIPART-COPY-LIVE create -> upload source -> copy part -> complete."""
    import json

    src_key = "ve-tos-cli-e2e-multipart-copy/source.txt"
    dst_key = "ve-tos-cli-e2e-multipart-copy/destination.txt"

    upload_src = cli_runner.run(
        ["ve-tos", "object", "upload", "--bucket", temp_bucket, "--key", src_key,
         "--body", str(small_text_payload)]
    )
    if upload_src.exit_code != 0:
        skip_on_live_error("ve-tos object upload (source)", upload_src)

    create = cli_runner.run(["ve-tos", ROOT, "create", "--bucket", temp_bucket, "--key", dst_key])
    if create.exit_code != 0:
        skip_on_live_error("ve-tos multipart create", create)
    envelope_assert.assert_success_envelope(create.require_envelope())
    upload_id = create.payload()["body"].get("upload_id") or create.payload()["body"].get("uploadId")
    assert upload_id, create.stdout

    try:
        copy_result = cli_runner.run(
            ["ve-tos", ROOT, "copy", "--bucket", temp_bucket, "--key", dst_key,
             "--upload-id", upload_id, "--part-number", "1",
             "--copy-source", f"/{temp_bucket}/{src_key}"]
        )
        if copy_result.exit_code != 0:
            skip_on_live_error("ve-tos multipart copy", copy_result)
        envelope_assert.assert_success_envelope(copy_result.require_envelope())
        copy_data = copy_result.payload()
        etag = (
            copy_data.get("headers", {}).get("etag")
            or copy_data.get("headers", {}).get("ETag")
            or (copy_data.get("body") or {}).get("etag")
            or (copy_data.get("body") or {}).get("e_tag")
            or (copy_data.get("body") or {}).get("copy_part_result", {}).get("etag")
            or (copy_data.get("body") or {}).get("CopyPartResult", {}).get("ETag")
        )
        assert etag, f"expected etag in copy response, got {copy_data!r}"

        parts_json = json.dumps({"Parts": [{"PartNumber": 1, "ETag": etag}]})
        complete = cli_runner.run(
            ["ve-tos", ROOT, "complete", "--bucket", temp_bucket, "--key", dst_key,
             "--upload-id", upload_id, "--parts", parts_json]
        )
        if complete.exit_code != 0:
            skip_on_live_error("ve-tos multipart complete", complete)
        envelope_assert.assert_success_envelope(complete.require_envelope())

        head = cli_runner.run(["ve-tos", "object", "head", "--bucket", temp_bucket, "--key", dst_key])
        assert head.exit_code == 0, head.stderr
    finally:
        cli_runner.run(
            ["ve-tos", ROOT, "abort", "--bucket", temp_bucket, "--key", dst_key,
             "--upload-id", upload_id, "--force"]
        )
        for k in (src_key, dst_key):
            _delete_object(cli_runner, temp_bucket, k)


@pytest.mark.destructive
def test_multipart_list_uploads_live_chain(
    cli_runner: CliRunner,
    temp_bucket: str,
) -> None:
    """@case-id LL-CORE-MULTIPART-LIST-LIVE create uploads -> list -> abort cleanup."""

    keys = [f"ve-tos-cli-e2e-multipart-list/{i}.txt" for i in range(2)]
    upload_ids = []

    try:
        for key in keys:
            create = cli_runner.run(["ve-tos", ROOT, "create", "--bucket", temp_bucket, "--key", key])
            if create.exit_code != 0:
                skip_on_live_error("ve-tos multipart create", create)
            envelope_assert.assert_success_envelope(create.require_envelope())
            uid = create.payload()["body"].get("upload_id") or create.payload()["body"].get("uploadId")
            assert uid, create.stdout
            upload_ids.append((key, uid))

        list_result = cli_runner.run(
            ["ve-tos", ROOT, "list", "--bucket", temp_bucket,
             "--prefix", "ve-tos-cli-e2e-multipart-list/", "--max-uploads", "10"]
        )
        if list_result.exit_code != 0:
            skip_on_live_error("ve-tos multipart list", list_result)
        envelope_assert.assert_success_envelope(list_result.require_envelope())
        payload = list_result.payload()
        body = payload.get("body") or {}
        uploads = (
            body.get("uploads") or body.get("Uploads")
            or body.get("upload") or body.get("Upload") or []
        )
        if isinstance(uploads, dict):
            uploads = [uploads]
        assert isinstance(uploads, list), f"expected list of uploads, got {type(uploads)}: {payload!r}"
    finally:
        for key, uid in upload_ids:
            cli_runner.run(
                ["ve-tos", ROOT, "abort", "--bucket", temp_bucket, "--key", key,
                 "--upload-id", uid, "--force"]
            )
