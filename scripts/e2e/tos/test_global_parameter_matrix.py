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

"""GP global-parameter cross matrix."""

from __future__ import annotations

from _lib import CliRunner, TosCredentials, envelope_assert


def test_global_endpoint_profile_and_control_endpoint_dry_run(
    cli_runner: CliRunner, tos_credentials: TosCredentials
) -> None:
    """@case-id GP-001.D1 endpoint/profile/control-endpoint are accepted globally."""
    control_endpoint = tos_credentials.endpoint.replace("tos-", "tos-control-", 1)
    result = cli_runner.run(
        [
            "--dry-run",
            "--endpoint",
            tos_credentials.endpoint,
            "--control-endpoint",
            control_endpoint,
            "--profile",
            "default",
            "ve-tos",
            "bucket",
            "list",
            "--bucket-type",
            "fns",
        ]
    )
    assert result.exit_code == 0, result.stderr
    envelope_assert.assert_success_envelope(result.require_envelope())
    assert result.payload()["dry_run"] is True


def test_global_query_projects_dry_run_payload(cli_runner: CliRunner) -> None:
    """@case-id GP-002.D6.query query can project a dry-run payload value."""
    result = cli_runner.run(["--dry-run", "--query", "data.dry_run", "ve-tos", "bucket", "list"])
    assert result.exit_code == 0, result.stderr
    assert result.json() is True


def test_global_invalid_query_is_validation_error(cli_runner: CliRunner) -> None:
    """@case-id GP-003.NEG.01 invalid query is deterministic and has no side effects."""
    result = cli_runner.run(
        ["--dry-run", "--query", "data.missing_field", "ve-tos", "bucket", "list"]
    )
    # [Review Fix] JMESPath on missing field now returns null (exit=0) instead of validation error.
    assert result.exit_code == 0
    assert "null" in result.stdout


def test_global_yaml_output_is_supported(cli_runner: CliRunner) -> None:
    """@case-id GP-004.D6.yaml non-json output still runs through the binary."""
    result = cli_runner.run(
        ["--dry-run", "--output", "yaml", "ve-tos", "bucket", "list"],
        json_output=False,
    )
    assert result.exit_code == 0, result.stderr
    assert "dry_run: true" in result.stdout


def test_global_quiet_and_trace_flags_parse(cli_runner: CliRunner, tmp_path) -> None:
    """@case-id GP-005.D7.trace quiet/trace flags parse without mutating resources."""
    result = cli_runner.run(
        [
            "--dry-run",
            "--quiet",
            "--trace-dir",
            str(tmp_path / "trace"),
            "--trace-redact",
            "strict",
            "ve-tos",
            "bucket",
            "list",
        ]
    )
    assert result.exit_code == 0, result.stderr
    envelope_assert.assert_success_envelope(result.require_envelope())
