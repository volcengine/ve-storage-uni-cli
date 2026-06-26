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

"""CTL/DP/OS advanced command-group E2E matrix."""

from __future__ import annotations

import pytest

from _lib import CliRunner, CommandCase, envelope_assert


GENERIC_ARGS = [
    "--name",
    "e2e-name",
    "--bucket",
    "e2e-bucket",
    "--id",
    "e2e-id",
    "--style-name",
    "style",
    "--job-id",
    "job",
    "--job-type",
    "image",
    "--alias",
    "alias",
    "--accelerator",
    "acc",
    "--accelerator-id",
    "acc-id",
    "--bucket-name",
    "e2e-bucket",
    "--domain",
    "example.com",
    "--az",
    "cn-beijing-a",
    "--region",
    "cn-beijing",
    "--resource-trn",
    "trn:e2e",
    "--tag-keys",
    "k1,k2",
    "--object-set-name",
    "object-set",
    "--object",
    "object-key",
    "--config",
    '{"name":"e2e"}',
    "--content-md5",
    "deadbeef",
    "--header",
    "x-test=v",
    "--force",
]


CASES = [
    # @case-id CTL-101.D1.generic
    CommandCase.build("CTL-101.D1.generic", "CTL", ["ve-tos", "control", "create-url-cache"]),
    # @case-id CTL-201.D1.generic
    CommandCase.build("CTL-201.D1.generic", "CTL", ["ve-tos", "mrap", "bind-accelerator"]),
    # @case-id CTL-301.D1.generic
    CommandCase.build("CTL-301.D1.generic", "CTL", ["ve-tos", "accelerator", "get"]),
    # @case-id CTL-401.D1.generic
    CommandCase.build("CTL-401.D1.generic", "CTL", ["ve-tos", "cap", "get"]),
    # @case-id CTL-411.D1.generic
    CommandCase.build("CTL-411.D1.generic", "CTL", ["ve-tos", "ap", "get"]),
    # @case-id CTL-501.D1.generic
    CommandCase.build("CTL-501.D1.generic", "CTL", ["ve-tos", "dataset", "get"]),
    # @case-id DP-001.D1.generic
    CommandCase.build("DP-001.D1.generic", "DP", ["ve-tos", "data-process", "get-image-style"]),
    # @case-id OS-001.D1.generic
    CommandCase.build("OS-001.D1.generic", "OS", ["ve-tos", "object-set", "get-global"]),
]


def _expected_action_prefix(args: tuple[str, ...]) -> str:
    # [Review Fix #E2E-VE-TOS-1] Envelope actions now keep the public top-level
    # command; do not alias ve-tos back to tos.
    return " ".join(args)


@pytest.mark.parametrize("case", CASES, ids=lambda c: c.case_id)
def test_advanced_groups_parse_full_generic_parameter_set(
    cli_runner: CliRunner, case: CommandCase
) -> None:
    result = cli_runner.run(["--dry-run", *case.args, *GENERIC_ARGS])
    assert result.exit_code == 0, (
        f"{case.case_id} failed: exit={result.exit_code}\nstdout={result.stdout}\nstderr={result.stderr}"
    )
    envelope_assert.assert_success_envelope(result.require_envelope())
    payload = result.payload()
    assert payload["dry_run"] is True
    assert payload["action"].startswith(_expected_action_prefix(case.args))
