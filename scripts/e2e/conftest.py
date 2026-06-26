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

"""Pytest fixtures：CliRunner / temp bucket / 临时文件工厂。

bucket 命名规则：
    {prefix}-{16-hex} 长度恰好 ≤ 63 且全小写，符合 TOS 约束。
"""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path
from typing import Any, Callable, Iterator

import pytest

THIS_DIR = Path(__file__).resolve().parent
if str(THIS_DIR) not in sys.path:
    sys.path.insert(0, str(THIS_DIR))

from _lib import (  # noqa: E402
    ADriveCredentials,
    CliRunner,
    E2EConfigError,
    TosCredentials,
    envelope_assert,
    md5_of,
    unique_suffix,
)

__all__ = ["md5_of", "skip_on_live_error"]


def pytest_configure(config: pytest.Config) -> None:
    config.addinivalue_line("markers", "slow: marks tests as slow (>30s)")
    config.addinivalue_line("markers", "destructive: tests mutating real bucket state")
    config.addinivalue_line("markers", "live: tests that require valid TOS credentials")
    config.addinivalue_line("markers", "adrive: tests that require valid ADrive credentials")


@pytest.fixture(scope="session")
def tos_credentials() -> TosCredentials:
    try:
        return TosCredentials.from_env()
    except E2EConfigError as exc:
        # [Review Fix #1] Preserve the config error in the skip reason instead of raising NameError.
        pytest.skip(f"TOS credentials not configured: {exc}")
        raise RuntimeError("pytest.skip should have aborted this fixture")


@pytest.fixture(scope="session")
def cli_runner(tos_credentials: TosCredentials, tmp_path_factory: pytest.TempPathFactory) -> CliRunner:
    home = tmp_path_factory.mktemp("e2e-home")
    return CliRunner(creds=tos_credentials, home_override=home)


@pytest.fixture(scope="session")
def adrive_credentials() -> ADriveCredentials:
    try:
        return ADriveCredentials.from_env()
    except E2EConfigError as exc:
        # [Review Fix #ADrive-E2E-DryRun] Non-live ADrive tests exercise
        # parsing/dry-run contracts and should not be skipped just because live
        # credentials are absent. Live tests still skip when real API setup fails.
        return ADriveCredentials(
            access_key="offline-ak",
            secret_key="offline-sk",
            endpoint="https://ids.example.invalid",
            region="cn-beijing",
            resource_prefix="tos-uni-adrive-e2e",
        )


@pytest.fixture(scope="session")
def adrive_cli_runner(
    adrive_credentials: ADriveCredentials,
    tmp_path_factory: pytest.TempPathFactory,
) -> CliRunner:
    home = tmp_path_factory.mktemp("adrive-e2e-home")
    return CliRunner(creds=adrive_credentials, home_override=home)


@pytest.fixture()
def fresh_bucket_name(tos_credentials: TosCredentials) -> str:
    suffix = unique_suffix(16)
    name = f"{tos_credentials.bucket_prefix}-{suffix}"
    # TOS bucket 命名: 3-63 字符，小写字母数字与 '-'
    assert 3 <= len(name) <= 63, f"bucket name out of range: {name}"
    return name


@pytest.fixture(scope="session")
def e2e_bucket_name(cli_runner: CliRunner, tos_credentials: TosCredentials) -> Iterator[str]:
    """Session-scoped bucket: 测试开始时自动创建，结束时自动清理删除。"""
    suffix = unique_suffix(16)
    bucket = f"{tos_credentials.bucket_prefix}-{suffix}"
    assert 3 <= len(bucket) <= 63, f"bucket name out of range: {bucket}"

    create = cli_runner.run(["ve-tos", "bucket", "create", "--bucket", bucket])
    if create.exit_code != 0:
        message = f"{create.stdout}\n{create.stderr}"
        if "TooManyBuckets" in message:
            pytest.skip(f"cannot create session bucket {bucket}: TooManyBuckets")
        pytest.skip(
            f"cannot create session bucket {bucket}: "
            f"exit={create.exit_code}, stderr={create.stderr[:300]}"
        )
    try:
        yield bucket
    finally:
        if os.environ.get("TOS_E2E_KEEP") == "1":
            print(
                f"[e2e_bucket_name] TOS_E2E_KEEP=1, leaving bucket {bucket} for inspection",
                file=sys.stderr,
            )
            return
        _cleanup_bucket(cli_runner, bucket)


