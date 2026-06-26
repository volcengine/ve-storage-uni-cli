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

"""共享 CLI runner / Envelope 解析 / 唯一 ID 工具。

设计要点：
- 严格 subprocess 执行（PTY 不接管），用于验证 Safe Execution guard 不依赖交互确认。
- 默认 ``--output json``，统一从 stdout 解析 ``Envelope`` 结构。
- 永不 raise on non-zero exit，把 ``CommandResult`` 完整返回给 caller，让单测自己断言期望。
"""

from __future__ import annotations

import dataclasses
import json
import os
import secrets
import subprocess
from pathlib import Path
from typing import Any, Iterable, Mapping, Optional, Protocol


REPO_ROOT = Path(__file__).resolve().parents[3]
DEFAULT_BIN = REPO_ROOT / "target" / "release" / "ve-storage-uni-cli"

ENV_PROPAGATE = (
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TMPDIR",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
)


class E2EConfigError(RuntimeError):
    """配置缺失或非法。"""


class Credentials(Protocol):
    def as_env(self) -> dict[str, str]:
        """Return environment variables used by subprocess-based E2E commands."""
        ...


@dataclasses.dataclass(frozen=True)
class TosCredentials:
    access_key: str
    secret_key: str
    endpoint: str
    region: str
    control_endpoint: str | None
    account_id: str | None
    https_endpoint: str | None
    bucket_prefix: str

    @classmethod
    def from_env(cls) -> "TosCredentials":
        missing = [
            name
            for name in ("TOS_ACCESS_KEY", "TOS_SECRET_KEY", "TOS_ENDPOINT", "TOS_REGION")
            if not os.environ.get(name)
        ]
        if missing:
            raise E2EConfigError(
                f"missing required env vars: {', '.join(missing)}; "
                "see scripts/e2e/README.md for setup"
            )
        return cls(
            access_key=os.environ["TOS_ACCESS_KEY"],
            secret_key=os.environ["TOS_SECRET_KEY"],
            endpoint=os.environ["TOS_ENDPOINT"],
            region=os.environ["TOS_REGION"],
            control_endpoint=os.environ.get("TOS_CONTROL_ENDPOINT"),
            account_id=os.environ.get("TOS_ACCOUNT_ID"),
            https_endpoint=os.environ.get("TOS_HTTPS_ENDPOINT"),
            bucket_prefix=os.environ.get("TOS_E2E_BUCKET_PREFIX", "ve-tos-cli-e2e"),
        )

    def as_env(self) -> dict[str, str]:
        env = {
            "TOS_ACCESS_KEY": self.access_key,
            "TOS_SECRET_KEY": self.secret_key,
            "TOS_ENDPOINT": self.endpoint,
            "TOS_REGION": self.region,
        }
        if self.control_endpoint:
            env["TOS_CONTROL_ENDPOINT"] = self.control_endpoint
        if self.account_id:
            env["TOS_ACCOUNT_ID"] = self.account_id
        if self.https_endpoint:
            env["TOS_HTTPS_ENDPOINT"] = self.https_endpoint
        return env


@dataclasses.dataclass(frozen=True)
class ADriveCredentials:
    access_key: str
    secret_key: str
    endpoint: str
    region: str | None
    resource_prefix: str

    @classmethod
    def from_env(cls) -> "ADriveCredentials":
        missing = [
            name
            for name in (
                "ADRIVE_ACCESS_KEY",
                "ADRIVE_SECRET_KEY",
                "ADRIVE_ENDPOINT",
            )
            if not os.environ.get(name)
        ]
        if missing:
            raise E2EConfigError(
                f"missing required env vars: {', '.join(missing)}; "
                "ADrive E2E requires ADRIVE_ACCESS_KEY, ADRIVE_SECRET_KEY and ADRIVE_ENDPOINT"
            )
        return cls(
            access_key=os.environ["ADRIVE_ACCESS_KEY"],
            secret_key=os.environ["ADRIVE_SECRET_KEY"],
            endpoint=os.environ["ADRIVE_ENDPOINT"],
            region=os.environ.get("ADRIVE_REGION"),
            resource_prefix=os.environ.get("ADRIVE_E2E_RESOURCE_PREFIX", "tos-uni-adrive-e2e"),
        )

    def as_env(self) -> dict[str, str]:
        env = {
            "ADRIVE_ACCESS_KEY": self.access_key,
            "ADRIVE_SECRET_KEY": self.secret_key,
            "ADRIVE_ENDPOINT": self.endpoint,
        }
        if self.region:
            env["ADRIVE_REGION"] = self.region
        return env


