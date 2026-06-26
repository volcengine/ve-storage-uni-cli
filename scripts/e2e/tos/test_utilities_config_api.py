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

"""CFG/AGT utility coverage.

Per the test-plan scope, Utilities covers only ``config`` and ``api`` in E2E.
"""

from __future__ import annotations

from _lib import CliRunner, TosCredentials, envelope_assert, unique_suffix


def test_config_init_set_show_uses_isolated_home(
    cli_runner: CliRunner, tos_credentials: TosCredentials
) -> None:
    """@case-id CFG-001.D5.path config flow must be idempotent and isolated by HOME."""
    profile = f"e2e-{unique_suffix(8)}"
    init = cli_runner.run(["ve-tos", "config", "init", "--profile", profile])
    assert init.exit_code == 0, init.stderr
    envelope_assert.assert_success_envelope(init.require_envelope())
    config_path = init.payload()["config_path"]
    assert "/.tos/config.toml" in config_path

    setters = [
        ("region", tos_credentials.region),
        ("endpoint", tos_credentials.endpoint),
        ("control_endpoint", tos_credentials.endpoint.replace("tos-", "tos-control-", 1)),
    ]
    for key, value in setters:
        result = cli_runner.run(["ve-tos", "config", "set", f"{profile}.{key}", value])
        assert result.exit_code == 0, result.stderr
        envelope_assert.assert_success_envelope(result.require_envelope())

    show = cli_runner.run(["ve-tos", "config", "show", "--profile", profile])
    assert show.exit_code == 0, show.stderr
    envelope_assert.assert_success_envelope(show.require_envelope())
    payload = show.payload()
    assert isinstance(payload, dict)
    assert tos_credentials.secret_key not in show.stdout, "config show must redact secrets"


def test_api_describe_registered_command(cli_runner: CliRunner) -> None:
    """@case-id AGT-002.D6.json api describe returns command metadata for registry-backed APIs."""
    result = cli_runner.run(["ve-tos", "api", "bucket", "create", "--describe"])
    assert result.exit_code == 0, result.stderr
    envelope_assert.assert_success_envelope(result.require_envelope())
    payload = result.payload()
    assert payload["mode"] == "capability_metadata"
    assert payload["command"] == "ve-tos bucket create"
    parameters = payload["capability_row"]["parameters"]
    assert any(param.get("name") == "storage-class" for param in parameters)


def test_api_optional_request_and_force_flags_parse_in_describe_mode(cli_runner: CliRunner) -> None:
    """@case-id AGT-003.D6.flags raw api root flags must remain covered in safe describe mode."""

    result = cli_runner.run(
        [
            "ve-tos",
            "api",
            "bucket",
            "create",
            "--request",
            '{"Bucket":"dry-run-bucket"}',
            "--force",
            "--describe",
        ]
    )
    assert result.exit_code == 0, result.stderr
    envelope_assert.assert_success_envelope(result.require_envelope())
    payload = result.payload()
    assert payload["mode"] == "raw_passthrough_plan"
    assert payload["command"] == "ve-tos bucket create"
    # [Review Fix] CLI now returns "capability_row" instead of "command_metadata".
    assert "capability_row" in payload


def test_api_unknown_command_is_validation_error(cli_runner: CliRunner) -> None:
    """@case-id AGT-002.NEG.01 unknown raw API describe must fail deterministically."""
    result = cli_runner.run(["ve-tos", "api", "bucket", "DefinitelyMissing", "--describe"])
    assert result.exit_code == envelope_assert.EXIT_CODES["validation_error"]
    envelope_assert.assert_failure_envelope(result.require_envelope(), expected_kind="validation_error")
