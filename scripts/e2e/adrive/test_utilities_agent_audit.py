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

"""Agent utility audit coverage for ADrive."""

from __future__ import annotations

import dataclasses
from pathlib import Path

from _lib import CliRunner, envelope_assert


@dataclasses.dataclass(frozen=True)
class OfflineADriveCredentials:
    def as_env(self) -> dict[str, str]:
        return {
            "ADRIVE_ACCESS_KEY": "offline-ak",
            "ADRIVE_SECRET_KEY": "offline-sk",
            "ADRIVE_ENDPOINT": "https://ids.example.invalid",
            "ADRIVE_REGION": "cn-beijing",
        }


@dataclasses.dataclass(frozen=True)
class LegacyIdsOnlyCredentials:
    def as_env(self) -> dict[str, str]:
        legacy_prefix = "ID" + "S"
        return {
            f"{legacy_prefix}_ACCESS_KEY": "legacy-ak",
            f"{legacy_prefix}_SECRET_KEY": "legacy-sk",
            f"{legacy_prefix}_ENDPOINT": "https://ids.example.invalid",
            f"{legacy_prefix}_REGION": "cn-beijing",
        }


def offline_adrive_runner(tmp_path: Path) -> CliRunner:
    return CliRunner(creds=OfflineADriveCredentials(), home_override=tmp_path / "home")


def test_adrive_ignores_legacy_ids_environment_names(tmp_path: Path) -> None:
    runner = CliRunner(
        creds=LegacyIdsOnlyCredentials(),
        home_override=tmp_path / "home",
    )
    result = runner.run(["ve-adrive", "ls"], timeout=5)
    assert result.exit_code == 3, result.stderr
    envelope = result.require_envelope()
    assert envelope["status"] == "failed"
    assert envelope["error"]["kind"] == "config_missing"
    assert "ADRIVE_ACCESS_KEY" in envelope["error"]["message"]
    assert "ID" + "S_ACCESS_KEY" not in envelope["error"]["message"]


def test_adrive_doctor_principles_reports_domain_skill_coverage(
    tmp_path: Path,
) -> None:
    """@case-id ADRIVE-AGT-010 doctor validates registry and skill domains."""

    runner = offline_adrive_runner(tmp_path)
    result = runner.run(["ve-adrive", "doctor", "--check", "principles"])
    assert result.exit_code == 0, result.stderr
    envelope_assert.assert_success_envelope(result.require_envelope())
    check = result.payload()["checks"][0]
    assert check["status"] == "passed"
    details = check["details"]
    assert details["missing_domain"] == []
    assert details["exposed_unimplemented_low_level"] == []
    assert details["uncovered_skill_domains"] == []
    assert set(details["skill_domains"]) >= {
        "adrive-admin",
        "adrive-shared",
        "adrive-transfer",
    }


def test_adrive_skill_list_exposes_domain_metadata(tmp_path: Path) -> None:
    """@case-id ADRIVE-AGT-011 skill metadata is domain-scoped."""

    runner = offline_adrive_runner(tmp_path)
    result = runner.run(["ve-adrive", "skill", "list"])
    assert result.exit_code == 0, result.stderr
    envelope_assert.assert_success_envelope(result.require_envelope())
    skills = result.payload()["skills"]
    assert skills, "skill list must not be empty"
    list_skill = next(skill for skill in skills if skill["command"] == "ve-adrive ls")
    assert list_skill["domain"] == "adrive-transfer"
    assert list_skill["input_schema"]["additionalProperties"] is False


def test_adrive_serve_mcp_dry_run_reports_registry_plan(
    tmp_path: Path,
) -> None:
    """@case-id ADRIVE-AGT-012 serve dry-run must not start a long-running server."""

    runner = offline_adrive_runner(tmp_path)
    result = runner.run(["ve-adrive", "serve", "--mcp", "--transport", "stdio", "--dry-run"])
    assert result.exit_code == 0, result.stderr
    envelope_assert.assert_success_envelope(result.require_envelope())
    payload = result.payload()
    assert payload["mode"] == "mcp"
    assert payload["transport"] == "stdio"
    assert payload["status"] == "planned_not_started"
    assert payload["capabilities"] > 0


def test_adrive_doctor_mcp_reports_runtime_available(tmp_path: Path) -> None:
    """@case-id ADRIVE-AGT-015 doctor validates MCP runtime wiring."""

    runner = offline_adrive_runner(tmp_path)
    result = runner.run(["ve-adrive", "doctor", "--check", "mcp"])
    assert result.exit_code == 0, result.stderr
    envelope_assert.assert_success_envelope(result.require_envelope())
    check = result.payload()["checks"][0]
    assert check["status"] == "passed"
    assert check["details"]["runtime"] == "available"
    assert check["details"]["stdio_status"] == "available"
    assert check["details"]["sse_status"] == "available"


def test_adrive_serve_rejects_legacy_mode_alias(tmp_path: Path) -> None:
    """@case-id ADRIVE-AGT-016 serve only accepts TOS-aligned MCP parameters."""

    runner = offline_adrive_runner(tmp_path)
    result = runner.run(["ve-adrive", "serve", "--mode", "stdio", "--dry-run"])
    assert result.exit_code != 0
    assert "unexpected argument '--mode'" in (result.stderr + result.stdout)


def test_adrive_low_level_commands_are_not_exposed(tmp_path: Path) -> None:
    """@case-id ADRIVE-AGT-017 ve-adrive exposes only High-Level and Utilities."""

    runner = offline_adrive_runner(tmp_path)
    result = runner.run(["ve-adrive", "account", "--help"])
    assert result.exit_code != 0
    assert "unrecognized subcommand 'account'" in (result.stderr + result.stdout)


def test_adrive_completion_generation_uses_registry(
    tmp_path: Path,
) -> None:
    """@case-id ADRIVE-AGT-013 completion includes registry-backed commands."""

    runner = offline_adrive_runner(tmp_path)
    result = runner.run(["ve-adrive", "completion", "bash"])
    assert result.exit_code == 0, result.stderr
    envelope_assert.assert_success_envelope(result.require_envelope())
    payload = result.payload()
    assert payload["shell"] == "bash"
    assert "skill" in payload["script"]
    assert payload["command_count"] > 0


def test_adrive_api_passthrough_is_guarded(tmp_path: Path) -> None:
    """@case-id ADRIVE-AGT-014 api execution is reserved until Low-Level is implemented."""

    runner = offline_adrive_runner(tmp_path)
    result = runner.run(["ve-adrive", "api", "file", "list"])
    assert result.exit_code != 0
    assert "ADrive raw API execution is not implemented yet" in (result.stderr + result.stdout)

    force = runner.run(["ve-adrive", "api", "file", "list", "--force"])
    assert force.exit_code != 0
    assert "ADrive raw API execution is not implemented yet" in (force.stderr + force.stdout)

    plan = runner.run(["ve-adrive", "api", "file", "list", "--dry-run"])
    assert plan.exit_code == 0, plan.stderr
    envelope_assert.assert_success_envelope(plan.require_envelope())
    assert plan.payload()["status"] == "planned_not_executed"
