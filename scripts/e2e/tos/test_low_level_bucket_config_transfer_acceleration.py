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

"""Black-box E2E coverage for ``ve-tos transfer-acceleration`` low-level APIs.

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

ROOT = "transfer-acceleration"
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
def transfer_acceleration_leaf_commands(cli_runner: CliRunner) -> list[dict[str, Any]]:
    result = cli_runner.run(["ve-tos", "capabilities", "--view", "tree"])
    assert result.exit_code == 0, result.stderr
    envelope_assert.assert_success_envelope(result.require_envelope())
    return _root_leaf_commands(result.payload()["commands"])


@pytest.mark.slow
def test_transfer_acceleration_all_leaf_commands_exercise_full_parameter_surface_dry_run(
    cli_runner: CliRunner,
    transfer_acceleration_leaf_commands: list[dict[str, Any]],
    tmp_path: Path,
) -> None:
    """@case-id LL-BC-TRANSFER_ACCELERATION-SURFACE every leaf operation and parameter is asserted."""

    failures: list[str] = []
    total_cases = 0

    for leaf in transfer_acceleration_leaf_commands:
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

    assert total_cases >= len(transfer_acceleration_leaf_commands)
    assert not failures, "\n".join(failures[:80])


@pytest.mark.slow
def test_transfer_acceleration_leaf_operations_chain_in_metadata_order_dry_run(
    cli_runner: CliRunner,
    transfer_acceleration_leaf_commands: list[dict[str, Any]],
    tmp_path: Path,
) -> None:
    """@case-id LL-BC-TRANSFER_ACCELERATION-CHAIN all operations for this root are asserted in one flow."""

    seen_commands: list[str] = []
    failures: list[str] = []

    for leaf in transfer_acceleration_leaf_commands:
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

    expected_commands = {str(leaf["command"]) for leaf in transfer_acceleration_leaf_commands}
    assert set(seen_commands) == expected_commands
    assert not failures, "\n".join(failures[:80])

@pytest.mark.destructive
def test_transfer_acceleration_set_get_live_chain(
    cli_runner: CliRunner,
    e2e_bucket_name: str,
    live_bucket_config,
) -> None:
    """@case-id LL-BC-TRANSFER_ACCELERATION-LIVE set -> get with restoration."""
    from conftest import skip_on_live_error  # type: ignore[import-not-found]

    original_config = live_bucket_config.get_optional(ROOT, e2e_bucket_name)

    try:
        set_result = cli_runner.run(
            ["ve-tos", ROOT, "set", "--bucket", e2e_bucket_name, "--config",
             '{"TransferAccelerationConfiguration":{"Enabled":"true"}}']
        )
        if set_result.exit_code != 0:
            skip_on_live_error("ve-tos transfer-acceleration set", set_result)
        envelope_assert.assert_success_envelope(set_result.require_envelope())

        get_result = cli_runner.run(["ve-tos", ROOT, "get", "--bucket", e2e_bucket_name])
        if get_result.exit_code != 0:
            skip_on_live_error("ve-tos transfer-acceleration get", get_result)
        envelope_assert.assert_success_envelope(get_result.require_envelope())
        body = get_result.payload()["body"]
        # TOS returns TransferAccelerationConfiguration with Enabled field
        config = (
            body.get("TransferAccelerationConfiguration")
            or body.get("transfer_acceleration_configuration")
        )
        if config:
            enabled = config.get("Enabled") or config.get("enabled")
            assert enabled == "true", f"expected 'true', got {enabled!r} (body={body!r})"
        else:
            # Fallback: check top-level or rules-based response
            rules = (
                body.get("TransferAccelerationRules")
                or body.get("transfer_acceleration_rules")
            )
            if rules:
                status = rules[0].get("Status") or rules[0].get("status")
            else:
                status = body.get("status") or body.get("Status") or body.get("Enabled") or body.get("enabled")
            assert status in ("Enabled", "true"), f"expected Enabled/true, got {status!r} (body={body!r})"

        set_back = cli_runner.run(
            ["ve-tos", ROOT, "set", "--bucket", e2e_bucket_name, "--config",
             '{"TransferAccelerationConfiguration":{"Enabled":"false"}}']
        )
        if set_back.exit_code != 0:
            skip_on_live_error("ve-tos transfer-acceleration set (restore)", set_back)
        envelope_assert.assert_success_envelope(set_back.require_envelope())

        get_after = cli_runner.run(["ve-tos", ROOT, "get", "--bucket", e2e_bucket_name])
        assert get_after.exit_code == 0, get_after.stderr
        body_after = get_after.payload()["body"]
        config_after = (
            body_after.get("TransferAccelerationConfiguration")
            or body_after.get("transfer_acceleration_configuration")
        )
        if config_after:
            enabled_after = config_after.get("Enabled") or config_after.get("enabled")
            assert enabled_after == "false", f"expected 'false', got {enabled_after!r} (body={body_after!r})"
        else:
            rules_after = (
                body_after.get("TransferAccelerationRules")
                or body_after.get("transfer_acceleration_rules")
            )
            if rules_after:
                status_after = rules_after[0].get("Status") or rules_after[0].get("status")
            else:
                status_after = body_after.get("status") or body_after.get("Status") or body_after.get("Enabled") or body_after.get("enabled")
            assert status_after in ("Suspended", "false"), f"expected Suspended/false, got {status_after!r} (body={body_after!r})"
    finally:
        if original_config is not None:
            live_bucket_config.set_config(ROOT, e2e_bucket_name, original_config)

