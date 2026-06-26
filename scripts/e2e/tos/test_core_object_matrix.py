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

"""OBJ core object parameter matrix."""

from __future__ import annotations

from pathlib import Path

import pytest

from _lib import CliRunner, CommandCase, envelope_assert


def _object_cases(tmp_path: Path) -> list[CommandCase]:
    body = str(tmp_path / "body.txt")
    return [
        # @case-id OBJ-001.D1.uri
        CommandCase.build(
            "OBJ-001.D1.uri",
            "OBJ",
            [
                "ve-tos",
                "object",
                "upload",
                "tos://dry-run-bucket/obj",
                "--body",
                body,
                "--content-type",
                "text/plain",
                "--storage-class",
                "STANDARD",
                "--meta",
                "k=v",
                "--net-speed-test",
                "true",
            ],
        ),
        # @case-id OBJ-002.D3.range
        CommandCase.build(
            "OBJ-002.D3.range",
            "OBJ",
            [
                "ve-tos",
                "object",
                "download",
                "tos://dry-run-bucket/obj",
                "--body",
                str(tmp_path / "download.txt"),
                "--version-id",
                "v1",
                "--range",
                "0-3",
                "--if-modified-since",
                "Wed, 21 Oct 2015 07:28:00 GMT",
                "--if-unmodified-since",
                "Wed, 21 Oct 2030 07:28:00 GMT",
                "--replicated-from",
                "src",
                "--from-modular",
                "true",
            ],
        ),
        # @case-id OBJ-003.D2.ALL
        CommandCase.build(
            "OBJ-003.D2.ALL",
            "OBJ",
            [
                "ve-tos",
                "object",
                "copy",
                "tos://dry-run-bucket/src",
                "tos://dry-run-bucket/dst",
                "--range",
                "0-3",
                "--copy-source-if-modified-since",
                "Wed, 21 Oct 2015 07:28:00 GMT",
                "--copy-source-if-unmodified-since",
                "Wed, 21 Oct 2030 07:28:00 GMT",
                "--etag-pattern",
                "*",
                "--metadata-directive",
                "COPY",
                "--tagging-directive",
                "COPY",
                "--unique-tag",
                "u",
                "--copy-source-last-modified",
                "1",
                "--data-id",
                "d",
                "--finger-print",
                "f",
                "--internal-metadata-directive",
                "COPY",
                "--crr-source-timestamp-nsec",
                "1",
                "--crr-proxy",
                "p",
                "--crr-source-bucket-version-status",
                "Enabled",
                "--traffic-limit",
                "1024",
                "--object-lock-mode",
                "GOVERNANCE",
                "--object-lock-retain-until-date",
                "2030-01-01T00:00:00Z",
                "--if-unmodified-since",
                "Wed, 21 Oct 2030 07:28:00 GMT",
                "--if-none-match",
                "none",
                "--if-match",
                "etag",
                "--persistent-headers",
                "k=v",
                "--tagging",
                "k=v",
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
            ],
        ),
        # @case-id OBJ-004.D2.force
        CommandCase.build(
            "OBJ-004.D2.force",
            "OBJ",
            [
                "ve-tos",
                "object",
                "delete",
                "tos://dry-run-bucket/obj",
                "--version-id",
                "v1",
                "--force",
                "--confirm",
                "tos://dry-run-bucket/obj",
                "--from-modular",
                "true",
                "--if-match-expires",
                "1",
                "--last-modified",
                "1",
                "--if-match-create-time",
                "1",
                "--if-match",
                "etag",
                "--if-match-tags",
                "k=v",
                "--if-match-access-time",
                "1",
                "--lifecycle-directly-delete-versions",
                "--if-match-inode-id",
                "inode",
                "--parent-inode-id",
                "parent",
                "--only-put-delete-marker",
                "--inner-properties-timestamp",
                "1",
                "--inner-properties-timestamp-nsec",
                "1",
            ],
        ),
        # @case-id OBJ-005.D5.batch
        CommandCase.build(
            "OBJ-005.D5.batch",
            "OBJ",
            [
                "ve-tos",
                "object",
                "batch-delete",
                "--bucket",
                "dry-run-bucket",
                "--keys",
                '["a","b"]',
                "--force",
                "--confirm",
                "tos://dry-run-bucket",
                "--recursive",
                "--skip-trash",
                "--content-md5",
                "deadbeef",
            ],
        ),
        # @case-id OBJ-006.D3.page
        CommandCase.build(
            "OBJ-006.D3.page",
            "OBJ",
            [
                "ve-tos",
                "object",
                "list",
                "tos://dry-run-bucket/prefix/",
                "--delimiter",
                "/",
                "--max-keys",
                "10",
                "--continuation-token",
                "token",
            ],
        ),
        # @case-id OBJ-007.D2.metadata
        CommandCase.build(
            "OBJ-007.D2.metadata",
            "OBJ",
            [
                "ve-tos",
                "object",
                "set-meta",
                "tos://dry-run-bucket/obj",
                "--meta",
                "k=v",
                "--version-id",
                "v1",
                "--unique-tag",
                "u",
                "--content-type",
                "text/plain",
            ],
        ),
        # @case-id OBJ-008.D2.append
        CommandCase.build(
            "OBJ-008.D2.append",
            "OBJ",
            [
                "ve-tos",
                "object",
                "append",
                "tos://dry-run-bucket/append",
                "--body",
                body,
                "--offset",
                "0",
                "--append-last-time",
                "1",
                "--version-id",
                "v1",
                "--content-type",
                "text/plain",
                "--content-md5",
                "deadbeef",
                "--content-sha256",
                "abc",
                "--decoded-content-length",
                "4",
                "--traffic-limit",
                "1024",
                "--if-none-match",
                "none",
                "--if-match",
                "etag",
            ],
        ),
        # @case-id OBJ-009.D4.acl
        CommandCase.build(
            "OBJ-009.D4.acl",
            "OBJ",
            ["ve-tos", "object", "set-acl", "tos://dry-run-bucket/obj", "--acl", "private"],
        ),
        # @case-id OBJ-010.D5.tags
        CommandCase.build(
            "OBJ-010.D5.tags",
            "OBJ",
            ["ve-tos", "object", "set-tagging", "tos://dry-run-bucket/obj", "--tags", "k=v"],
        ),
    ]


@pytest.mark.parametrize("case", _object_cases(Path("/tmp/tos-e2e-obj")), ids=lambda c: c.case_id)
def test_object_core_full_parameter_dry_run(cli_runner: CliRunner, case: CommandCase) -> None:
    result = cli_runner.run(["--dry-run", *case.args])
    assert result.exit_code == 0, (
        f"{case.case_id} failed: exit={result.exit_code}\nstdout={result.stdout}\nstderr={result.stderr}"
    )
    envelope_assert.assert_success_envelope(result.require_envelope())
    assert result.payload()["dry_run"] is True
