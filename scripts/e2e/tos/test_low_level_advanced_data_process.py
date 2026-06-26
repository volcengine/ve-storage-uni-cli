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

"""Black-box E2E coverage for ``ve-tos data-process`` low-level APIs.

This script intentionally discovers leaf operations from the running binary via
``ve-tos capabilities --view tree`` and then executes only black-box CLI calls.
"""

from __future__ import annotations

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

ROOT = "data-process"
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
def data_process_leaf_commands(cli_runner: CliRunner) -> list[dict[str, Any]]:
    result = cli_runner.run(["ve-tos", "capabilities", "--view", "tree"])
    assert result.exit_code == 0, result.stderr
    envelope_assert.assert_success_envelope(result.require_envelope())
    return _root_leaf_commands(result.payload()["commands"])


@pytest.mark.slow
def test_data_process_all_leaf_commands_exercise_full_parameter_surface_dry_run(
    cli_runner: CliRunner,
    data_process_leaf_commands: list[dict[str, Any]],
    tmp_path: Path,
) -> None:
    """@case-id LL-ADV-DATA_PROCESS-SURFACE every leaf operation and parameter is asserted."""

    failures: list[str] = []
    total_cases = 0

    for leaf in data_process_leaf_commands:
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

    assert total_cases >= len(data_process_leaf_commands)
    assert not failures, "\n".join(failures[:80])


@pytest.mark.slow
def test_data_process_leaf_operations_chain_in_metadata_order_dry_run(
    cli_runner: CliRunner,
    data_process_leaf_commands: list[dict[str, Any]],
    tmp_path: Path,
) -> None:
    """@case-id LL-ADV-DATA_PROCESS-CHAIN all operations for this root are asserted in one flow."""

    seen_commands: list[str] = []
    failures: list[str] = []

    for leaf in data_process_leaf_commands:
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

    expected_commands = {str(leaf["command"]) for leaf in data_process_leaf_commands}
    assert set(seen_commands) == expected_commands
    assert not failures, "\n".join(failures[:80])

@pytest.mark.destructive
def test_data_process_control_plane_list_live_chain(
    cli_runner: CliRunner,
    e2e_bucket_name: str,
) -> None:
    """@case-id LL-ADV-DATA_PROCESS-LIVE control endpoint list probe."""

    result = cli_runner.run(
        [
            "ve-tos",
            "data-process",
            "list-image-styles",
            "--bucket",
            e2e_bucket_name,
            "--output",
            "json",
        ]
    )
    if result.exit_code != 0:
        skip_on_live_error("ve-tos data-process list-image-styles", result)
    envelope_assert.assert_success_envelope(result.require_envelope())
    assert isinstance(result.payload(), dict)


@pytest.mark.destructive
def test_data_process_image_style_crud_live_chain(
    cli_runner: CliRunner,
    e2e_bucket_name: str,
) -> None:
    """@case-id LL-ADV-DATA-PROCESS-IMAGE-STYLE set -> get -> list -> delete cycle."""

    import json

    style_name = "e2e-test-style"
    # A simple resize image style
    test_config = {
        "Content": "image/resize,w_100",
    }

    # Step 1: Set image style
    set_result = cli_runner.run([
        "ve-tos", ROOT, "set-image-style", "--bucket", e2e_bucket_name,
        "--style-name", style_name,
        "--config", json.dumps(test_config, separators=(",", ":")),
    ])
    if set_result.exit_code != 0:
        skip_on_live_error(f"ve-tos {ROOT} set-image-style", set_result)

    try:
        envelope_assert.assert_success_envelope(set_result.require_envelope())

        # Step 2: Get image style
        get_result = cli_runner.run([
            "ve-tos", ROOT, "get-image-style", "--bucket", e2e_bucket_name,
            "--style-name", style_name,
        ])
        if get_result.exit_code != 0:
            skip_on_live_error(f"ve-tos {ROOT} get-image-style", get_result)
        envelope_assert.assert_success_envelope(get_result.require_envelope())

        # Step 3: List image styles
        list_result = cli_runner.run([
            "ve-tos", ROOT, "list-image-styles", "--bucket", e2e_bucket_name,
        ])
        if list_result.exit_code != 0:
            skip_on_live_error(f"ve-tos {ROOT} list-image-styles", list_result)
        envelope_assert.assert_success_envelope(list_result.require_envelope())

        # Step 4: Delete image style
        del_result = cli_runner.run([
            "ve-tos", ROOT, "delete-image-style", "--bucket", e2e_bucket_name,
            "--style-name", style_name,
            "--force",
            "--confirm", f"tos://{e2e_bucket_name}",
        ])
        if del_result.exit_code != 0:
            skip_on_live_error(f"ve-tos {ROOT} delete-image-style", del_result)
        envelope_assert.assert_success_envelope(del_result.require_envelope())
    finally:
        # Best-effort cleanup
        cli_runner.run([
            "ve-tos", ROOT, "delete-image-style", "--bucket", e2e_bucket_name,
            "--style-name", style_name,
            "--force",
            "--confirm", f"tos://{e2e_bucket_name}",
        ])


