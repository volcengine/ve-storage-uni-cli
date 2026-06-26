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

"""Bucket Lifecycle + Safe Execution：

- create / head / list / delete 一条 round-trip
- 非交互模式下不带 ``--force`` 删除桶必须被 ``enforce_registry_guards`` 拒绝
  （[Review Fix #6] 的真实环境验证）。
"""

from __future__ import annotations

import pytest

from _lib import CliRunner, envelope_assert
from _lib.runner import unique_suffix
from conftest import skip_on_live_error  # type: ignore[import-not-found]


@pytest.mark.destructive
def test_bucket_create_head_delete(cli_runner: CliRunner, fresh_bucket_name: str) -> None:
    """@case-id BB-001.D1.uri 完整生命周期：create → head/stat/info/location → list → delete."""
    create = cli_runner.run(["ve-tos", "bucket", "create", f"tos://{fresh_bucket_name}"])
    if create.exit_code == envelope_assert.EXIT_CODES["transfer_failed"]:
        skip_on_live_error("ve-tos bucket create", create)
    assert create.exit_code == 0, create.stderr
    envelope_assert.assert_success_envelope(create.require_envelope())

    try:
        for action in ("head", "stat", "info", "location"):
            result = cli_runner.run(["ve-tos", "bucket", action, f"tos://{fresh_bucket_name}"])
            assert result.exit_code == 0, f"{action} failed: {result.stderr}"
            envelope_assert.assert_success_envelope(result.require_envelope())

        listed = cli_runner.run(["ve-tos", "bucket", "list"])
        assert listed.exit_code == 0
        # 只确认 bucket list 的 envelope 形状正确；具体 bucket 是否被列出受 ListBuckets
        # 一致性窗口影响，不在这里强断言。
        envelope_assert.assert_success_envelope(listed.require_envelope())
    finally:
        delete = cli_runner.run(
            [
                "ve-tos",
                "bucket",
                "delete",
                "--bucket",
                fresh_bucket_name,
                "--force",
                "--confirm",
                f"tos://{fresh_bucket_name}",
            ]
        )
        assert delete.exit_code == 0, (
            f"failed to clean up bucket: exit={delete.exit_code}, stderr={delete.stderr}"
        )


@pytest.mark.destructive
def test_bucket_delete_without_force_in_non_tty_is_rejected(
    cli_runner: CliRunner, temp_bucket: str
) -> None:
    """@case-id BB-001.NEG.01 真实执行必须显式 --force；subprocess 仅用于避免交互输入。

    断言：
        1. 退出码非 0；
        2. 错误为结构化 Envelope，且 kind=validation_error；
        3. bucket 仍存在（head 返回 0），证明 guard 真的拦截了。
    """
    result = cli_runner.run(["ve-tos", "bucket", "delete", "--bucket", temp_bucket])
    assert result.exit_code != 0, (
        f"expected guard to reject delete without --force, but it succeeded; "
        f"stdout={result.stdout[:300]}"
    )
    env = result.require_envelope()
    envelope_assert.assert_failure_envelope(env, expected_kind="validation_error")

    # 复检：bucket 应仍然存在
    head = cli_runner.run(["ve-tos", "bucket", "head", "--bucket", temp_bucket])
    assert head.exit_code == 0, (
        f"guard claimed to block deletion but bucket disappeared; head.exit={head.exit_code}"
    )


def test_head_nonexistent_bucket_is_404(cli_runner: CliRunner) -> None:
    """@case-id BB-002.NEG.01 不存在 bucket 应稳定映射 404。"""
    nonexistent = f"ve-tos-cli-e2e-ghost-{unique_suffix(16)}"
    result = cli_runner.run(["ve-tos", "bucket", "head", "--bucket", nonexistent])
    if result.exit_code == envelope_assert.EXIT_CODES["transfer_failed"]:
        skip_on_live_error("ve-tos bucket head", result)
    assert result.exit_code == envelope_assert.EXIT_CODES["resource_not_found"]


def test_bucket_create_all_optional_parameters_dry_run(cli_runner: CliRunner) -> None:
    """@case-id BB-001.D4.ALL 覆盖 create/list/delete 的可选参数且不产生副作用。"""
    create = cli_runner.run(
        [
            "--dry-run",
            "ve-tos",
            "bucket",
            "create",
            "tos://dry-run-bucket",
            "--region",
            "cn-beijing",
            "--storage-class",
            "IA",
            "--bucket-type",
            "hns",
            "--bucket-object-lock-enabled",
            "--acl",
            "private",
            "--grant-full-control",
            "id=owner",
            "--grant-read",
            "id=reader",
            "--grant-read-non-list",
            "id=reader",
            "--grant-read-acp",
            "id=reader",
            "--grant-write",
            "id=writer",
            "--grant-write-acp",
            "id=writer",
            "--az-redundancy",
            "multi-az",
            "--tagging",
            "purpose=e2e",
        ]
    )
    assert create.exit_code == 0, create.stderr
    assert create.payload()["dry_run"] is True

    delete = cli_runner.run(
        ["--dry-run", "ve-tos", "bucket", "delete", "tos://dry-run-bucket", "--destroy"]
    )
    assert delete.exit_code == 0, delete.stderr
    assert delete.payload()["dry_run"] is True
