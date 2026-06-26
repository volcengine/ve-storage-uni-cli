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

"""Black-box E2E coverage for ``ve-tos object`` low-level APIs.

This script intentionally discovers leaf operations from the running binary via
``ve-tos capabilities --view tree`` and then executes only black-box CLI calls.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

import pytest

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

ROOT = "object"
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
            ROOT,
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
def object_leaf_commands(cli_runner: CliRunner) -> list[dict[str, Any]]:
    result = cli_runner.run(["ve-tos", "capabilities", "--view", "tree"])
    assert result.exit_code == 0, result.stderr
    envelope_assert.assert_success_envelope(result.require_envelope())
    return _root_leaf_commands(result.payload()["commands"])


@pytest.mark.slow
def test_object_all_leaf_commands_exercise_full_parameter_surface_dry_run(
    cli_runner: CliRunner,
    object_leaf_commands: list[dict[str, Any]],
    tmp_path: Path,
) -> None:
    """@case-id LL-CORE-OBJECT-SURFACE every leaf operation and parameter is asserted."""

    failures: list[str] = []
    total_cases = 0

    for leaf in object_leaf_commands:
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

    assert total_cases >= len(object_leaf_commands)
    assert not failures, "\n".join(failures[:80])


@pytest.mark.slow
def test_object_leaf_operations_chain_in_metadata_order_dry_run(
    cli_runner: CliRunner,
    object_leaf_commands: list[dict[str, Any]],
    tmp_path: Path,
) -> None:
    """@case-id LL-CORE-OBJECT-CHAIN all operations for this root are asserted in one flow."""

    seen_commands: list[str] = []
    failures: list[str] = []

    for leaf in object_leaf_commands:
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

    expected_commands = {str(leaf["command"]) for leaf in object_leaf_commands}
    assert set(seen_commands) == expected_commands
    assert not failures, "\n".join(failures[:80])

@pytest.mark.destructive
def test_object_upload_head_download_delete_live_chain(
    cli_runner: CliRunner,
    e2e_bucket_name: str,
    small_text_payload: Path,
    tmp_path: Path,
) -> None:
    """@case-id LL-CORE-OBJECT-LIVE upload -> head -> download -> delete."""
    from conftest import skip_on_live_error  # type: ignore[import-not-found]

    key = "ve-tos-cli-e2e-object-live/payload.txt"
    download_path = tmp_path / "downloaded.txt"
    try:
        upload = cli_runner.run(
            [
                "ve-tos", ROOT, "upload",
                "--bucket", e2e_bucket_name,
                "--key", key,
                "--body", str(small_text_payload),
                "--content-type", "text/plain",
                "--storage-class", "STANDARD",
                "--meta", "x-tos-meta-e2e=test-value",
            ]
        )
        if upload.exit_code != 0:
            skip_on_live_error("ve-tos object upload", upload)
        envelope_assert.assert_success_envelope(upload.require_envelope())

        head = cli_runner.run(["ve-tos", ROOT, "head", "--bucket", e2e_bucket_name, "--key", key])
        assert head.exit_code == 0, head.stderr
        envelope_assert.assert_success_envelope(head.require_envelope())

        stat = cli_runner.run(["ve-tos", ROOT, "stat", "--bucket", e2e_bucket_name, "--key", key])
        assert stat.exit_code == 0, stat.stderr
        envelope_assert.assert_success_envelope(stat.require_envelope())

        download = cli_runner.run(
            [
                "ve-tos", ROOT, "download",
                "--bucket", e2e_bucket_name,
                "--key", key,
                "--body", str(download_path),
            ]
        )
        assert download.exit_code == 0, download.stderr
        assert download_path.read_text() == small_text_payload.read_text()

        list_result = cli_runner.run(
            ["ve-tos", ROOT, "list", "--bucket", e2e_bucket_name, "--prefix", "ve-tos-cli-e2e-object-live/"]
        )
        assert list_result.exit_code == 0, list_result.stderr
        envelope_assert.assert_success_envelope(list_result.require_envelope())
    finally:
        cleanup = _delete_object(cli_runner, e2e_bucket_name, key)
        assert cleanup.exit_code in (0, envelope_assert.EXIT_CODES["resource_not_found"])


@pytest.mark.destructive
def test_object_copy_live_chain(
    cli_runner: CliRunner,
    e2e_bucket_name: str,
    small_text_payload: Path,
    tmp_path: Path,
) -> None:
    """@case-id LL-CORE-OBJECT-COPY-LIVE upload -> copy -> head copy -> delete both."""
    from conftest import skip_on_live_error  # type: ignore[import-not-found]

    src_key = "ve-tos-cli-e2e-object-copy/src.txt"
    dst_key = "ve-tos-cli-e2e-object-copy/dst.txt"

    try:
        upload = cli_runner.run(
            ["ve-tos", ROOT, "upload", "--bucket", e2e_bucket_name, "--key", src_key,
             "--body", str(small_text_payload)]
        )
        if upload.exit_code != 0:
            skip_on_live_error("ve-tos object upload", upload)

        copy_result = cli_runner.run(
            ["ve-tos", ROOT, "copy",
             f"tos://{e2e_bucket_name}/{src_key}",
             f"tos://{e2e_bucket_name}/{dst_key}"]
        )
        if copy_result.exit_code != 0:
            skip_on_live_error("ve-tos object copy", copy_result)
        envelope_assert.assert_success_envelope(copy_result.require_envelope())

        head = cli_runner.run(["ve-tos", ROOT, "head", "--bucket", e2e_bucket_name, "--key", dst_key])
        assert head.exit_code == 0, head.stderr
        envelope_assert.assert_success_envelope(head.require_envelope())
    finally:
        for k in (src_key, dst_key):
            _delete_object(cli_runner, e2e_bucket_name, k)


@pytest.mark.destructive
def test_object_tagging_live_chain(
    cli_runner: CliRunner,
    e2e_bucket_name: str,
    small_text_payload: Path,
) -> None:
    """@case-id LL-CORE-OBJECT-TAGGING-LIVE set-tagging -> get-tagging -> delete-tagging."""
    from conftest import skip_on_live_error  # type: ignore[import-not-found]

    key = "ve-tos-cli-e2e-object-tag/test.txt"

    try:
        upload = cli_runner.run(
            ["ve-tos", ROOT, "upload", "--bucket", e2e_bucket_name, "--key", key,
             "--body", str(small_text_payload)]
        )
        if upload.exit_code != 0:
            skip_on_live_error("ve-tos object upload", upload)

        set_tag = cli_runner.run(
            ["ve-tos", ROOT, "set-tagging", "--bucket", e2e_bucket_name, "--key", key,
             "--tags", '{"TagSet":{"Tags":[{"Key":"env","Value":"e2e"}]}}']
        )
        if set_tag.exit_code != 0:
            skip_on_live_error("ve-tos object set-tagging", set_tag)
        envelope_assert.assert_success_envelope(set_tag.require_envelope())

        get_tag = cli_runner.run(
            ["ve-tos", ROOT, "get-tagging", "--bucket", e2e_bucket_name, "--key", key]
        )
        if get_tag.exit_code != 0:
            skip_on_live_error("ve-tos object get-tagging", get_tag)
        envelope_assert.assert_success_envelope(get_tag.require_envelope())
        body = get_tag.payload()["body"]
        tags = body.get("tag_set", {}).get("tags") or body.get("TagSet", {}).get("Tags")
        assert tags and tags[0].get("key", tags[0].get("Key")) == "env"

        # [Review Fix #2] delete-tagging is a delete-class operation and must
        # exercise the same non-interactive force + path confirmation contract.
        del_tag = cli_runner.run(
            [
                "ve-tos",
                ROOT,
                "delete-tagging",
                "--bucket",
                e2e_bucket_name,
                "--key",
                key,
                "--force",
                "--confirm",
                f"tos://{e2e_bucket_name}/{key}",
            ]
        )
        if del_tag.exit_code != 0:
            skip_on_live_error("ve-tos object delete-tagging", del_tag)
        envelope_assert.assert_success_envelope(del_tag.require_envelope())
    finally:
        _delete_object(cli_runner, e2e_bucket_name, key)


@pytest.mark.destructive
def test_object_acl_live_chain(
    cli_runner: CliRunner,
    e2e_bucket_name: str,
    small_text_payload: Path,
) -> None:
    """@case-id LL-CORE-OBJECT-ACL-LIVE set-acl -> get-acl."""
    from conftest import skip_on_live_error  # type: ignore[import-not-found]

    key = "ve-tos-cli-e2e-object-acl/test.txt"

    try:
        upload = cli_runner.run(
            ["ve-tos", ROOT, "upload", "--bucket", e2e_bucket_name, "--key", key,
             "--body", str(small_text_payload)]
        )
        if upload.exit_code != 0:
            skip_on_live_error("ve-tos object upload", upload)

        set_acl = cli_runner.run(
            ["ve-tos", ROOT, "set-acl", "--bucket", e2e_bucket_name, "--key", key,
             "--acl", "private"]
        )
        if set_acl.exit_code != 0:
            skip_on_live_error("ve-tos object set-acl", set_acl)
        envelope_assert.assert_success_envelope(set_acl.require_envelope())

        get_acl = cli_runner.run(
            ["ve-tos", ROOT, "get-acl", "--bucket", e2e_bucket_name, "--key", key]
        )
        if get_acl.exit_code != 0:
            skip_on_live_error("ve-tos object get-acl", get_acl)
        envelope_assert.assert_success_envelope(get_acl.require_envelope())
    finally:
        _delete_object(cli_runner, e2e_bucket_name, key)


@pytest.mark.destructive
def test_object_batch_delete_live_chain(
    cli_runner: CliRunner,
    e2e_bucket_name: str,
    small_text_payload: Path,
) -> None:
    """@case-id LL-CORE-OBJECT-BATCH-DELETE-LIVE upload multiple -> batch-delete."""
    from conftest import skip_on_live_error  # type: ignore[import-not-found]
    import json

    keys = [f"ve-tos-cli-e2e-batch-del/{i}.txt" for i in range(3)]

    try:
        for k in keys:
            r = cli_runner.run(
                ["ve-tos", ROOT, "upload", "--bucket", e2e_bucket_name, "--key", k,
                 "--body", str(small_text_payload)]
            )
            if r.exit_code != 0:
                skip_on_live_error("ve-tos object upload", r)

        keys_json = ",".join(keys)
        batch = cli_runner.run(
            ["ve-tos", ROOT, "batch-delete", "--bucket", e2e_bucket_name,
             "--keys", keys_json, "--force", "--confirm", f"tos://{e2e_bucket_name}"]
        )
        if batch.exit_code != 0:
            skip_on_live_error("ve-tos object batch-delete", batch)
        envelope_assert.assert_success_envelope(batch.require_envelope())
    finally:
        for k in keys:
            _delete_object(cli_runner, e2e_bucket_name, k)


@pytest.mark.destructive
def test_object_append_live_chain(
    cli_runner: CliRunner,
    e2e_bucket_name: str,
    tmp_path: Path,
) -> None:
    """@case-id LL-CORE-OBJECT-APPEND-LIVE append -> append -> head."""
    from conftest import skip_on_live_error  # type: ignore[import-not-found]

    key = "ve-tos-cli-e2e-object-append/appendable.txt"
    part1 = tmp_path / "part1.bin"
    part1.write_bytes(b"A" * 5 * 1024 * 1024)
    part2 = tmp_path / "part2.bin"
    part2.write_bytes(b"B" * 5 * 1024 * 1024)

    try:
        append1 = cli_runner.run(
            ["ve-tos", ROOT, "append", "--bucket", e2e_bucket_name, "--key", key,
             "--body", str(part1), "--offset", "0"]
        )
        if append1.exit_code != 0:
            skip_on_live_error("ve-tos object append", append1)
        envelope_assert.assert_success_envelope(append1.require_envelope())

        append2 = cli_runner.run(
            ["ve-tos", ROOT, "append", "--bucket", e2e_bucket_name, "--key", key,
             "--body", str(part2), "--offset", str(5 * 1024 * 1024)]
        )
        if append2.exit_code != 0:
            skip_on_live_error("ve-tos object append (2nd)", append2)
        envelope_assert.assert_success_envelope(append2.require_envelope())

        head = cli_runner.run(["ve-tos", ROOT, "head", "--bucket", e2e_bucket_name, "--key", key])
        assert head.exit_code == 0, head.stderr
    finally:
        _delete_object(cli_runner, e2e_bucket_name, key)


@pytest.mark.destructive
def test_object_seal_append_live_chain(
    cli_runner: CliRunner,
    e2e_bucket_name: str,
    tmp_path: Path,
) -> None:
    """@case-id LL-CORE-OBJECT-SEAL-APPEND-LIVE append -> seal-append -> head."""
    from conftest import skip_on_live_error  # type: ignore[import-not-found]

    key = "ve-tos-cli-e2e-object-seal/appendable.txt"
    part = tmp_path / "seal-part.bin"
    part.write_bytes(b"X" * 5 * 1024 * 1024)

    try:
        append = cli_runner.run(
            ["ve-tos", ROOT, "append", "--bucket", e2e_bucket_name, "--key", key,
             "--body", str(part), "--offset", "0"]
        )
        if append.exit_code != 0:
            skip_on_live_error("ve-tos object append", append)
        envelope_assert.assert_success_envelope(append.require_envelope())

        seal = cli_runner.run(
            ["ve-tos", ROOT, "seal-append", "--bucket", e2e_bucket_name, "--key", key,
             "--offset", str(5 * 1024 * 1024)]
        )
        if seal.exit_code != 0:
            skip_on_live_error("ve-tos object seal-append", seal)
        envelope_assert.assert_success_envelope(seal.require_envelope())

        head = cli_runner.run(["ve-tos", ROOT, "head", "--bucket", e2e_bucket_name, "--key", key])
        assert head.exit_code == 0, head.stderr
    finally:
        _delete_object(cli_runner, e2e_bucket_name, key)


@pytest.mark.destructive
def test_object_modify_live_chain(
    cli_runner: CliRunner,
    hns_temp_bucket: str,
    small_text_payload: Path,
    tmp_path: Path,
) -> None:
    """@case-id LL-CORE-OBJECT-MODIFY-LIVE upload -> modify at next offset -> verify."""
    from conftest import skip_on_live_error  # type: ignore[import-not-found]

    key = "ve-tos-cli-e2e-object-modify/test.txt"
    patch_file = tmp_path / "patch.bin"
    patch_file.write_bytes(b"PATCHED")
    original_bytes = small_text_payload.read_bytes()
    # [Review Fix #HNS-MODIFY-2] ModifyObject requires the current
    # next-modify-offset, so use the uploaded object's byte length.
    modify_offset = len(original_bytes)

    try:
        upload = cli_runner.run(
            ["ve-tos", ROOT, "upload", "--bucket", hns_temp_bucket, "--key", key,
             "--body", str(small_text_payload)]
        )
        if upload.exit_code != 0:
            skip_on_live_error("ve-tos object upload", upload)

        modify = cli_runner.run(
            ["ve-tos", ROOT, "modify", "--bucket", hns_temp_bucket, "--key", key,
             "--body", str(patch_file), "--offset", str(modify_offset)]
        )
        if modify.exit_code != 0:
            skip_on_live_error("ve-tos object modify", modify)
        envelope_assert.assert_success_envelope(modify.require_envelope())

        download_path = tmp_path / "modified.bin"
        download = cli_runner.run(
            ["ve-tos", ROOT, "download", "--bucket", hns_temp_bucket, "--key", key,
             "--body", str(download_path)]
        )
        assert download.exit_code == 0, download.stderr
        content = download_path.read_bytes()
        assert content == original_bytes + b"PATCHED"
    finally:
        _delete_object(cli_runner, hns_temp_bucket, key)


@pytest.mark.destructive
def test_object_fetch_live_chain(
    cli_runner: CliRunner,
    e2e_bucket_name: str,
    small_text_payload: Path,
) -> None:
    """@case-id LL-CORE-OBJECT-FETCH-LIVE upload source -> fetch to new key -> head."""
    from conftest import skip_on_live_error  # type: ignore[import-not-found]
    import os
    import time
    from urllib.parse import urlparse

    src_key = "ve-tos-cli-e2e-object-fetch/src.txt"
    dst_key = "ve-tos-cli-e2e-object-fetch/fetched.txt"
    endpoint = os.environ.get("TOS_ENDPOINT", "")
    if not endpoint:
        pytest.skip("TOS_ENDPOINT not set; cannot construct source URL for fetch")

    try:
        upload = cli_runner.run(
            ["ve-tos", ROOT, "upload", "--bucket", e2e_bucket_name, "--key", src_key,
             "--body", str(small_text_payload)]
        )
        if upload.exit_code != 0:
            skip_on_live_error("ve-tos object upload", upload)

        # [Review Fix] Set source ACL to public-read so TOS fetch can access it
        acl_result = cli_runner.run(
            ["ve-tos", "object", "set-acl", "--bucket", e2e_bucket_name, "--key", src_key,
             "--acl", "public-read"]
        )
        if acl_result.exit_code != 0:
            skip_on_live_error("ve-tos object set-acl (public-read for fetch)", acl_result)

        # [Review Fix #Fetch-E2E-1] Keep the source URL scheme aligned with
        # TOS_ENDPOINT and give public-read ACL propagation a short window.
        time.sleep(5)
        parsed_endpoint = urlparse(endpoint if "://" in endpoint else f"https://{endpoint}")
        ep_host = parsed_endpoint.netloc or parsed_endpoint.path
        source_url = f"{parsed_endpoint.scheme}://{e2e_bucket_name}.{ep_host.rstrip('/')}/{src_key}"
        fetch = cli_runner.run(
            ["ve-tos", ROOT, "fetch", "--bucket", e2e_bucket_name, "--key", dst_key,
             "--source-url", source_url, "--storage-class", "STANDARD"]
        )
        if fetch.exit_code != 0:
            skip_on_live_error("ve-tos object fetch", fetch)
        envelope_assert.assert_success_envelope(fetch.require_envelope())

        head = cli_runner.run(["ve-tos", ROOT, "head", "--bucket", e2e_bucket_name, "--key", dst_key])
        assert head.exit_code == 0, head.stderr
    finally:
        for k in (src_key, dst_key):
            _delete_object(cli_runner, e2e_bucket_name, k)


@pytest.mark.destructive
def test_object_create_fetch_task_get_fetch_task_live_chain(
    cli_runner: CliRunner,
    e2e_bucket_name: str,
    small_text_payload: Path,
) -> None:
    """@case-id LL-CORE-OBJECT-FETCH-TASK-LIVE create-fetch-task -> get-fetch-task."""
    from conftest import skip_on_live_error  # type: ignore[import-not-found]
    import os

    src_key = "ve-tos-cli-e2e-object-fetch-task/src.txt"
    dst_key = "ve-tos-cli-e2e-object-fetch-task/async-fetched.txt"
    endpoint = os.environ.get("TOS_ENDPOINT", "")
    if not endpoint:
        pytest.skip("TOS_ENDPOINT not set; cannot construct source URL for fetch task")

    try:
        upload = cli_runner.run(
            ["ve-tos", ROOT, "upload", "--bucket", e2e_bucket_name, "--key", src_key,
             "--body", str(small_text_payload)]
        )
        if upload.exit_code != 0:
            skip_on_live_error("ve-tos object upload", upload)

        ep_host = endpoint.replace("http://", "").replace("https://", "").rstrip("/")
        source_url = f"http://{e2e_bucket_name}.{ep_host}/{src_key}"
        create_task = cli_runner.run(
            ["ve-tos", ROOT, "create-fetch-task", "--bucket", e2e_bucket_name, "--key", dst_key,
             "--source-url", source_url]
        )
        if create_task.exit_code != 0:
            skip_on_live_error("ve-tos object create-fetch-task", create_task)
        envelope_assert.assert_success_envelope(create_task.require_envelope())
        task_body = create_task.payload().get("body") or {}
        task_id = task_body.get("task_id") or task_body.get("taskId") or task_body.get("TaskId")

        if task_id:
            get_task = cli_runner.run(
                ["ve-tos", ROOT, "get-fetch-task", "--bucket", e2e_bucket_name,
                 "--task-id", task_id]
            )
            if get_task.exit_code != 0:
                skip_on_live_error("ve-tos object get-fetch-task", get_task)
            envelope_assert.assert_success_envelope(get_task.require_envelope())
    finally:
        for k in (src_key, dst_key):
            _delete_object(cli_runner, e2e_bucket_name, k)


@pytest.mark.destructive
def test_object_set_meta_set_time_set_expires_live_chain(
    cli_runner: CliRunner,
    hns_temp_bucket: str,
    small_text_payload: Path,
) -> None:
    """@case-id LL-CORE-OBJECT-META-TIME-EXPIRES-LIVE set-meta -> set-time -> set-expires -> head."""
    from conftest import skip_on_live_error  # type: ignore[import-not-found]

    key = "ve-tos-cli-e2e-object-meta/test.txt"

    try:
        upload = cli_runner.run(
            ["ve-tos", ROOT, "upload", "--bucket", hns_temp_bucket, "--key", key,
             "--body", str(small_text_payload)]
        )
        if upload.exit_code != 0:
            skip_on_live_error("ve-tos object upload", upload)

        set_meta = cli_runner.run(
            ["ve-tos", ROOT, "set-meta", "--bucket", hns_temp_bucket, "--key", key,
             "--meta", "x-tos-meta-test=hello&x-tos-meta-env=e2e"]
        )
        if set_meta.exit_code != 0:
            skip_on_live_error("ve-tos object set-meta", set_meta)
        envelope_assert.assert_success_envelope(set_meta.require_envelope())

        set_time = cli_runner.run(
            ["ve-tos", ROOT, "set-time", "--bucket", hns_temp_bucket, "--key", key,
             "--time", "2025-01-01T00:00:00Z"]
        )
        if set_time.exit_code != 0:
            skip_on_live_error("ve-tos object set-time", set_time)
        envelope_assert.assert_success_envelope(set_time.require_envelope())

        set_expires = cli_runner.run(
            ["ve-tos", ROOT, "set-expires", "--bucket", hns_temp_bucket, "--key", key,
             "--expires", "2030-12-31T23:59:59Z"]
        )
        if set_expires.exit_code != 0:
            skip_on_live_error("ve-tos object set-expires", set_expires)
        envelope_assert.assert_success_envelope(set_expires.require_envelope())

        head = cli_runner.run(["ve-tos", ROOT, "head", "--bucket", hns_temp_bucket, "--key", key])
        assert head.exit_code == 0, head.stderr
    finally:
        _delete_object(cli_runner, hns_temp_bucket, key)


@pytest.mark.destructive
def test_object_rename_live_chain(
    cli_runner: CliRunner,
    rename_enabled_bucket: str,
    small_text_payload: Path,
) -> None:
    """@case-id LL-CORE-OBJECT-RENAME-LIVE upload -> rename -> head new -> delete."""
    from conftest import skip_on_live_error  # type: ignore[import-not-found]

    src_key = "ve-tos-cli-e2e-object-rename/src.txt"
    dst_key = "ve-tos-cli-e2e-object-rename/dst.txt"

    try:
        upload = cli_runner.run(
            ["ve-tos", ROOT, "upload", "--bucket", rename_enabled_bucket, "--key", src_key,
             "--body", str(small_text_payload)]
        )
        if upload.exit_code != 0:
            skip_on_live_error("ve-tos object upload", upload)

        rename = cli_runner.run(
            ["ve-tos", ROOT, "rename",
             f"tos://{rename_enabled_bucket}/{src_key}",
             f"tos://{rename_enabled_bucket}/{dst_key}"]
        )
        if rename.exit_code != 0:
            skip_on_live_error("ve-tos object rename", rename)
        envelope_assert.assert_success_envelope(rename.require_envelope())

        head = cli_runner.run(["ve-tos", ROOT, "head", "--bucket", rename_enabled_bucket, "--key", dst_key])
        assert head.exit_code == 0, head.stderr
    finally:
        _delete_object(cli_runner, rename_enabled_bucket, src_key)
        _delete_object(cli_runner, rename_enabled_bucket, dst_key)


@pytest.mark.destructive
def test_object_symlink_link_live_chain(
    cli_runner: CliRunner,
    e2e_bucket_name: str,
    hns_temp_bucket: str,
    small_text_payload: Path,
) -> None:
    """@case-id LL-CORE-OBJECT-SYMLINK-LINK-LIVE create-symlink -> get-symlink; link -> head."""
    from conftest import skip_on_live_error  # type: ignore[import-not-found]

    target_key = "ve-tos-cli-e2e-object-sym/target.txt"
    symlink_key = "ve-tos-cli-e2e-object-sym/link.txt"
    hns_target_key = "ve-tos-cli-e2e-object-link/target.txt"
    hardlink_key = "ve-tos-cli-e2e-object-sym/hard.txt"

    try:
        upload = cli_runner.run(
            ["ve-tos", ROOT, "upload", "--bucket", e2e_bucket_name, "--key", target_key,
             "--body", str(small_text_payload)]
        )
        if upload.exit_code != 0:
            skip_on_live_error("ve-tos object upload", upload)

        create_sym = cli_runner.run(
            ["ve-tos", ROOT, "create-symlink", "--bucket", e2e_bucket_name, "--key", symlink_key,
             "--target-key", target_key]
        )
        if create_sym.exit_code != 0:
            skip_on_live_error("ve-tos object create-symlink", create_sym)
        envelope_assert.assert_success_envelope(create_sym.require_envelope())

        get_sym = cli_runner.run(
            ["ve-tos", ROOT, "get-symlink", "--bucket", e2e_bucket_name, "--key", symlink_key]
        )
        if get_sym.exit_code != 0:
            skip_on_live_error("ve-tos object get-symlink", get_sym)
        envelope_assert.assert_success_envelope(get_sym.require_envelope())

        hns_upload = cli_runner.run(
            ["ve-tos", ROOT, "upload", "--bucket", hns_temp_bucket, "--key", hns_target_key,
             "--body", str(small_text_payload)]
        )
        if hns_upload.exit_code != 0:
            skip_on_live_error("ve-tos object upload (HNS link target)", hns_upload)

        link = cli_runner.run(
            ["ve-tos", ROOT, "link", "--bucket", hns_temp_bucket, "--key", hardlink_key,
             "--source-key", hns_target_key]
        )
        if link.exit_code != 0:
            skip_on_live_error("ve-tos object link", link)
        envelope_assert.assert_success_envelope(link.require_envelope())

        head_hard = cli_runner.run(
            ["ve-tos", ROOT, "head", "--bucket", hns_temp_bucket, "--key", hardlink_key]
        )
        if head_hard.exit_code != 0:
            skip_on_live_error("ve-tos object head (hardlink)", head_hard)
    finally:
        for k in (target_key, symlink_key):
            _delete_object(cli_runner, e2e_bucket_name, k)
        for k in (hns_target_key, hardlink_key):
            _delete_object(cli_runner, hns_temp_bucket, k)


@pytest.mark.destructive
def test_object_list_versions_live_chain(
    cli_runner: CliRunner,
    e2e_bucket_name: str,
    small_text_payload: Path,
) -> None:
    """@case-id LL-CORE-OBJECT-LIST-VERSIONS-LIVE list-versions with prefix filter."""
    from conftest import skip_on_live_error  # type: ignore[import-not-found]

    key = "ve-tos-cli-e2e-object-versions/test.txt"

    try:
        upload = cli_runner.run(
            ["ve-tos", ROOT, "upload", "--bucket", e2e_bucket_name, "--key", key,
             "--body", str(small_text_payload)]
        )
        if upload.exit_code != 0:
            skip_on_live_error("ve-tos object upload", upload)

        list_ver = cli_runner.run(
            ["ve-tos", ROOT, "list-versions", "--bucket", e2e_bucket_name,
             "--prefix", "ve-tos-cli-e2e-object-versions/"]
        )
        if list_ver.exit_code != 0:
            skip_on_live_error("ve-tos object list-versions", list_ver)
        envelope_assert.assert_success_envelope(list_ver.require_envelope())
    finally:
        _delete_object(cli_runner, e2e_bucket_name, key)


@pytest.mark.destructive
def test_object_restore_status_live_chain(
    cli_runner: CliRunner,
    e2e_bucket_name: str,
    small_text_payload: Path,
) -> None:
    """@case-id LL-CORE-OBJECT-RESTORE-STATUS-LIVE restore -> status on archive object."""
    from conftest import skip_on_live_error  # type: ignore[import-not-found]

    key = "ve-tos-cli-e2e-object-restore/archive.txt"

    try:
        upload = cli_runner.run(
            ["ve-tos", ROOT, "upload", "--bucket", e2e_bucket_name, "--key", key,
             "--body", str(small_text_payload), "--storage-class", "ARCHIVE"]
        )
        if upload.exit_code != 0:
            skip_on_live_error("ve-tos object upload (archive)", upload)

        restore = cli_runner.run(
            ["ve-tos", ROOT, "restore", "--bucket", e2e_bucket_name, "--key", key,
             "--days", "1"]
        )
        if restore.exit_code != 0:
            skip_on_live_error("ve-tos object restore", restore)
        envelope_assert.assert_success_envelope(restore.require_envelope())

        head = cli_runner.run(
            ["ve-tos", ROOT, "head", "--bucket", e2e_bucket_name, "--key", key]
        )
        if head.exit_code != 0:
            skip_on_live_error("ve-tos object head (restore status)", head)
        envelope_assert.assert_success_envelope(head.require_envelope())
        headers = head.payload().get("headers", {})
        restore_headers = {
            header_name: header_value
            for header_name, header_value in headers.items()
            if header_name.lower().startswith("x-tos-restore")
        }
        assert restore_headers, f"expected restore status headers after restore, got {headers!r}"
    finally:
        _delete_object(cli_runner, e2e_bucket_name, key)


@pytest.mark.destructive
def test_object_retention_live_chain(
    cli_runner: CliRunner,
    object_lock_temp_bucket: str,
    small_text_payload: Path,
) -> None:
    """@case-id LL-CORE-OBJECT-RETENTION-LIVE set-retention -> get-retention."""
    from conftest import skip_on_live_error  # type: ignore[import-not-found]
    from datetime import datetime, timedelta, timezone
    import time

    key = "ve-tos-cli-e2e-object-retention/test.txt"
    retain_until = datetime.now(timezone.utc) + timedelta(seconds=75)
    retain_until_text = retain_until.strftime("%Y-%m-%dT%H:%M:%SZ")
    retention_set = False

    try:
        upload = cli_runner.run(
            ["ve-tos", ROOT, "upload", "--bucket", object_lock_temp_bucket, "--key", key,
             "--body", str(small_text_payload)]
        )
        if upload.exit_code != 0:
            skip_on_live_error("ve-tos object upload", upload)

        set_ret = cli_runner.run(
            ["ve-tos", ROOT, "set-retention", "--bucket", object_lock_temp_bucket, "--key", key,
             "--mode", "COMPLIANCE", "--retain-until-date", retain_until_text]
        )
        if set_ret.exit_code != 0:
            skip_on_live_error("ve-tos object set-retention", set_ret)
        retention_set = True
        envelope_assert.assert_success_envelope(set_ret.require_envelope())

        get_ret = cli_runner.run(
            ["ve-tos", ROOT, "get-retention", "--bucket", object_lock_temp_bucket, "--key", key]
        )
        if get_ret.exit_code != 0:
            skip_on_live_error("ve-tos object get-retention", get_ret)
        envelope_assert.assert_success_envelope(get_ret.require_envelope())
    finally:
        if retention_set:
            remaining = (retain_until - datetime.now(timezone.utc)).total_seconds()
            if remaining > 0:
                time.sleep(remaining + 5)
        _delete_object(cli_runner, object_lock_temp_bucket, key)


@pytest.mark.destructive
def test_object_form_upload_live_chain(
    cli_runner: CliRunner,
    e2e_bucket_name: str,
    small_text_payload: Path,
) -> None:
    """@case-id LL-CORE-OBJECT-FORM-UPLOAD-LIVE form-upload -> head -> delete."""
    from conftest import skip_on_live_error  # type: ignore[import-not-found]

    key = "ve-tos-cli-e2e-object-form/upload.txt"

    try:
        form_upload = cli_runner.run(
            ["ve-tos", ROOT, "form-upload", "--bucket", e2e_bucket_name, "--key", key,
             "--body", str(small_text_payload)]
        )
        if form_upload.exit_code != 0:
            skip_on_live_error("ve-tos object form-upload", form_upload)
        envelope_assert.assert_success_envelope(form_upload.require_envelope())

        head = cli_runner.run(["ve-tos", ROOT, "head", "--bucket", e2e_bucket_name, "--key", key])
        assert head.exit_code == 0, head.stderr
    finally:
        _delete_object(cli_runner, e2e_bucket_name, key)
