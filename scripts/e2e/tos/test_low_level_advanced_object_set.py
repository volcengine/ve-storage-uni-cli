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

"""Black-box E2E coverage for ``ve-tos object-set`` low-level APIs.

This script intentionally discovers leaf operations from the running binary via
``ve-tos capabilities --view tree`` and then executes only black-box CLI calls.
"""

from __future__ import annotations

import json
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

ROOT = "object-set"
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
def object_set_leaf_commands(cli_runner: CliRunner) -> list[dict[str, Any]]:
    result = cli_runner.run(["ve-tos", "capabilities", "--view", "tree"])
    assert result.exit_code == 0, result.stderr
    envelope_assert.assert_success_envelope(result.require_envelope())
    return _root_leaf_commands(result.payload()["commands"])


@pytest.mark.slow
def test_object_set_all_leaf_commands_exercise_full_parameter_surface_dry_run(
    cli_runner: CliRunner,
    object_set_leaf_commands: list[dict[str, Any]],
    tmp_path: Path,
) -> None:
    """@case-id LL-ADV-OBJECT_SET-SURFACE every leaf operation and parameter is asserted."""

    failures: list[str] = []
    total_cases = 0

    for leaf in object_set_leaf_commands:
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

    assert total_cases >= len(object_set_leaf_commands)
    assert not failures, "\n".join(failures[:80])


@pytest.mark.slow
def test_object_set_leaf_operations_chain_in_metadata_order_dry_run(
    cli_runner: CliRunner,
    object_set_leaf_commands: list[dict[str, Any]],
    tmp_path: Path,
) -> None:
    """@case-id LL-ADV-OBJECT_SET-CHAIN all operations for this root are asserted in one flow."""

    seen_commands: list[str] = []
    failures: list[str] = []

    for leaf in object_set_leaf_commands:
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

    expected_commands = {str(leaf["command"]) for leaf in object_set_leaf_commands}
    assert set(seen_commands) == expected_commands
    assert not failures, "\n".join(failures[:80])

@pytest.mark.destructive
def test_object_set_control_plane_list_live_chain(
    cli_runner: CliRunner,
    e2e_bucket_name: str,
) -> None:
    """@case-id LL-ADV-OBJECT_SET-LIVE control endpoint list probe."""

    account_id = os.environ.get("TOS_ACCOUNT_ID")
    if not account_id:
        pytest.skip("TOS_ACCOUNT_ID not set; control-plane commands require account ID")

    templates_result = cli_runner.run([
        "ve-tos", "dataset", "list-templates", "--account-id", account_id, "--output", "json",
    ])
    if templates_result.exit_code != 0:
        skip_on_live_error("ve-tos dataset list-templates", templates_result)
    templates_body = templates_result.payload().get("body", templates_result.payload())
    templates = templates_body.get("templates") or templates_body.get("Templates") or []
    template_id = ""
    if templates and isinstance(templates, list):
        template_id = templates[0].get("template_id") or templates[0].get("TemplateId") or ""
    if not template_id:
        pytest.skip("No dataset templates available on this account")

    try:
        create_config = {
            "DatasetName": e2e_bucket_name,
            "BucketName": e2e_bucket_name,
            "TemplateId": template_id,
        }
        create_ds = cli_runner.run([
            "ve-tos", "dataset", "create", "--account-id", account_id,
            "--config", json.dumps(create_config, separators=(",", ":")),
        ])
        if create_ds.exit_code != 0:
            skip_on_live_error("ve-tos dataset create", create_ds)

        # [Review Fix #ObjectSetSetup] ListObjectSet requires the bucket-level
        # object-set configuration to be enabled in addition to creating the
        # control-plane dataset.
        global_config = {
            "PathLevel": 1,
            "EnableDefaultObjectSet": True,
        }
        set_global = cli_runner.run([
            "ve-tos", "object-set", "set-global", "--bucket", e2e_bucket_name,
            "--config", json.dumps(global_config, separators=(",", ":")),
        ])
        if set_global.exit_code != 0:
            skip_on_live_error("ve-tos object-set set-global", set_global)

        result = cli_runner.run(
            ["ve-tos", "object-set", "list", "--bucket", e2e_bucket_name, "--output", "json"]
        )
        if result.exit_code != 0:
            skip_on_live_error("ve-tos object-set list", result)
        envelope_assert.assert_success_envelope(result.require_envelope())
        assert isinstance(result.payload(), dict)
    finally:
        # Best-effort cleanup: delete dataset
        cli_runner.run(
            [
                "ve-tos", "dataset", "delete", "--account-id", account_id,
                "--name", e2e_bucket_name, "--force", "--confirm", f"tos://{e2e_bucket_name}",
            ]
        )


