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

"""Object Lifecycle：upload → head → download → delete 的 round-trip。

通过比对 MD5 验证流式 I/O 的字节正确性。
"""

from __future__ import annotations

from pathlib import Path

import pytest

from _lib import CliRunner, envelope_assert
from conftest import md5_of  # type: ignore[import-not-found]


@pytest.mark.destructive
def test_object_round_trip(
    cli_runner: CliRunner,
    temp_bucket: str,
    random_payload: Path,
    tmp_path: Path,
) -> None:
    """1 MiB 随机数据完整 round-trip，并校验 MD5。"""
    key = "e2e/payload.bin"
    upload = cli_runner.run(
        [
            "ve-tos", "object", "upload",
            "--bucket", temp_bucket,
            "--key", key,
            "--body", str(random_payload),
        ]
    )
    assert upload.exit_code == 0, upload.stderr
    envelope_assert.assert_success_envelope(upload.require_envelope())

    head = cli_runner.run(
        ["ve-tos", "object", "head", "--bucket", temp_bucket, "--key", key]
    )
    assert head.exit_code == 0, head.stderr
    envelope_assert.assert_success_envelope(head.require_envelope())

    download_path = tmp_path / "downloaded.bin"
    # [Review Fix #2] ``object download`` 使用 ``--body`` 指定输出路径，
    # 不是 ``--output-file``（这是 high-level cp 的语义）；与 crates/tos/src/cli/low_level.rs
    # ObjectDownloadArgs 对齐。
    download = cli_runner.run(
        [
            "ve-tos", "object", "download",
            "--bucket", temp_bucket,
            "--key", key,
            "--body", str(download_path),
        ]
    )
    assert download.exit_code == 0, download.stderr

    assert download_path.exists(), "download did not produce file"
    expected = md5_of(random_payload)
    actual = md5_of(download_path)
    assert expected == actual, (
        f"MD5 mismatch after round-trip: expected={expected}, actual={actual}"
    )

    delete = cli_runner.run(
        [
            "ve-tos", "object", "delete",
            "--bucket", temp_bucket,
            "--key", key,
            "--force",
            "--confirm",
            f"tos://{temp_bucket}/{key}",
        ]
    )
    assert delete.exit_code == 0, delete.stderr


@pytest.mark.destructive
def test_object_head_nonexistent_returns_404(cli_runner: CliRunner, temp_bucket: str) -> None:
    result = cli_runner.run(
        ["ve-tos", "object", "head", "--bucket", temp_bucket, "--key", "definitely-not-here"]
    )
    assert result.exit_code == envelope_assert.EXIT_CODES["resource_not_found"], (
        f"expected exit=4, got {result.exit_code}; stderr={result.stderr[:300]}"
    )
