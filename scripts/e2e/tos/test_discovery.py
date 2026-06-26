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

"""Discovery 原则：

- ``--capabilities`` 与 ``--describe`` 在配置真实凭证后仍能输出元数据，
  且元数据来自 registry（包含 ``low_level_apis``，对应 [Review Fix #6]）。
"""

from __future__ import annotations

from _lib import CliRunner, envelope_assert


def test_capabilities_returns_envelope(cli_runner: CliRunner) -> None:
    # [Review Fix #3] capabilities 是 ``tos`` 二进制下的子命令，不是顶层 flag；
    # 与 crates/tos/src/cli/mod.rs::Capabilities 对齐。
    result = cli_runner.run(["ve-tos", "capabilities"])
    assert result.exit_code == 0, result.stderr
    env = result.require_envelope()
    envelope_assert.assert_success_envelope(env)
    capabilities = env.get("data")
    assert isinstance(capabilities, (list, dict)) and capabilities, (
        "expected capabilities payload, got empty/none"
    )


def test_describe_high_level_command(cli_runner: CliRunner) -> None:
    """high-level 命令 ``--describe`` 必须暴露 ``low_level_apis``。"""
    # high-level 命令位于 ``tos`` 工具下；``cp`` 需要 positional 参数才能通过 clap。
    # ``--describe`` 走早返回路径，所以这里给占位 URI 但不会真触发 IO。
    result = cli_runner.run(
        ["ve-tos", "cp", "tos://placeholder/src", "tos://placeholder/dst", "--describe"]
    )
    assert result.exit_code == 0, result.stderr
    env = result.require_envelope()
    envelope_assert.assert_success_envelope(env)
    desc = env.get("data") or {}
    assert isinstance(desc, dict)
    apis = desc.get("low_level_apis")
    assert isinstance(apis, list) and apis, (
        f"high-level command 'cp' must list low_level_apis; got {apis!r}"
    )


def test_describe_low_level_command(cli_runner: CliRunner) -> None:
    """low-level 命令 ``--describe`` 至少应返回 command/usage 字段。"""
    result = cli_runner.run(["ve-tos", "bucket", "list", "--describe"])
    assert result.exit_code == 0, result.stderr
    desc = result.payload()
    assert "command" in desc, f"describe missing 'command' field: {desc!r}"
