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

"""Black-box E2E coverage for ``ve-tos turbo`` low-level APIs.

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

ROOT = "turbo"
EXPECTED_VALIDATION_FAILURES = {
    # The current CLI metadata exposes only --config/--content-md5, while the
    # handler still requires skipped fields; assert this black-box behavior
    # instead of silently dropping the leaf from coverage.
    "ve-tos real-time-log set": "requires --use-service-topic",
}


def _extract_turbo_token(result: CommandResult) -> str:
    token = _extract_optional_turbo_token(result)
    assert token, f"OpenTurbo response did not include a turbo token: {result.payload()!r}"
    return token


def _extract_optional_turbo_token(result: CommandResult) -> str | None:
    payload = result.payload()
    headers = payload.get("headers", {}) if isinstance(payload, dict) else {}
    for header_name, header_value in headers.items():
        if header_name.lower() == "x-tos-turbo-token":
            token = str(header_value)
            assert token and token != "***REDACTED***", "OpenTurbo token must be returned unredacted"
            return token

    body = payload.get("body") if isinstance(payload, dict) else None
    return _find_turbo_token(body)


def _find_turbo_token(value: Any) -> str | None:
    if isinstance(value, dict):
        for key, item in value.items():
            lower_key = str(key).lower()
            if "turbo" in lower_key and "token" in lower_key and isinstance(item, str):
                return item
            nested = _find_turbo_token(item)
            if nested:
                return nested
    if isinstance(value, list):
        for item in value:
            nested = _find_turbo_token(item)
            if nested:
                return nested
    return None


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
def turbo_leaf_commands(cli_runner: CliRunner) -> list[dict[str, Any]]:
    result = cli_runner.run(["ve-tos", "capabilities", "--view", "tree"])
    assert result.exit_code == 0, result.stderr
    envelope_assert.assert_success_envelope(result.require_envelope())
    return _root_leaf_commands(result.payload()["commands"])


@pytest.mark.slow
def test_turbo_all_leaf_commands_exercise_full_parameter_surface_dry_run(
    cli_runner: CliRunner,
    turbo_leaf_commands: list[dict[str, Any]],
    tmp_path: Path,
) -> None:
    """@case-id LL-CORE-TURBO-SURFACE every leaf operation and parameter is asserted."""

    failures: list[str] = []
    total_cases = 0

    for leaf in turbo_leaf_commands:
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

    assert total_cases >= len(turbo_leaf_commands)
    assert not failures, "\n".join(failures[:80])


@pytest.mark.slow
def test_turbo_leaf_operations_chain_in_metadata_order_dry_run(
    cli_runner: CliRunner,
    turbo_leaf_commands: list[dict[str, Any]],
    tmp_path: Path,
) -> None:
    """@case-id LL-CORE-TURBO-CHAIN all operations for this root are asserted in one flow."""

    seen_commands: list[str] = []
    failures: list[str] = []

    for leaf in turbo_leaf_commands:
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

    expected_commands = {str(leaf["command"]) for leaf in turbo_leaf_commands}
    assert set(seen_commands) == expected_commands
    assert not failures, "\n".join(failures[:80])

@pytest.mark.destructive
def test_turbo_list_live_chain(
    cli_runner: CliRunner,
    e2e_bucket_name: str,
) -> None:
    """@case-id LL-CORE-TURBO-LIVE list turbo sessions on reusable bucket."""
    from conftest import skip_on_live_error  # type: ignore[import-not-found]

    list_result = cli_runner.run(
        ["ve-tos", ROOT, "list", "--bucket", e2e_bucket_name, "--key", "ve-tos-cli-e2e-turbo-probe"]
    )
    if list_result.exit_code == envelope_assert.EXIT_CODES["resource_not_found"]:
        envelope_assert.assert_failure_envelope(
            list_result.require_envelope(), expected_kind="resource_not_found"
        )
        return
    if list_result.exit_code != 0:
        skip_on_live_error("ve-tos turbo list", list_result)
    envelope_assert.assert_success_envelope(list_result.require_envelope())


@pytest.mark.destructive
def test_turbo_open_append_close_live_chain(
    cli_runner: CliRunner,
    e2e_bucket_name: str,
    tmp_path: Path,
) -> None:
    """@case-id LL-CORE-TURBO-LIFECYCLE-LIVE open -> append -> close."""
    from conftest import skip_on_live_error  # type: ignore[import-not-found]

    key = "ve-tos-cli-e2e-turbo-lifecycle/test.bin"
    body_file = tmp_path / "turbo-data.bin"
    body_file.write_bytes(b"turbo-e2e-data" * 100)

    open_result = cli_runner.run(
        ["ve-tos", ROOT, "open", "--bucket", e2e_bucket_name, "--key", key,
         "--content-type", "application/octet-stream"]
    )
    if open_result.exit_code != 0:
        skip_on_live_error("ve-tos turbo open", open_result)
    envelope_assert.assert_success_envelope(open_result.require_envelope())
    turbo_token = _extract_turbo_token(open_result)

    try:
        append_result = cli_runner.run(
            ["ve-tos", ROOT, "append", "--bucket", e2e_bucket_name, "--key", key,
             "--body", str(body_file), "--turbo-token", turbo_token]
        )
        if append_result.exit_code != 0:
            skip_on_live_error("ve-tos turbo append", append_result)
        envelope_assert.assert_success_envelope(append_result.require_envelope())
        turbo_token = _extract_optional_turbo_token(append_result) or turbo_token

        close_result = cli_runner.run(
            ["ve-tos", ROOT, "close", "--bucket", e2e_bucket_name, "--key", key,
             "--turbo-token", turbo_token]
        )
        if close_result.exit_code != 0:
            skip_on_live_error("ve-tos turbo close", close_result)
        envelope_assert.assert_success_envelope(close_result.require_envelope())
    finally:
        cli_runner.run(
            [
                "ve-tos",
                "object",
                "delete",
                "--bucket",
                e2e_bucket_name,
                "--key",
                key,
                "--force",
                "--confirm",
                f"tos://{e2e_bucket_name}/{key}",
            ]
        )