class LiveBucketConfig:
    """Shared helpers for mutating reusable live bucket configuration safely."""

    def __init__(self, cli_runner: CliRunner) -> None:
        self._cli_runner = cli_runner

    def get_required(self, root: str, bucket: str) -> dict[str, Any]:
        result = self._cli_runner.run(["ve-tos", root, "get", "--bucket", bucket])
        assert result.exit_code == 0, result.stderr
        envelope_assert.assert_success_envelope(result.require_envelope())
        return result.payload()["body"]

    def get_optional(
        self,
        root: str,
        bucket: str,
        *,
        empty_field: str | None = None,
    ) -> dict[str, Any] | None:
        result = self._cli_runner.run(["ve-tos", root, "get", "--bucket", bucket])
        if result.exit_code == envelope_assert.EXIT_CODES["resource_not_found"]:
            return None
        assert result.exit_code == 0, result.stderr
        envelope_assert.assert_success_envelope(result.require_envelope())
        body = result.payload()["body"]
        if empty_field is not None and body.get(empty_field) is None:
            return None
        return body

    def set_config(self, root: str, bucket: str, config: dict[str, Any]) -> None:
        result = self._cli_runner.run(
            [
                "ve-tos",
                root,
                "set",
                "--bucket",
                bucket,
                "--config",
                json.dumps(config, separators=(",", ":")),
            ]
        )
        assert result.exit_code == 0, result.stderr
        envelope_assert.assert_success_envelope(result.require_envelope())

    def delete_config(
        self,
        root: str,
        bucket: str,
        *,
        allow_missing: bool = False,
    ) -> None:
        result = self._cli_runner.run(
            ["ve-tos", root, "delete", "--bucket", bucket, "--force", "--confirm", f"tos://{bucket}"]
        )
        allowed_codes = [0]
        if allow_missing:
            allowed_codes.append(envelope_assert.EXIT_CODES["resource_not_found"])
        assert result.exit_code in allowed_codes, result.stderr
        if result.exit_code == 0:
            envelope_assert.assert_success_envelope(result.require_envelope())

    def restore_optional(
        self,
        root: str,
        bucket: str,
        original_config: dict[str, Any] | None,
        *,
        normalize: Callable[[dict[str, Any]], dict[str, Any]] | None = None,
    ) -> None:
        if original_config is None:
            self.delete_config(root, bucket, allow_missing=True)
            return
        restored_config = normalize(original_config) if normalize else original_config
        self.set_config(root, bucket, restored_config)

    def restore_notification(self, bucket: str, original_config: dict[str, Any]) -> None:
        current_config = self.get_required("notification", bucket)
        self.set_config(
            "notification",
            bucket,
            {
                "Version": current_config.get("version", ""),
                "Rules": original_config.get("rules", []),
            },
        )

    def cleanup_inventory(self, bucket: str, inventory_id: str) -> None:
        result = self._cli_runner.run(
            [
                "ve-tos",
                "inventory",
                "delete",
                "--bucket",
                bucket,
                "--id",
                inventory_id,
                "--force",
                "--confirm",
                f"tos://{bucket}",
            ]
        )
        assert result.exit_code in (0, envelope_assert.EXIT_CODES["resource_not_found"])

    def set_inventory(self, bucket: str, inventory_id: str, config: dict[str, Any]) -> None:
        result = self._cli_runner.run(
            [
                "ve-tos",
                "inventory",
                "set",
                "--bucket",
                bucket,
                "--id",
                inventory_id,
                "--config",
                json.dumps(config, separators=(",", ":")),
            ]
        )
        assert result.exit_code == 0, result.stderr
        envelope_assert.assert_success_envelope(result.require_envelope())


@pytest.fixture()
def live_bucket_config(cli_runner: CliRunner) -> LiveBucketConfig:
    return LiveBucketConfig(cli_runner)


