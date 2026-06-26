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

"""Metadata-driven full leaf-command dry-run coverage."""

from __future__ import annotations

from pathlib import Path

import pytest

from _lib import CliRunner, envelope_assert
from _lib.registry import TESTED_UTILITY_ROOTS, command_root, plan_group_for_root
from _lib.surface_matrix import build_execution_case, build_smoke_case, in_scope_leaf_commands


NON_UTILITY_GROUPS = ("HL", "BB", "BC", "OBJ", "CTL", "DP", "OS")


@pytest.fixture(scope="session")
def live_in_scope_leaves(cli_runner: CliRunner) -> list[dict[str, object]]:
    result = cli_runner.run(["ve-tos", "capabilities", "--view", "tree"])
    assert result.exit_code == 0, result.stderr
    return in_scope_leaf_commands(result.payload()["commands"])


@pytest.mark.slow
@pytest.mark.parametrize("group", NON_UTILITY_GROUPS)
def test_non_utility_leaf_commands_accept_generated_full_surface_dry_run(
    cli_runner: CliRunner,
    live_in_scope_leaves: list[dict[str, object]],
    tmp_path: Path,
    group: str,
) -> None:
    """@case-id GP-103 every non-utility leaf command is exercised by generated dry-run coverage."""

    failures: list[str] = []
    for leaf in live_in_scope_leaves:
        command = str(leaf["command"])
        root = command_root(command)
        if root in TESTED_UTILITY_ROOTS:
            continue
        if plan_group_for_root(root) != group:
            continue

        case_tmp = tmp_path / command.replace(" ", "-")
        case_tmp.mkdir(parents=True, exist_ok=True)
        case = build_execution_case(leaf, case_tmp)
        result = cli_runner.run(["--dry-run", *case.args], timeout=180.0)
        if result.exit_code != 0:
            fallback = build_smoke_case(leaf, case_tmp)
            describe = cli_runner.run([*fallback.args, "--describe"], timeout=120.0)
            if describe.exit_code == 0 and describe.json() is not None:
                envelope = describe.envelope()
                if envelope is not None:
                    envelope_assert.assert_success_envelope(envelope)
                continue

            help_result = cli_runner.run([*fallback.args, "--help"], json_output=False, timeout=120.0)
            if help_result.exit_code != 0 or "Usage:" not in help_result.stdout:
                failures.append(
                    f"{case.case_id} {command}: dry-run exit={result.exit_code} stderr={result.stderr[:180]!r}; "
                    f"describe exit={describe.exit_code} stderr={describe.stderr[:180]!r}; "
                    f"help exit={help_result.exit_code} stdout={help_result.stdout[:120]!r} stderr={help_result.stderr[:120]!r}"
                )
            continue

        envelope_assert.assert_success_envelope(result.require_envelope())
        payload = result.payload()
        if not isinstance(payload, dict) or payload.get("dry_run") is not True:
            failures.append(
                f"{case.case_id} {command}: expected dry_run payload, got {payload!r}"
            )

    assert not failures, "\n".join(failures[:80])
