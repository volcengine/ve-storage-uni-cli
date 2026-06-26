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

"""HL/GP command matrix from the Lark E2E plan.

Every command runs through the real binary. Mutating commands use ``--dry-run`` when
the case is about parameter coverage rather than object bytes.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from _lib import CliRunner, CommandCase, envelope_assert


def _report(tmp_path: Path, name: str) -> str:
    return str(tmp_path / f"{name}.jsonl")


def _cases(tmp_path: Path) -> list[CommandCase]:
    return [
        # @case-id HL-001.D2.ALL
        CommandCase.build(
            "HL-001.D2.ALL",
            "HL",
            [
                "ve-tos",
                "cp",
                str(tmp_path / "src.txt"),
                "tos://dry-run-bucket/obj",
                "--recursive",
                "--include",
                "*.txt",
                "--exclude",
                "*.tmp",
                "--checkpoint",
                "--checkpoint-dir",
                str(tmp_path / "cp-checkpoints"),
                "--report-path",
                _report(tmp_path, "cp"),
                "--bandwidth-limit",
                "1MB",
                "--no-progress",
                "--force",
            ],
            dry_run=True,
        ),
        # @case-id HL-002.D2.ALL
        CommandCase.build(
            "HL-002.D2.ALL",
            "HL",
            [
                "ve-tos",
                "mv",
                "tos://dry-run-bucket/src",
                "tos://dry-run-bucket/dst",
                "--recursive",
                "--include",
                "*.log",
                "--exclude",
                "*.tmp",
                "--checkpoint-dir",
                str(tmp_path / "mv-checkpoints"),
                "--report-path",
                _report(tmp_path, "mv"),
                "--no-progress",
                "--force",
                "--confirm",
                "tos://dry-run-bucket/src",
            ],
            dry_run=True,
        ),
        # @case-id HL-003.D2.ALL
        CommandCase.build(
            "HL-003.D2.ALL",
            "HL",
            [
                "ve-tos",
                "sync",
                str(tmp_path / "sync-src"),
                "tos://dry-run-bucket/sync/",
                "--delete",
                "--force",
                "--confirm",
                "tos://dry-run-bucket/sync/",
                "--size-only",
                "--exact-timestamps",
                "--include",
                "*.txt",
                "--exclude",
                "*.bak",
                "--checkpoint-dir",
                str(tmp_path / "sync-checkpoints"),
                "--report-path",
                _report(tmp_path, "sync"),
                "--bandwidth-limit",
                "1MB",
                "--no-progress",
            ],
            dry_run=True,
        ),
        # @case-id HL-004.D4.ALL
        CommandCase.build(
            "HL-004.D4.ALL",
            "HL",
            [
                "ve-tos",
                "mb",
                "tos://dry-run-bucket",
                "--region",
                "cn-beijing",
                "--storage-class",
                "IA",
                "--acl",
                "private",
                "--az-redundancy",
                "multi-az",
                "--bucket-object-lock-enabled",
            ],
            dry_run=True,
        ),
        # @case-id HL-005.D2.force
        CommandCase.build(
            "HL-005.D2.force",
            "HL",
            ["ve-tos", "rb", "tos://dry-run-bucket", "--force"],
            dry_run=True,
        ),
        # @case-id HL-005B.D1.mkdir
        CommandCase.build(
            "HL-005B.D1.mkdir",
            "HL",
            ["ve-tos", "mkdir", "tos://dry-run-bucket/folder/subfolder/", "--parents"],
            dry_run=True,
        ),
        # @case-id HL-006.D2.ALL
        CommandCase.build(
            "HL-006.D2.ALL",
            "HL",
            [
                "ve-tos",
                "rm",
                "tos://dry-run-bucket/prefix/",
                "--recursive",
                "--recursive-delete-mode",
                "bottom-up",
                "--force",
                "--confirm",
                "tos://dry-run-bucket/prefix/",
                "--report-path",
                _report(tmp_path, "rm"),
                "--include",
                "*.txt",
                "--exclude",
                "*.tmp",
                "--no-progress",
            ],
            dry_run=True,
        ),
        # @case-id HL-007.D6.ALL
        CommandCase.build(
            "HL-007.D6.ALL",
            "HL",
            [
                "ve-tos",
                "ls",
                "tos://dry-run-bucket/prefix/",
                "--human-readable",
                "--sort",
                "size",
                "--manifest-path",
                _report(tmp_path, "ls"),
            ],
            dry_run=True,
        ),
        # @case-id HL-008.D1.object
        CommandCase.build(
            "HL-008.D1.object",
            "HL",
            ["ve-tos", "stat", "tos://dry-run-bucket/obj", "--version-id", "v1"],
            dry_run=True,
        ),
        # @case-id HL-009.D3.ALL
        CommandCase.build(
            "HL-009.D3.ALL",
            "HL",
            [
                "ve-tos",
                "du",
                "tos://dry-run-bucket/prefix/",
                "--human-readable",
                "--max-depth",
                "2",
                "--manifest-path",
                _report(tmp_path, "du"),
                "--no-progress",
            ],
            dry_run=True,
        ),
        # @case-id HL-010.D5.ALL
        CommandCase.build(
            "HL-010.D5.ALL",
            "HL",
            [
                "ve-tos",
                "find",
                "tos://dry-run-bucket/prefix/",
                "--name",
                "*.txt",
                "--size",
                "+1KB",
                "--mtime=-1d",
                "--storage-class",
                "STANDARD",
                "--manifest-path",
                _report(tmp_path, "find"),
                "--no-progress",
            ],
            dry_run=True,
        ),
        # @case-id HL-011.D3.range
        CommandCase.build(
            "HL-011.D3.range",
            "HL",
            ["ve-tos", "cat", "tos://dry-run-bucket/obj", "--range", "0-3", "--version-id", "v1"],
            dry_run=True,
        ),
        # @case-id HL-012.D4.method
        CommandCase.build(
            "HL-012.D4.method",
            "HL",
            ["ve-tos", "presign", "tos://dry-run-bucket/obj", "--expires", "60", "--method", "PUT"],
            dry_run=True,
        ),
        # @case-id HL-013.D2.ALL
        CommandCase.build(
            "HL-013.D2.ALL",
            "HL",
            [
                "ve-tos",
                "restore",
                "tos://dry-run-bucket/archive/",
                "--recursive",
                "--manifest",
                str(tmp_path / "restore-manifest.txt"),
                "--include",
                "*.gz",
                "--exclude",
                "*.tmp",
                "--days",
                "1",
                "--tier",
                "Standard",
                "--version-id",
                "v1",
                "--report-path",
                _report(tmp_path, "restore"),
                "--force",
                "--no-progress",
            ],
            dry_run=True,
        ),
    ]


@pytest.mark.parametrize("case", _cases(Path("/tmp/tos-e2e-case")), ids=lambda c: c.case_id)
def test_high_level_full_parameter_dry_run(cli_runner: CliRunner, case: CommandCase) -> None:
    args = list(case.args)
    if case.dry_run:
        args.insert(0, "--dry-run")
    result = cli_runner.run(args)
    assert result.exit_code == case.expected_exit, (
        f"{case.case_id} failed: exit={result.exit_code}\nstdout={result.stdout}\nstderr={result.stderr}"
    )
    envelope_assert.assert_success_envelope(result.require_envelope())
    payload = result.payload()
    assert isinstance(payload, dict)
    assert payload.get("dry_run") is True, f"{case.case_id} must not mutate real resources"
    assert isinstance(payload.get("summary"), dict), f"{case.case_id} must expose plan summary"
    assert isinstance(payload.get("request_plan"), list), f"{case.case_id} must expose request plan"
    assert isinstance(payload.get("samples"), list), f"{case.case_id} must expose plan samples"
    assert payload["request_plan"], f"{case.case_id} request plan must not be empty"
