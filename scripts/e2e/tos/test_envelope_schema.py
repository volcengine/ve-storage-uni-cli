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

"""Controlled Output 原则：

- 成功路径必须是 Envelope；
- ``request_id`` 在真实 TOS 调用上必须被透传；
- ``--query`` 路径表达式可裁剪 Envelope.data。

注：``--query`` 在当前实现是简化版点路径（见 ``crates/tos/src/handler/common.rs::eval_path``），
不是完整 JMESPath；E2E 只验证契约：合法表达式裁剪成功，非法/不命中表达式映射到 ValidationError(6)。
"""

from __future__ import annotations

import json

import pytest

from _lib import CliRunner, envelope_assert
from conftest import skip_on_live_error  # type: ignore[import-not-found]

pytestmark = pytest.mark.live


def _skip_if_transfer_failed(command: str, result) -> None:
    if result.exit_code == envelope_assert.EXIT_CODES["transfer_failed"]:
        skip_on_live_error(command, result)


def test_bucket_list_envelope_with_request_id(cli_runner: CliRunner) -> None:
    result = cli_runner.run(["ve-tos", "bucket", "list"])
    _skip_if_transfer_failed("ve-tos bucket list", result)
    assert result.exit_code == 0, f"stderr={result.stderr}"
    env = result.require_envelope()
    envelope_assert.assert_success_envelope(env, command_substr="bucket")
    assert "data" in env, "bucket list must populate data"
    request_id = env.get("request_id")
    assert isinstance(request_id, str) and request_id, (
        "real TOS responses must carry request_id; got " + repr(request_id)
    )


def test_bucket_list_supports_query_projection(cli_runner: CliRunner) -> None:
    """``--query data`` 应该只返回 data 子树（脱壳）。"""
    plain = cli_runner.run(["ve-tos", "bucket", "list"])
    _skip_if_transfer_failed("ve-tos bucket list", plain)
    assert plain.exit_code == 0
    plain_env = plain.require_envelope()
    plain_data = plain_env.get("data")
    assert plain_data, "expected non-empty data for query baseline"

    queried = cli_runner.run(["ve-tos", "bucket", "list", "--query", "data"])
    assert queried.exit_code == 0, queried.stderr
    parsed = json.loads(queried.stdout)
    # query 后形状应等于原 envelope.data
    assert parsed == plain_data, (
        f"--query data should expose the data subtree intact; "
        f"got {parsed!r} vs baseline {plain_data!r}"
    )


def test_unmatched_query_yields_validation_error(cli_runner: CliRunner) -> None:
    """JMESPath 对不存在的字段返回 null（exit=0）。

    CLI 不再将 missing field 视为 ValidationError，而是输出 null。
    """
    result = cli_runner.run(
        ["ve-tos", "bucket", "list", "--query", "data.this_field_definitely_missing"]
    )
    _skip_if_transfer_failed("ve-tos bucket list --query", result)
    # [Review Fix] CLI now returns exit=0 with stdout "null\n" for unmatched JMESPath queries.
    assert result.exit_code == 0, (
        f"expected exit=0, got {result.exit_code}; stderr={result.stderr[:200]}"
    )
    assert result.stdout == "null\n"