@pytest.mark.destructive
def test_data_process_image_protect_rule_live_chain(
    cli_runner: CliRunner,
    e2e_bucket_name: str,
) -> None:
    """@case-id LL-ADV-DATA-PROCESS-PROTECT set -> get protect rule cycle."""

    import json

    # [Review Fix] API uses lowercase field names (discovered via live GET response)
    test_config = {
        "enable": True,
        "suffixes": ["*"],
    }

    # Step 1: Set protect rule
    set_result = cli_runner.run([
        "ve-tos", ROOT, "set-image-protect-rule", "--bucket", e2e_bucket_name,
        "--name", "e2e-protect",
        "--config", json.dumps(test_config, separators=(",", ":")),
    ])
    if set_result.exit_code != 0:
        skip_on_live_error(f"ve-tos {ROOT} set-image-protect-rule", set_result)
    envelope_assert.assert_success_envelope(set_result.require_envelope())

    # Step 2: Get protect rule
    get_result = cli_runner.run([
        "ve-tos", ROOT, "get-image-protect-rule", "--bucket", e2e_bucket_name,
    ])
    if get_result.exit_code != 0:
        skip_on_live_error(f"ve-tos {ROOT} get-image-protect-rule", get_result)
    envelope_assert.assert_success_envelope(get_result.require_envelope())


@pytest.mark.destructive
def test_data_process_image_style_separator_live_chain(
    cli_runner: CliRunner,
    e2e_bucket_name: str,
) -> None:
    """@case-id LL-ADV-DATA-PROCESS-SEPARATOR set -> get separator cycle."""

    import json

    test_config = {
        "Separator": ["-"],
    }

    # Step 1: Set separator
    set_result = cli_runner.run([
        "ve-tos", ROOT, "set-image-style-separator", "--bucket", e2e_bucket_name,
        "--config", json.dumps(test_config, separators=(",", ":")),
    ])
    if set_result.exit_code != 0:
        skip_on_live_error(f"ve-tos {ROOT} set-image-style-separator", set_result)
    envelope_assert.assert_success_envelope(set_result.require_envelope())

    # Step 2: Get separator
    get_result = cli_runner.run([
        "ve-tos", ROOT, "get-image-style-separator", "--bucket", e2e_bucket_name,
    ])
    if get_result.exit_code != 0:
        skip_on_live_error(f"ve-tos {ROOT} get-image-style-separator", get_result)
    envelope_assert.assert_success_envelope(get_result.require_envelope())


@pytest.mark.destructive
def test_data_process_workflow_crud_live_chain(
    cli_runner: CliRunner,
    e2e_bucket_name: str,
) -> None:
    """@case-id LL-ADV-DATA-PROCESS-WORKFLOW set -> get -> delete cycle."""

    import json

    test_config = {
        "Rules": [
            {
                "ID": "e2e-test-workflow",
                "Enabled": True,
                "Prefix": "workflow-input/",
                "Topology": [["op-1"]],
                "Operations": {
                    "AudioTranscode": [
                        {
                            "OperationID": "op-1",
                            "Format": "mp3",
                            "Bitrate": 128000,
                            "Output": {
                                "Bucket": e2e_bucket_name,
                                "Object": "workflow-output/${inputName}.mp3",
                            },
                        },
                    ],
                },
            },
        ],
    }

    # Step 1: Set workflow
    set_result = cli_runner.run([
        "ve-tos", ROOT, "set-workflow", "--bucket", e2e_bucket_name,
        "--config", json.dumps(test_config, separators=(",", ":")),
    ])
    if set_result.exit_code != 0:
        skip_on_live_error(f"ve-tos {ROOT} set-workflow", set_result)

    try:
        envelope_assert.assert_success_envelope(set_result.require_envelope())

        # Step 2: Get workflow
        get_result = cli_runner.run([
            "ve-tos", ROOT, "get-workflow", "--bucket", e2e_bucket_name,
        ])
        if get_result.exit_code != 0:
            skip_on_live_error(f"ve-tos {ROOT} get-workflow", get_result)
        envelope_assert.assert_success_envelope(get_result.require_envelope())

        # Step 3: Delete workflow
        del_result = cli_runner.run([
            "ve-tos", ROOT, "delete-workflow", "--bucket", e2e_bucket_name,
            "--force",
            "--confirm", f"tos://{e2e_bucket_name}",
        ])
        if del_result.exit_code != 0:
            skip_on_live_error(f"ve-tos {ROOT} delete-workflow", del_result)
        envelope_assert.assert_success_envelope(del_result.require_envelope())
    finally:
        cli_runner.run([
            "ve-tos", ROOT, "delete-workflow", "--bucket", e2e_bucket_name,
            "--force",
            "--confirm", f"tos://{e2e_bucket_name}",
        ])


