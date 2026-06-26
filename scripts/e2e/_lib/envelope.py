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

"""Envelope 与退出码的契约校验工具。"""

from __future__ import annotations

from typing import Any, Iterable, Mapping

VALID_STATUS = {"success", "failed"}
VALID_ERROR_KINDS = {
    "unknown",
    "auth_failed",
    "config_missing",
    "resource_not_found",
    "permission_denied",
    "validation_error",
    "rate_limited",
    "transfer_failed",
    "conflict",
}

EXIT_CODES = {
    "success": 0,
    "unknown": 1,
    "auth_failed": 2,
    "config_missing": 3,
    "resource_not_found": 4,
    "permission_denied": 5,
    "validation_error": 6,
    "rate_limited": 7,
    "transfer_failed": 8,
    "conflict": 9,
}


def assert_success_envelope(envelope: Mapping[str, Any], *, command_substr: str = "") -> None:
    """成功响应必须满足：status=success、command 非空、error 缺失或为 None。"""
    assert envelope.get("status") == "success", (
        f"expected status=success, got {envelope.get('status')!r}; envelope={envelope}"
    )
    cmd = envelope.get("command")
    assert isinstance(cmd, str) and cmd, f"expected non-empty command, got {cmd!r}"
    if command_substr:
        assert command_substr in cmd, f"expected '{command_substr}' in command, got {cmd!r}"
    err = envelope.get("error")
    assert err in (None, {}), f"success envelope must not carry error, got {err!r}"


def assert_failure_envelope(
    envelope: Mapping[str, Any],
    *,
    expected_kind: str,
) -> None:
    """失败响应必须包含结构化 error，且 kind 与预期一致。"""
    assert expected_kind in VALID_ERROR_KINDS, f"unknown error kind: {expected_kind}"
    assert envelope.get("status") == "failed", (
        f"expected status=failed, got {envelope.get('status')!r}; envelope={envelope}"
    )
    err = envelope.get("error")
    assert isinstance(err, dict), f"expected error dict, got {err!r}"
    assert err.get("kind") == expected_kind, (
        f"expected error.kind={expected_kind}, got {err.get('kind')!r}"
    )
    assert isinstance(err.get("message"), str) and err["message"], "error.message must be non-empty"
    assert isinstance(err.get("exit_code"), int), "error.exit_code must be int"
    assert err["exit_code"] == EXIT_CODES[expected_kind], (
        f"error.exit_code mismatch for kind={expected_kind}: "
        f"expected {EXIT_CODES[expected_kind]}, got {err['exit_code']}"
    )


def assert_exit_code(actual: int, expected_kind: str) -> None:
    expected = EXIT_CODES[expected_kind]
    assert actual == expected, f"expected exit code {expected} ({expected_kind}), got {actual}"


def envelope_data_iter(envelope: Mapping[str, Any]) -> Iterable[Any]:
    """统一 data 提取：兼容 dict / list / scalar。"""
    data = envelope.get("data")
    if data is None:
        return iter(())
    if isinstance(data, list):
        return iter(data)
    return iter([data])
