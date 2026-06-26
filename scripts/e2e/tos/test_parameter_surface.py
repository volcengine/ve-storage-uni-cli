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

"""Parameter-surface coverage checks.

These tests turn the metadata-driven dry-run matrix into a guardrail: if an in-scope
command gains a new CLI parameter, the generated surface builder must cover it or the
suite fails immediately.
"""

from __future__ import annotations

from _lib import CliRunner
from _lib.registry import (
    TESTED_UTILITY_ROOTS,
    UTILITY_ROOTS,
    command_root,
    commands_by_plan_group,
    flatten_commands,
    is_user_requested_e2e_root,
    leaf_commands,
    plan_group_for_root,
)
from _lib.surface_matrix import (
    build_surface_cases,
    in_scope_leaf_commands,
    manual_utility_parameter_coverage,
    parameter_keys,
)


def test_full_surface_matrix_covers_declared_parameters(cli_runner: CliRunner, tmp_path) -> None:
    """@case-id GP-100 Every in-scope command parameter is passed by at least one E2E case."""

    result = cli_runner.run(["ve-tos", "capabilities", "--view", "tree"])
    assert result.exit_code == 0, result.stderr
    manual_coverage = manual_utility_parameter_coverage()
    failures: list[str] = []
    for leaf in in_scope_leaf_commands(result.payload()["commands"]):
        command = leaf["command"]
        root = command_root(command)
        if root in TESTED_UTILITY_ROOTS:
            covered = manual_coverage.get(command, set())
        else:
            case_tmp = tmp_path / command.replace(" ", "-")
            case_tmp.mkdir(parents=True, exist_ok=True)
            covered = set()
            for case in build_surface_cases(leaf, case_tmp):
                covered.update(case.covered_parameters)
        missing = sorted(parameter_keys(leaf) - covered)
        if missing:
            failures.append(f"{command} E2E matrix misses parameters: {missing}")
    assert not failures, "\n".join(failures[:80])


def test_utilities_scope_is_limited_to_config_and_api(cli_runner: CliRunner) -> None:
    """@case-id AGT-001.D1.scope Utilities E2E intentionally excludes non config/api lines."""
    result = cli_runner.run(["ve-tos", "capabilities", "--view", "tree"])
    assert result.exit_code == 0, result.stderr
    leaves = leaf_commands(result.payload()["commands"])
    utility_roots = {command_root(leaf["command"]) for leaf in leaves if command_root(leaf["command"]) in UTILITY_ROOTS}
    assert TESTED_UTILITY_ROOTS.issubset(utility_roots)
    assert all(
        is_user_requested_e2e_root(root) == (root in TESTED_UTILITY_ROOTS)
        for root in utility_roots
    )


def test_all_registry_leaf_commands_are_classified_for_e2e_scope(cli_runner: CliRunner) -> None:
    """@case-id GP-101 Every non-excluded registry leaf maps to exactly one plan group."""
    result = cli_runner.run(["ve-tos", "capabilities", "--view", "tree"])
    assert result.exit_code == 0, result.stderr
    leaves = leaf_commands(result.payload()["commands"])
    included = [leaf for leaf in leaves if is_user_requested_e2e_root(command_root(leaf["command"]))]
    unknown = [
        leaf["command"]
        for leaf in included
        if plan_group_for_root(command_root(leaf["command"])) == "UNKNOWN"
    ]
    assert not unknown, f"unclassified E2E commands: {unknown[:20]}"

    grouped = commands_by_plan_group(included)
    expected_groups = {"AGT", "BB", "BC", "CFG", "CTL", "DP", "HL", "OBJ", "OS"}
    missing_groups = expected_groups - grouped.keys()
    assert not missing_groups, f"missing E2E plan groups: {sorted(missing_groups)}"


def test_all_in_scope_leaf_commands_expose_parameter_metadata(cli_runner: CliRunner) -> None:
    """@case-id GP-102 Full parameter surface is audited from the registry SSOT."""
    result = cli_runner.run(["ve-tos", "capabilities", "--view", "tree"])
    assert result.exit_code == 0, result.stderr
    leaves = leaf_commands(result.payload()["commands"])
    failures: list[str] = []
    for leaf in leaves:
        root = command_root(leaf["command"])
        if not is_user_requested_e2e_root(root):
            continue
        for parameter in leaf.get("parameters") or []:
            has_name = isinstance(parameter.get("name"), str) and bool(parameter["name"])
            has_shape = parameter.get("positional") or parameter.get("long")
            if not has_name or not has_shape:
                failures.append(f"{leaf['command']} invalid parameter metadata: {parameter!r}")
    assert not failures, "\n".join(failures[:50])