@pytest.fixture()
def temp_bucket(cli_runner: CliRunner, fresh_bucket_name: str) -> Iterator[str]:
    """创建一个空 bucket，yield 名称，结束自动清理。

    清理顺序：
        1. ``rm tos://bucket/ --recursive --force --confirm tos://bucket/``  — 把残留对象递归清空
        2. ``bucket delete --force --confirm tos://bucket``  — 删 bucket 本身
    任一步失败都只打印告警，不让 fixture 把测试结果污染成 error。
    """
    yield from _create_temp_bucket(cli_runner, fresh_bucket_name, fixture_name="temp_bucket")


@pytest.fixture()
def hns_temp_bucket(cli_runner: CliRunner, fresh_bucket_name: str) -> Iterator[str]:
    """创建一个 HNS bucket，供依赖 HNS 能力的 live tests 使用。"""
    # [Review Fix #HNS-SETUP-1] Isolate HNS-only APIs from the default FNS
    # bucket setup so live tests exercise the service capability they require.
    yield from _create_temp_bucket(
        cli_runner,
        fresh_bucket_name,
        fixture_name="hns_temp_bucket",
        bucket_type="hns",
    )


@pytest.fixture()
def rename_enabled_bucket(cli_runner: CliRunner, fresh_bucket_name: str) -> Iterator[str]:
    """创建一个已开启 RenameObject 的 FNS bucket。"""

    def enable_rename(bucket: str) -> None:
        result = cli_runner.run(
            [
                "ve-tos",
                "rename",
                "set",
                "--bucket",
                bucket,
                "--config",
                '{"RenameEnable":true}',
            ]
        )
        if result.exit_code != 0:
            skip_on_live_error("tos rename set (setup)", result)
        envelope_assert.assert_success_envelope(result.require_envelope())

    yield from _create_temp_bucket(
        cli_runner,
        fresh_bucket_name,
        fixture_name="rename_enabled_bucket",
        setup=enable_rename,
    )


@pytest.fixture()
def object_lock_temp_bucket(cli_runner: CliRunner, fresh_bucket_name: str) -> Iterator[str]:
    """创建一个已启用 ObjectLockConfiguration 的 bucket。"""

    def enable_object_lock(bucket: str) -> None:
        result = cli_runner.run(
            [
                "ve-tos",
                "worm",
                "set",
                "--bucket",
                bucket,
                "--config",
                '{"ObjectLockEnabled":"Enabled"}',
            ]
        )
        if result.exit_code != 0:
            skip_on_live_error("tos worm set (setup)", result)
        envelope_assert.assert_success_envelope(result.require_envelope())
        # [Review Fix #ObjectLock-E2E-1] ObjectLock configuration may be served
        # from a delayed cache; wait before creating objects that depend on it.
        import time

        time.sleep(60)

    yield from _create_temp_bucket(
        cli_runner,
        fresh_bucket_name,
        fixture_name="object_lock_temp_bucket",
        bucket_object_lock_enabled=True,
        setup=enable_object_lock,
    )


def _create_temp_bucket(
    cli_runner: CliRunner,
    bucket: str,
    *,
    fixture_name: str,
    bucket_type: str | None = None,
    bucket_object_lock_enabled: bool = False,
    setup: Callable[[str], None] | None = None,
) -> Iterator[str]:
    create_args = ["ve-tos", "bucket", "create", "--bucket", bucket]
    if bucket_type is not None:
        create_args.extend(["--bucket-type", bucket_type])
    if bucket_object_lock_enabled:
        create_args.append("--bucket-object-lock-enabled")
    create = cli_runner.run(create_args)
    if create.exit_code != 0:
        message = f"{create.stdout}\n{create.stderr}"
        if "TooManyBuckets" in message:
            pytest.skip(f"cannot create isolated bucket {bucket}: TooManyBuckets")
        pytest.skip(
            f"cannot create isolated bucket {bucket}: "
            f"exit={create.exit_code}, stderr={create.stderr[:300]}"
        )
    try:
        if setup is not None:
            setup(bucket)
        yield bucket
    finally:
        if os.environ.get("TOS_E2E_KEEP") == "1":
            print(
                f"[{fixture_name}] TOS_E2E_KEEP=1, leaving bucket {bucket} for inspection",
                file=sys.stderr,
            )
            return
        _cleanup_bucket(cli_runner, bucket)


