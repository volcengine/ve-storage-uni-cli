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

"""BC bucket-configuration command matrix.

The matrix executes create/get/list/delete-style bucket configuration commands via the
real CLI. It uses dry-run for broad parameter coverage and a real temporary bucket for
safe read-only probes.
"""

from __future__ import annotations

import pytest

from _lib import CliRunner, CommandCase, envelope_assert


CONFIG_JSON = '{"Rules":[]}'
POLICY_JSON = '{"Statement":[]}'
VERSIONING_JSON = '{"Status":"Suspended"}'
TAGGING_JSON = '{"TagSet":[{"Key":"purpose","Value":"e2e"}]}'
ACL_JSON = '{"Owner":{},"Grants":[]}'


BC_CASES = [
    # @case-id BC-001.D5.json-input
    CommandCase.build("BC-001.D5.json-input", "BC", ["ve-tos", "quota", "set", "--config", '{"Quota":1048576}']),
    # @case-id BC-002.D5.json-input
    CommandCase.build("BC-002.D5.json-input", "BC", ["ve-tos", "policy", "set", "--config", POLICY_JSON]),
    # @case-id BC-002.NEG.force
    CommandCase.build(
        "BC-002.NEG.force",
        "BC",
        ["ve-tos", "policy", "delete", "--force", "--confirm", "tos://dry-run-bucket"],
    ),
    # @case-id BC-003.D5.json-input
    CommandCase.build("BC-003.D5.json-input", "BC", ["ve-tos", "lifecycle", "set", "--config", CONFIG_JSON]),
    # @case-id BC-003.NEG.force
    CommandCase.build(
        "BC-003.NEG.force",
        "BC",
        ["ve-tos", "lifecycle", "delete", "--force", "--confirm", "tos://dry-run-bucket"],
    ),
    # @case-id BC-004.D5.json-input
    CommandCase.build("BC-004.D5.json-input", "BC", ["ve-tos", "cors", "set", "--config", CONFIG_JSON]),
    # @case-id BC-004.D5.md5
    CommandCase.build(
        "BC-004.D5.md5",
        "BC",
        ["ve-tos", "cors", "set", "--config", CONFIG_JSON, "--content-md5", "deadbeef"],
    ),
    # @case-id BC-005.D4.storage-class
    CommandCase.build(
        "BC-005.D4.storage-class",
        "BC",
        ["ve-tos", "storageclass", "set", "--storage-class", "IA"],
    ),
    # @case-id BC-006.D2.toggle
    CommandCase.build("BC-006.D2.toggle", "BC", ["ve-tos", "versioning", "set", "--config", VERSIONING_JSON]),
    # @case-id BC-007.D5.json-input
    CommandCase.build("BC-007.D5.json-input", "BC", ["ve-tos", "encryption", "set", "--config", CONFIG_JSON]),
    # @case-id BC-008.D5.json-input
    CommandCase.build("BC-008.D5.json-input", "BC", ["ve-tos", "tagging", "set", "--config", TAGGING_JSON]),
    # @case-id BC-009.D5.json-input
    CommandCase.build("BC-009.D5.json-input", "BC", ["ve-tos", "acl", "set", "--config", ACL_JSON]),
    # @case-id BC-010.D2.toggle
    CommandCase.build("BC-010.D2.toggle", "BC", ["ve-tos", "rename", "set", "--config", '{"Enabled":true}']),
    # @case-id BC-011.D2.toggle
    CommandCase.build(
        "BC-011.D2.toggle",
        "BC",
        ["ve-tos", "transfer-acceleration", "set", "--config", '{"Status":"Suspended"}'],
    ),
    # @case-id BC-012.D2.toggle
    CommandCase.build("BC-012.D2.toggle", "BC", ["ve-tos", "trash", "set", "--config", '{"Status":"Disabled"}']),
    # @case-id BC-013.D2.toggle
    CommandCase.build("BC-013.D2.toggle", "BC", ["ve-tos", "payment", "set", "--config", '{"Payer":"BucketOwner"}']),
    # @case-id BC-014.D5.json-input
    CommandCase.build("BC-014.D5.json-input", "BC", ["ve-tos", "logging", "set", "--config", CONFIG_JSON]),
    # @case-id BC-015.D5.json-input
    CommandCase.build("BC-015.D5.json-input", "BC", ["ve-tos", "max-age", "set", "--config", '{"MaxAge":60}']),
]


@pytest.mark.parametrize("case", BC_CASES, ids=lambda c: c.case_id)
def test_bucket_config_full_parameter_dry_run(
    cli_runner: CliRunner, fresh_bucket_name: str, case: CommandCase
) -> None:
    args = ["--dry-run", *case.args, "--bucket", fresh_bucket_name]
    result = cli_runner.run(args)
    assert result.exit_code == 0, (
        f"{case.case_id} failed: exit={result.exit_code}\nstdout={result.stdout}\nstderr={result.stderr}"
    )
    envelope_assert.assert_success_envelope(result.require_envelope())
    assert result.payload()["dry_run"] is True


@pytest.mark.destructive
def test_bucket_config_read_flow_on_temp_bucket(cli_runner: CliRunner, temp_bucket: str) -> None:
    """@case-id BC-020.D1.read-flow get/list style commands are grouped and cleaned by fixture."""
    read_cases = [
        ["ve-tos", "quota", "get", "--bucket", temp_bucket],
        ["ve-tos", "versioning", "get", "--bucket", temp_bucket],
        ["ve-tos", "acl", "get", "--bucket", temp_bucket],
        ["ve-tos", "tagging", "get", "--bucket", temp_bucket],
        ["ve-tos", "cors", "get", "--bucket", temp_bucket],
        ["ve-tos", "inventory", "list", "--bucket", temp_bucket],
    ]
    for args in read_cases:
        result = cli_runner.run(args)
        assert result.exit_code in (0, envelope_assert.EXIT_CODES["resource_not_found"]), (
            f"{' '.join(args)} failed unexpectedly: exit={result.exit_code}\n"
            f"stdout={result.stdout}\nstderr={result.stderr}"
        )
        if result.exit_code == 0:
            envelope_assert.assert_success_envelope(result.require_envelope())
