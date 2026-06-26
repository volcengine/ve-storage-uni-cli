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

"""Black-box E2E coverage for ``ve-tos control`` low-level APIs.

This script intentionally discovers leaf operations from the running binary via
``ve-tos capabilities --view tree`` and then executes only black-box CLI calls.
"""

from __future__ import annotations

import os
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

ROOT = "control"
EXPECTED_VALIDATION_FAILURES = {
    # The current CLI metadata exposes only --config/--content-md5, while the
    # handler still requires skipped fields; assert this black-box behavior
    # instead of silently dropping the leaf from coverage.
    "ve-tos real-time-log set": "requires --use-service-topic",
}


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
def control_leaf_commands(cli_runner: CliRunner) -> list[dict[str, Any]]:
    result = cli_runner.run(["ve-tos", "capabilities", "--view", "tree"])
    assert result.exit_code == 0, result.stderr
    envelope_assert.assert_success_envelope(result.require_envelope())
    return _root_leaf_commands(result.payload()["commands"])


@pytest.mark.slow
def test_control_all_leaf_commands_exercise_full_parameter_surface_dry_run(
    cli_runner: CliRunner,
    control_leaf_commands: list[dict[str, Any]],
    tmp_path: Path,
) -> None:
    """@case-id LL-ADV-CONTROL-SURFACE every leaf operation and parameter is asserted."""

    failures: list[str] = []
    total_cases = 0

    for leaf in control_leaf_commands:
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

    assert total_cases >= len(control_leaf_commands)
    assert not failures, "\n".join(failures[:80])


@pytest.mark.slow
def test_control_leaf_operations_chain_in_metadata_order_dry_run(
    cli_runner: CliRunner,
    control_leaf_commands: list[dict[str, Any]],
    tmp_path: Path,
) -> None:
    """@case-id LL-ADV-CONTROL-CHAIN all operations for this root are asserted in one flow."""

    seen_commands: list[str] = []
    failures: list[str] = []

    for leaf in control_leaf_commands:
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

    expected_commands = {str(leaf["command"]) for leaf in control_leaf_commands}
    assert set(seen_commands) == expected_commands
    assert not failures, "\n".join(failures[:80])

@pytest.mark.destructive
def test_control_control_plane_list_live_chain(
    cli_runner: CliRunner,
) -> None:
    """@case-id LL-ADV-CONTROL-LIVE control endpoint list probe."""

    account_id = os.environ.get("TOS_ACCOUNT_ID")
    if not account_id:
        pytest.skip("TOS_ACCOUNT_ID not set; control-plane commands require account ID")

    result = cli_runner.run(["ve-tos", 'control', 'list-batch-jobs', "--account-id", account_id, "--output", "json"])
    if result.exit_code != 0:
        skip_on_live_error("ve-tos control list-batch-jobs", result)
    envelope_assert.assert_success_envelope(result.require_envelope())
    assert isinstance(result.payload(), dict)


## [Review Fix] Removed test_control_resource_tags_crud_live_chain:
## set-resource-tag / list-resource-tags / delete-resource-tag are NOT supported TOS commands.


@pytest.mark.destructive
def test_control_subscribe_crud_live_chain(
    cli_runner: CliRunner,
) -> None:
    """@case-id LL-ADV-CONTROL-SUBSCRIBE set -> get -> delete cycle."""

    import json

    account_id = os.environ.get("TOS_ACCOUNT_ID")
    if not account_id:
        pytest.skip("TOS_ACCOUNT_ID not set; control-plane commands require account ID")

    test_config = {
        "Enabled": True,
    }

    # Step 1: Set subscribe
    set_result = cli_runner.run([
        "ve-tos", ROOT, "set-subscribe",
        "--account-id", account_id,
        "--config", json.dumps(test_config, separators=(",", ":")),
    ])
    if set_result.exit_code != 0:
        skip_on_live_error(f"ve-tos {ROOT} set-subscribe", set_result)

    try:
        envelope_assert.assert_success_envelope(set_result.require_envelope())

        # Step 2: Get subscribe
        get_result = cli_runner.run([
            "ve-tos", ROOT, "get-subscribe",
            "--account-id", account_id,
        ])
        if get_result.exit_code != 0:
            skip_on_live_error(f"ve-tos {ROOT} get-subscribe", get_result)
        envelope_assert.assert_success_envelope(get_result.require_envelope())

        # Step 3: Delete subscribe
        del_result = cli_runner.run([
            "ve-tos", ROOT, "delete-subscribe",
            "--account-id", account_id,
        ])
        if del_result.exit_code != 0:
            skip_on_live_error(f"ve-tos {ROOT} delete-subscribe", del_result)
        envelope_assert.assert_success_envelope(del_result.require_envelope())
    finally:
        cli_runner.run([
            "ve-tos", ROOT, "delete-subscribe",
            "--account-id", account_id,
        ])