def _cleanup_bucket(runner: CliRunner, bucket: str) -> None:
    # [Review Fix #1] 用 high-level ``tos rm tos://bucket/ --recursive`` 清空对象，
    # 而不是 low-level ``object batch-delete --all`` —— 后者参数语义是 ``--keys``
    # 显式列表，没有 ``--all`` 选项，盲调会被 ValidationError 拒绝。
    # HNS buckets intentionally use the same path so e2e teardown covers the
    # bottom-up recursive delete behavior implemented by ``tos rm`` itself.
    purge = runner.run(
        [
            "ve-tos",
            "rm",
            f"tos://{bucket}/",
            "--recursive",
            "--force",
            "--confirm",
            f"tos://{bucket}/",
        ],
        timeout=300.0,
    )
    if purge.exit_code != 0:
        print(
            f"[temp_bucket] object purge non-zero (exit={purge.exit_code}); "
            f"continuing to delete bucket. stderr={purge.stderr.strip()[:200]}",
            file=sys.stderr,
        )
    # [Review Fix #2] MRAP live tests attach a control-plane config to the
    # session bucket; TOS rejects bucket deletion while that config remains.
    _cleanup_mrap_configs(runner, bucket)
    delete = runner.run(
        ["ve-tos", "bucket", "delete", "--bucket", bucket, "--force", "--confirm", f"tos://{bucket}"]
    )
    if delete.exit_code != 0:
        print(
            f"[temp_bucket] WARN: failed to delete {bucket}: exit={delete.exit_code} "
            f"stderr={delete.stderr.strip()[:200]}",
            file=sys.stderr,
        )


def _cleanup_mrap_configs(runner: CliRunner, bucket: str) -> None:
    account_id = os.environ.get("TOS_ACCOUNT_ID")
    if not account_id:
        return

    for mrap_name in _mrap_cleanup_names(bucket):
        delete = runner.run(
            [
                "ve-tos",
                "mrap",
                "delete",
                "--account-id",
                account_id,
                "--name",
                mrap_name,
                "--force",
                "--confirm",
                f"tos://{mrap_name}",
            ],
            timeout=240.0,
        )
        if delete.exit_code == 0 or _is_resource_not_found(delete):
            continue
        print(
            f"[temp_bucket] WARN: failed to delete MRAP {mrap_name} for {bucket}: "
            f"exit={delete.exit_code} stderr={delete.stderr.strip()[:200]}",
            file=sys.stderr,
        )


def _mrap_cleanup_names(bucket: str) -> list[str]:
    return [f"e2emrap{bucket[:20].replace('-', '')}"]


def _is_resource_not_found(result: Any) -> bool:
    if result.exit_code == envelope_assert.EXIT_CODES["resource_not_found"]:
        return True
    message = f"{result.stdout}\n{result.stderr}".lower()
    return any(token in message for token in ("notfound", "not found", "no such", "nosuch"))


@pytest.fixture()
def random_payload(tmp_path: Path) -> Path:
    """生成 1 MiB 随机字节，返回路径。"""
    payload = tmp_path / f"payload-{unique_suffix(8)}.bin"
    data = os.urandom(1 * 1024 * 1024)
    payload.write_bytes(data)
    return payload


@pytest.fixture()
def small_text_payload(tmp_path: Path) -> Path:
    payload = tmp_path / "hello.txt"
    payload.write_text(f"hello-tos-e2e-{unique_suffix(8)}\n")
    return payload


def md5_of(path):
    """转发到 ``_lib.md5_of``，保留旧导入路径的向后兼容。"""
    from _lib import md5_of as _md5

    return _md5(path)


def skip_on_live_error(command: str, result) -> None:
    """Skip a live E2E case with the service-side reason captured in output."""
    envelope = result.envelope()
    if envelope and envelope.get("error"):
        error = envelope["error"]
        pytest.skip(
            f"{command} live E2E blocked: {error.get('code')} {error.get('message')}"
        )
    pytest.skip(
        f"{command} live E2E blocked: exit={result.exit_code}, "
        f"stderr={result.stderr[:300]}"
    )