@pytest.mark.destructive
def test_data_process_template_crud_live_chain(
    cli_runner: CliRunner,
    e2e_bucket_name: str,
) -> None:
    """@case-id LL-ADV-DATA-PROCESS-TEMPLATE set -> get -> delete cycle."""

    import json

    def template_id_from_payload(payload: dict[str, Any]) -> str:
        body = payload.get("body", payload)
        if isinstance(body, str):
            return body.strip()
        if isinstance(body, dict):
            return (
                body.get("template_id")
                or body.get("TemplateId")
                or body.get("TemplateID")
                or body.get("id")
                or body.get("Id")
                or body.get("raw")
                or ""
            )
        return ""

    template_id = ""
    test_config = {
        "Name": "e2e-test-template",
        "Tag": "Transcode",
        "TranscodeConfig": {
            "TimeInterval": {
                "Start": 0,
                "Duration": 10000,
            },
            "Container": {"Format": "mp4"},
            "Video": {
                "Codec": "h264",
                "Width": 480,
                "Height": 480,
                "Crf": 1,
                "PixFmt": "yuv420p",
                "BitRate": 3000000,
                "Fps": 24,
                "Remove": False,
            },
            "Audio": {
                "Codec": "aac",
                "BitRate": 128000,
                "SampleFormat": "fltp",
                "SampleRate": 48000,
                "Channels": 2,
                "Remove": False,
            },
        },
    }

    # Step 1: Set template
    set_result = cli_runner.run([
        "ve-tos", ROOT, "set-template", "--bucket", e2e_bucket_name,
        "--config", json.dumps(test_config, separators=(",", ":")),
    ])
    if set_result.exit_code != 0:
        skip_on_live_error(f"ve-tos {ROOT} set-template", set_result)

    try:
        envelope_assert.assert_success_envelope(set_result.require_envelope())
        template_id = template_id_from_payload(set_result.payload())
        assert template_id, f"expected template id in set-template response, got {set_result.payload()!r}"

        # Step 2: Get template
        get_result = cli_runner.run([
            "ve-tos", ROOT, "get-template", "--bucket", e2e_bucket_name,
            "--tag", "Transcode",
        ])
        if get_result.exit_code != 0:
            skip_on_live_error(f"ve-tos {ROOT} get-template", get_result)
        envelope_assert.assert_success_envelope(get_result.require_envelope())

        # Step 3: Delete template
        del_result = cli_runner.run([
            "ve-tos", ROOT, "delete-template", "--bucket", e2e_bucket_name,
            "--tag", "Transcode",
            "--id", template_id,
            "--force",
            "--confirm", f"tos://{e2e_bucket_name}",
        ])
        if del_result.exit_code != 0:
            skip_on_live_error(f"ve-tos {ROOT} delete-template", del_result)
        envelope_assert.assert_success_envelope(del_result.require_envelope())
    finally:
        # [Review Fix #DataProcessTemplateTag] Template get/delete require the
        # same service-side tag discriminator used during creation.
        if template_id:
            cli_runner.run([
                "ve-tos", ROOT, "delete-template", "--bucket", e2e_bucket_name,
                "--tag", "Transcode",
                "--id", template_id,
                "--force",
                "--confirm", f"tos://{e2e_bucket_name}",
            ])