@dataclasses.dataclass(frozen=True)
class CommandResult:
    args: tuple[str, ...]
    exit_code: int
    stdout: str
    stderr: str

    def envelope(self) -> Optional[dict[str, Any]]:
        """尝试把 stdout/stderr 解析成 Envelope dict；非 JSON 返回 None。"""
        parsed = self.json()
        if isinstance(parsed, dict) and "status" in parsed:
            return parsed
        return None

    def json(self) -> Optional[Any]:
        """解析 CLI JSON 输出。

        部分 clap/handler 早返回路径会把结构化错误写到 stderr，describe 路径也可能输出
        裸 JSON 而不是 Envelope；统一在这里做 best-effort 解析。
        """
        for stream in (self.stdout, self.stderr):
            if not stream.strip():
                continue
            try:
                return json.loads(stream)
            except json.JSONDecodeError:
                continue
        return None

    def payload(self) -> Any:
        """返回 Envelope.data；若命令输出裸 JSON，则返回该 JSON 本身。"""
        parsed = self.json()
        if parsed is None:
            raise AssertionError(
                f"expected JSON output, got:\n--- stdout ---\n{self.stdout}\n"
                f"--- stderr ---\n{self.stderr}"
            )
        if isinstance(parsed, dict) and "status" in parsed and "data" in parsed:
            return parsed["data"]
        return parsed

    def require_envelope(self) -> dict[str, Any]:
        env = self.envelope()
        if env is None:
            raise AssertionError(
                f"expected JSON envelope on stdout, got:\n--- stdout ---\n{self.stdout}\n"
                f"--- stderr ---\n{self.stderr}"
            )
        return env


class CliRunner:
    """对 ve-storage-uni-cli 二进制的非交互调用包装。"""

    def __init__(
        self,
        creds: Credentials,
        binary: Optional[Path] = None,
        home_override: Optional[Path] = None,
    ) -> None:
        self._creds = creds
        self._binary = Path(binary) if binary else DEFAULT_BIN
        if not self._binary.exists():
            raise E2EConfigError(
                f"binary not found at {self._binary}; run `cargo build --release` first"
            )
        self._home = home_override

    def run(
        self,
        args: Iterable[str],
        *,
        extra_env: Optional[Mapping[str, str]] = None,
        stdin: Optional[bytes] = None,
        timeout: float = 120.0,
        json_output: bool = True,
    ) -> CommandResult:
        argv = [str(self._binary)]
        if json_output and not _has_output_flag(args):
            argv.extend(["--output", "json"])
        argv.extend(args)

        env = self._build_env(extra_env)
        try:
            proc = subprocess.run(
                argv,
                input=stdin,
                env=env,
                capture_output=True,
                timeout=timeout,
                check=False,
            )
        except subprocess.TimeoutExpired as exc:
            raise AssertionError(
                f"command timed out after {timeout}s: {' '.join(argv)}"
            ) from exc

        return CommandResult(
            args=tuple(argv),
            exit_code=proc.returncode,
            stdout=proc.stdout.decode("utf-8", errors="replace"),
            stderr=proc.stderr.decode("utf-8", errors="replace"),
        )

    def _build_env(self, extra: Optional[Mapping[str, str]]) -> dict[str, str]:
        env: dict[str, str] = {}
        for key in ENV_PROPAGATE:
            if key in os.environ:
                env[key] = os.environ[key]
        env.update(self._creds.as_env())
        if self._home is not None:
            env["HOME"] = str(self._home)
        # 显式删除可能干扰的临时凭证环境变量
        for k in ("TOS_SECURITY_TOKEN",):
            env.pop(k, None)
        if extra:
            env.update(extra)
        return env


def _has_output_flag(args: Iterable[str]) -> bool:
    for a in args:
        if a == "--output" or a.startswith("--output="):
            return True
        if a == "-o":
            return True
    return False


def unique_suffix(length: int = 16) -> str:
    """生成 hex 后缀；TOS bucket 名称 only allows lower-case alnum + '-'。"""
    return secrets.token_hex(length // 2)


def md5_of(path: Path) -> str:
    """流式计算 hex MD5；64 KiB 分块，避免大文件爆内存。"""
    import hashlib

    digest = hashlib.md5()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(64 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()
