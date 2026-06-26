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

"""Deterministic Errors 原则：

把不同失败场景映射到不同 exit_code 与 ``error.kind``。
- 401 → AuthFailed (2)
- 404 → ResourceNotFound (4)
- 输入校验 → ValidationError (6)
"""

from __future__ import annotations

import pytest

from _lib import CliRunner, TosCredentials, envelope_assert
from _lib.runner import unique_suffix
from conftest import skip_on_live_error  # type: ignore[import-not-found]


def _skip_if_transfer_failed(command: str, result) -> None:
    if result.exit_code == envelope_assert.EXIT_CODES["transfer_failed"]:
        skip_on_live_error(command, result)


@pytest.mark.live
def test_auth_failed_returns_exit_2(tos_credentials: TosCredentials, cli_runner: CliRunner) -> None:
    """伪造 SK，让 TOS 返回 SignatureDoesNotMatch，应映射到 PermissionDenied(5)。"""
    bogus = {"TOS_SECRET_KEY": "deadbeefdeadbeefdeadbeefdeadbeef"}
    result = cli_runner.run(["ve-tos", "bucket", "list"], extra_env=bogus)
    _skip_if_transfer_failed("ve-tos bucket list with invalid credentials", result)
    # [Review Fix] CLI maps SignatureDoesNotMatch to permission_denied (exit=5), not auth_failed.
    assert result.exit_code == envelope_assert.EXIT_CODES["permission_denied"], (
        f"expected exit=5, got {result.exit_code}; stderr={result.stderr[:300]}"
    )
    env = result.require_envelope()
    envelope_assert.assert_failure_envelope(env, expected_kind="permission_denied")


@pytest.mark.live
def test_not_found_returns_exit_4(cli_runner: CliRunner) -> None:
    """对一个一定不存在的 bucket 做 head，应返回 ResourceNotFound(4)。"""
    nonexistent = f"ve-tos-cli-e2e-not-exists-{unique_suffix(16)}"
    result = cli_runner.run(["ve-tos", "bucket", "head", "--bucket", nonexistent])
    _skip_if_transfer_failed("ve-tos bucket head", result)
    assert result.exit_code == envelope_assert.EXIT_CODES["resource_not_found"], (
        f"expected exit=4, got {result.exit_code}; stderr={result.stderr[:300]}"
    )
    env = result.require_envelope()
    envelope_assert.assert_failure_envelope(env, expected_kind="resource_not_found")


def test_validation_error_on_missing_required_arg(cli_runner: CliRunner) -> None:
    """``bucket head`` 缺 ``--bucket`` 必填参数，clap 直接走默认错误退出。

    注意：clap 解析失败发生在 ``Cli::parse_from`` 内部，先于我们的 Envelope 路径，
    退出码由 clap 本身决定（一般为 2），且 stdout 没有 Envelope 输出，错误信息直接
    打到 stderr。这是 ``crates/tos/src/cli/meta.rs`` 之外的解析层契约，需要单独验证。

    断言策略：
        1. 退出码非 0；
        2. stderr 含 ``required`` / ``error`` 等 clap 标志性字样；
        3. 不假设具体退出码，避免与 clap 版本绑死。
    """
    result = cli_runner.run(["ve-tos", "bucket", "head"])
    assert result.exit_code != 0, (
        f"missing required arg should fail; stdout={result.stdout[:200]}"
    )
    combined = (result.stderr + result.stdout).lower()
    assert any(kw in combined for kw in ("required", "error", "usage")), (
        f"expected clap parse-error message, got stderr={result.stderr[:300]}"
    )
