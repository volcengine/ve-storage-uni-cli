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

"""Black-box E2E coverage for ``ve-tos dataset`` low-level APIs.

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

ROOT = "dataset"
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
def dataset_leaf_commands(cli_runner: CliRunner) -> list[dict[str, Any]]:
    result = cli_runner.run(["ve-tos", "capabilities", "--view", "tree"])
    assert result.exit_code == 0, result.stderr
    envelope_assert.assert_success_envelope(result.require_envelope())
    return _root_leaf_commands(result.payload()["commands"])


@pytest.mark.slow
def test_dataset_all_leaf_commands_exercise_full_parameter_surface_dry_run(
    cli_runner: CliRunner,
    dataset_leaf_commands: list[dict[str, Any]],
    tmp_path: Path,
) -> None:
    """@case-id LL-ADV-DATASET-SURFACE every leaf operation and parameter is asserted."""

    failures: list[str] = []
    total_cases = 0

    for leaf in dataset_leaf_commands:
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

    assert total_cases >= len(dataset_leaf_commands)
    assert not failures, "\n".join(failures[:80])


@pytest.mark.slow
def test_dataset_leaf_operations_chain_in_metadata_order_dry_run(
    cli_runner: CliRunner,
    dataset_leaf_commands: list[dict[str, Any]],
    tmp_path: Path,
) -> None:
    """@case-id LL-ADV-DATASET-CHAIN all operations for this root are asserted in one flow."""

    seen_commands: list[str] = []
    failures: list[str] = []

    for leaf in dataset_leaf_commands:
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

    expected_commands = {str(leaf["command"]) for leaf in dataset_leaf_commands}
    assert set(seen_commands) == expected_commands
    assert not failures, "\n".join(failures[:80])

@pytest.mark.destructive
def test_dataset_control_plane_list_live_chain(
    cli_runner: CliRunner,
) -> None:
    """@case-id LL-ADV-DATASET-LIVE control endpoint list probe."""

    account_id = os.environ.get("TOS_ACCOUNT_ID")
    if not account_id:
        pytest.skip("TOS_ACCOUNT_ID not set; control-plane commands require account ID")

    result = cli_runner.run(["ve-tos", 'dataset', 'list', "--account-id", account_id, "--output", "json"])
    if result.exit_code != 0:
        skip_on_live_error("ve-tos dataset list", result)
    envelope_assert.assert_success_envelope(result.require_envelope())
    assert isinstance(result.payload(), dict)


@pytest.mark.destructive
def test_dataset_full_crud_live_chain(
    cli_runner: CliRunner,
    e2e_bucket_name: str,
) -> None:
    """@case-id LL-ADV-DATASET-CRUD list-templates -> create -> get -> list -> delete cycle."""

    import json

    account_id = os.environ.get("TOS_ACCOUNT_ID")
    if not account_id:
        pytest.skip("TOS_ACCOUNT_ID not set; control-plane commands require account ID")

    dataset_name = f"e2e-ds-{e2e_bucket_name[:20]}"

    # Step 0: List templates to find a valid TemplateId
    templates_result = cli_runner.run([
        "ve-tos", ROOT, "list-templates", "--account-id", account_id, "--output", "json",
    ])
    if templates_result.exit_code != 0:
        skip_on_live_error(f"ve-tos {ROOT} list-templates", templates_result)
    envelope_assert.assert_success_envelope(templates_result.require_envelope())
    templates_body = templates_result.payload().get("body", templates_result.payload())
    # Try to extract first available template ID
    templates = templates_body.get("templates") or templates_body.get("Templates") or []
    template_id = ""
    if templates and isinstance(templates, list) and len(templates) > 0:
        template_id = templates[0].get("template_id") or templates[0].get("TemplateId") or ""
    if not template_id:
        pytest.skip("No dataset templates available on this account")

    # Step 1: Create dataset
    create_config = {
        "DatasetName": dataset_name,
        "BucketName": e2e_bucket_name,
        "TemplateId": template_id,
    }
    create_result = cli_runner.run([
        "ve-tos", ROOT, "create", "--account-id", account_id,
        "--config", json.dumps(create_config, separators=(",", ":")),
    ])
    if create_result.exit_code != 0:
        skip_on_live_error(f"ve-tos {ROOT} create", create_result)

    try:
        envelope_assert.assert_success_envelope(create_result.require_envelope())

        # Step 2: Get dataset
        get_result = cli_runner.run([
            "ve-tos", ROOT, "get", "--account-id", account_id,
            "--name", dataset_name,
        ])
        if get_result.exit_code != 0:
            skip_on_live_error(f"ve-tos {ROOT} get", get_result)
        envelope_assert.assert_success_envelope(get_result.require_envelope())

        # Step 3: List datasets
        list_result = cli_runner.run([
            "ve-tos", ROOT, "list", "--account-id", account_id, "--output", "json",
        ])
        if list_result.exit_code != 0:
            skip_on_live_error(f"ve-tos {ROOT} list", list_result)
        envelope_assert.assert_success_envelope(list_result.require_envelope())

        # Step 4: Delete dataset
        del_result = cli_runner.run([
            "ve-tos", ROOT, "delete", "--account-id", account_id,
            "--name", dataset_name,
            "--force",
            "--confirm", f"tos://{dataset_name}",
        ])
        if del_result.exit_code != 0:
            skip_on_live_error(f"ve-tos {ROOT} delete", del_result)
        envelope_assert.assert_success_envelope(del_result.require_envelope())
    finally:
        # Best-effort cleanup
        cli_runner.run([
            "ve-tos", ROOT, "delete", "--account-id", account_id,
            "--name", dataset_name,
            "--force", "--confirm", f"tos://{dataset_name}",
        ])
