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

"""Agent utility audit coverage for TOS.

These tests are not part of the non-Utility full-parameter matrix, but they
guard the six-principle/agent ecosystem surfaces that must remain functional:
doctor principles, domain-scoped skill metadata, MCP serve planning, and
completion generation.
"""

from __future__ import annotations

from _lib import CliRunner, envelope_assert


def test_doctor_principles_reports_domain_skill_coverage(cli_runner: CliRunner) -> None:
    """@case-id AGT-010.D6.principles doctor validates discovery and skill domains."""

    result = cli_runner.run(["ve-tos", "doctor", "--check", "principles"])
    assert result.exit_code == 0, result.stderr
    envelope_assert.assert_success_envelope(result.require_envelope())
    payload = result.payload()
    check = payload["checks"][0]
    assert check["status"] == "passed"
    details = check["details"]
    assert details["undiscoverable_leaf_commands"] == []
    assert details["uncovered_skill_domains"] == []
    assert set(details["skill_domains"]) >= {
        "tos-admin",
        "tos-bucket",
        "tos-bucket-config",
        "tos-shared",
        "tos-transfer",
    }


def test_skill_list_exposes_domain_metadata(cli_runner: CliRunner) -> None:
    """@case-id AGT-011.D6.skill skill metadata is domain-scoped."""

    result = cli_runner.run(["ve-tos", "skill", "list"])
    assert result.exit_code == 0, result.stderr
    envelope_assert.assert_success_envelope(result.require_envelope())
    skills = result.payload()["skills"]
    assert skills, "skill list must not be empty"
    cp_skill = next(skill for skill in skills if skill["command"] == "ve-tos cp")
    assert cp_skill["domain"] == "tos-transfer"
    assert cp_skill["input_schema"]["additionalProperties"] is False
    object_upload = next(skill for skill in skills if skill["command"] == "ve-tos object upload")
    assert object_upload["domain"] == "tos-bucket"
    assert object_upload["input_schema"]["additionalProperties"] is False


def test_serve_mcp_dry_run_reports_registry_plan(cli_runner: CliRunner) -> None:
    """@case-id AGT-012.D6.mcp serve --mcp dry-run must not start a long-running server."""

    result = cli_runner.run(["ve-tos", "serve", "--mcp", "--dry-run"])
    assert result.exit_code == 0, result.stderr
    envelope_assert.assert_success_envelope(result.require_envelope())
    payload = result.payload()
    assert payload["mode"] == "mcp"
    assert payload["status"] == "planned_not_started"
    assert payload["capabilities"] > 0


def test_completion_generation_uses_registry(cli_runner: CliRunner) -> None:
    """@case-id AGT-013.D6.completion completion includes registry-backed commands."""

    result = cli_runner.run(["ve-tos", "completion", "bash"])
    assert result.exit_code == 0, result.stderr
    envelope_assert.assert_success_envelope(result.require_envelope())
    payload = result.payload()
    assert payload["shell"] == "bash"
    assert "object" in payload["script"]
    assert payload["command_count"] > 0