@pytest.mark.destructive
def test_control_lens_crud_live_chain(
    cli_runner: CliRunner,
) -> None:
    """@case-id LL-ADV-CONTROL-LENS set -> get -> list -> delete cycle."""

    import json
    import hashlib
    import base64

    account_id = os.environ.get("TOS_ACCOUNT_ID")
    if not account_id:
        pytest.skip("TOS_ACCOUNT_ID not set; control-plane commands require account ID")

    lens_id = "e2e-test-lens"
    test_config = {
        "Id": lens_id,
        "AccountId": account_id,
        "StorageLensConfiguration": {
            "IsEnabled": True,
        },
    }

    # Step 1: Set lens
    config_json = json.dumps(test_config, separators=(",", ":"))
    content_md5 = base64.b64encode(hashlib.md5(config_json.encode()).digest()).decode()

    set_result = cli_runner.run([
        "ve-tos", ROOT, "set-lens",
        "--account-id", account_id,
        "--id", lens_id,
        "--config", config_json,
        "--content-md5", content_md5,
    ])
    if set_result.exit_code != 0:
        skip_on_live_error(f"ve-tos {ROOT} set-lens", set_result)

    try:
        envelope_assert.assert_success_envelope(set_result.require_envelope())

        # Step 2: Get lens
        get_result = cli_runner.run([
            "ve-tos", ROOT, "get-lens",
            "--account-id", account_id,
            "--id", lens_id,
        ])
        if get_result.exit_code != 0:
            skip_on_live_error(f"ve-tos {ROOT} get-lens", get_result)
        envelope_assert.assert_success_envelope(get_result.require_envelope())

        # Step 3: List lens
        list_result = cli_runner.run([
            "ve-tos", ROOT, "list-lens",
            "--account-id", account_id,
        ])
        if list_result.exit_code != 0:
            skip_on_live_error(f"ve-tos {ROOT} list-lens", list_result)
        envelope_assert.assert_success_envelope(list_result.require_envelope())

        # Step 4: Delete lens
        del_result = cli_runner.run([
            "ve-tos", ROOT, "delete-lens",
            "--account-id", account_id,
            "--id", lens_id,
            "--force",
            "--confirm", f"tos://{lens_id}",
        ])
        if del_result.exit_code != 0:
            skip_on_live_error(f"ve-tos {ROOT} delete-lens", del_result)
        envelope_assert.assert_success_envelope(del_result.require_envelope())
    finally:
        cli_runner.run([
            "ve-tos", ROOT, "delete-lens",
            "--account-id", account_id,
            "--id", lens_id,
            "--force",
            "--confirm", f"tos://{lens_id}",
        ])


@pytest.mark.destructive
def test_control_qos_policy_crud_live_chain(
    cli_runner: CliRunner,
) -> None:
    """@case-id LL-ADV-CONTROL-QOS set -> get -> delete cycle."""

    import json

    account_id = os.environ.get("TOS_ACCOUNT_ID")
    if not account_id:
        pytest.skip("TOS_ACCOUNT_ID not set; control-plane commands require account ID")

    test_config = {
        "Statement": [
            {
                "Sid": "e2e-qos-test",
                "Quota": {
                    "WritesQps": "",
                    "ReadsQps": "",
                    "ListQps": "",
                    "WritesRate": "5368709120",
                    "ReadsRate": "",
                },
                "Principal": ["*"],
                "Resource": "*",
            },
        ],
    }

    # Step 1: Set QoS policy
    set_result = cli_runner.run([
        "ve-tos", ROOT, "set-qos-policy",
        "--account-id", account_id,
        "--config", json.dumps(test_config, separators=(",", ":")),
    ])
    if set_result.exit_code != 0:
        skip_on_live_error(f"ve-tos {ROOT} set-qos-policy", set_result)

    try:
        envelope_assert.assert_success_envelope(set_result.require_envelope())

        # Step 2: Get QoS policy
        get_result = cli_runner.run([
            "ve-tos", ROOT, "get-qos-policy",
            "--account-id", account_id,
        ])
        if get_result.exit_code != 0:
            skip_on_live_error(f"ve-tos {ROOT} get-qos-policy", get_result)
        envelope_assert.assert_success_envelope(get_result.require_envelope())

        # Step 3: Delete QoS policy
        del_result = cli_runner.run([
            "ve-tos", ROOT, "delete-qos-policy",
            "--account-id", account_id,
            "--force",
            "--confirm", "tos://qospolicy",
        ])
        if del_result.exit_code != 0:
            skip_on_live_error(f"ve-tos {ROOT} delete-qos-policy", del_result)
        envelope_assert.assert_success_envelope(del_result.require_envelope())
    finally:
        cli_runner.run([
            "ve-tos", ROOT, "delete-qos-policy",
            "--account-id", account_id,
            "--force",
            "--confirm", "tos://qospolicy",
        ])


@pytest.mark.destructive
def test_control_url_cache_crud_live_chain(
    cli_runner: CliRunner,
    e2e_bucket_name: str,
) -> None:
    """@case-id LL-ADV-CONTROL-URL-CACHE create -> delete cycle."""

    import json

    test_config = {
        "URLs": ["https://example.com/file.txt"],
    }

    # Step 1: Create URL cache
    create_result = cli_runner.run([
        "ve-tos", ROOT, "create-url-cache", "--bucket", e2e_bucket_name,
        "--config", json.dumps(test_config, separators=(",", ":")),
    ])
    if create_result.exit_code != 0:
        skip_on_live_error(f"ve-tos {ROOT} create-url-cache", create_result)

    try:
        envelope_assert.assert_success_envelope(create_result.require_envelope())

        # Step 2: Delete URL cache
        del_result = cli_runner.run([
            "ve-tos", ROOT, "delete-url-cache", "--bucket", e2e_bucket_name,
        ])
        if del_result.exit_code != 0:
            skip_on_live_error(f"ve-tos {ROOT} delete-url-cache", del_result)
        envelope_assert.assert_success_envelope(del_result.require_envelope())
    finally:
        cli_runner.run([
            "ve-tos", ROOT, "delete-url-cache", "--bucket", e2e_bucket_name,
        ])