@pytest.mark.destructive
def test_object_set_global_config_live_chain(
    cli_runner: CliRunner,
    e2e_bucket_name: str,
) -> None:
    """@case-id LL-ADV-OBJECT-SET-GLOBAL set-global -> get-global cycle."""

    import json

    account_id = os.environ.get("TOS_ACCOUNT_ID")
    if not account_id:
        pytest.skip("TOS_ACCOUNT_ID not set; control-plane commands require account ID")

    # Ensure dataset exists (prerequisite for object-set)
    dataset_name = e2e_bucket_name

    # Step 0: List templates to get TemplateId
    templates_result = cli_runner.run([
        "ve-tos", "dataset", "list-templates", "--account-id", account_id, "--output", "json",
    ])
    if templates_result.exit_code != 0:
        skip_on_live_error("ve-tos dataset list-templates", templates_result)
    templates_body = templates_result.payload().get("body", templates_result.payload())
    templates = templates_body.get("templates") or templates_body.get("Templates") or []
    template_id = ""
    if templates and isinstance(templates, list) and len(templates) > 0:
        template_id = templates[0].get("template_id") or templates[0].get("TemplateId") or ""
    if not template_id:
        pytest.skip("No dataset templates available on this account")

    # Step 0b: Create dataset for this bucket (idempotent attempt)
    create_config = {
        "DatasetName": dataset_name,
        "BucketName": e2e_bucket_name,
        "TemplateId": template_id,
    }
    cli_runner.run([
        "ve-tos", "dataset", "create", "--account-id", account_id,
        "--config", json.dumps(create_config, separators=(",", ":")),
    ])

    try:
        # [Review Fix] Body matches Go SDK PutBucketObjectSetConfigurationInput (flat, no wrapper)
        global_config = {
            "PathLevel": 1,
            "EnableDefaultObjectSet": True,
        }
        set_result = cli_runner.run([
            "ve-tos", ROOT, "set-global", "--bucket", e2e_bucket_name,
            "--config", json.dumps(global_config, separators=(",", ":")),
        ])
        if set_result.exit_code != 0:
            skip_on_live_error(f"ve-tos {ROOT} set-global", set_result)
        envelope_assert.assert_success_envelope(set_result.require_envelope())

        # Step 2: Get global config
        get_result = cli_runner.run([
            "ve-tos", ROOT, "get-global", "--bucket", e2e_bucket_name,
        ])
        if get_result.exit_code != 0:
            skip_on_live_error(f"ve-tos {ROOT} get-global", get_result)
        envelope_assert.assert_success_envelope(get_result.require_envelope())

        # Step 3: List object sets
        list_result = cli_runner.run([
            "ve-tos", ROOT, "list", "--bucket", e2e_bucket_name,
        ])
        if list_result.exit_code != 0:
            skip_on_live_error(f"ve-tos {ROOT} list", list_result)
        envelope_assert.assert_success_envelope(list_result.require_envelope())

    finally:
        # Best-effort cleanup: delete dataset
        cli_runner.run([
            "ve-tos", "dataset", "delete", "--account-id", account_id,
            "--name", dataset_name, "--force", "--confirm", f"tos://{dataset_name}",
        ])
