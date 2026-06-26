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

"""OBJ multipart and turbo parameter matrix."""

from __future__ import annotations

import pytest

from _lib import CliRunner, CommandCase, envelope_assert


COMMON_GRANTS = [
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
]

CASES = [
    # @case-id OBJ-101.D2.ALL
    CommandCase.build(
        "OBJ-101.D2.ALL",
        "OBJ",
        [
            "ve-tos",
            "multipart",
            "create",
            "tos://dry-run-bucket/multi",
            "--forbid-overwrite",
            "--etag-pattern",
            "*",
            "--persistent-headers",
            "k=v",
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
            "--tagging",
            "k=v",
            "--replicated-from",
            "src",
            "--crr-source-version-id",
            "v1",
            "--crr-source-last-modify-time",
            "1",
            "--crr-source-timestamp-nsec",
            "1",
            "--crr-source-bucket-version-status",
            "Enabled",
            "--crr-source-upload-id",
            "upload",
            "--from-modular",
            "true",
            *COMMON_GRANTS,
        ],
    ),
    # @case-id OBJ-102.D3.part
    CommandCase.build(
        "OBJ-102.D3.part",
        "OBJ",
        [
            "ve-tos",
            "multipart",
            "upload",
            "tos://dry-run-bucket/multi",
            "--upload-id",
            "upload",
            "--part-number",
            "1",
            "--body",
            "/tmp/body.txt",
            "--content-md5",
            "deadbeef",
            "--content-sha256",
            "abc",
            "--hash-crc64ecma",
            "1",
            "--decoded-content-length",
            "4",
            "--traffic-limit",
            "1024",
        ],
    ),
    # @case-id OBJ-103.D5.parts
    CommandCase.build(
        "OBJ-103.D5.parts",
        "OBJ",
        [
            "ve-tos",
            "multipart",
            "complete",
            "tos://dry-run-bucket/multi",
            "--upload-id",
            "upload",
            "--parts",
            '[{"PartNumber":1,"ETag":"etag"}]',
            "--complete-all",
            "--if-unmodified-since",
            "Wed, 21 Oct 2030 07:28:00 GMT",
            "--if-none-match",
            "none",
            "--if-match",
            "etag",
            "--server-side-encryption",
            "AES256",
            "--from-modular",
            "true",
        ],
    ),
    # @case-id OBJ-104.D2.force
    CommandCase.build(
        "OBJ-104.D2.force",
        "OBJ",
        [
            "ve-tos",
            "multipart",
            "abort",
            "tos://dry-run-bucket/multi",
            "--upload-id",
            "upload",
            "--force",
            "--from-modular",
            "true",
        ],
    ),
    # @case-id OBJ-105.D3.copy-part
    CommandCase.build(
        "OBJ-105.D3.copy-part",
        "OBJ",
        [
            "ve-tos",
            "multipart",
            "copy",
            "tos://dry-run-bucket/multi",
            "--upload-id",
            "upload",
            "--part-number",
            "1",
            "--copy-source",
            "/dry-run-bucket/src",
            "--copy-source-range",
            "bytes=0-3",
            "--copy-source-part-number",
            "1",
            "--copy-source-if-modified-since",
            "Wed, 21 Oct 2015 07:28:00 GMT",
            "--copy-source-if-unmodified-since",
            "Wed, 21 Oct 2030 07:28:00 GMT",
            "--etag-pattern",
            "*",
            "--traffic-limit",
            "1024",
        ],
    ),
    # @case-id OBJ-106.D3.page
    CommandCase.build(
        "OBJ-106.D3.page",
        "OBJ",
        [
            "ve-tos",
            "multipart",
            "list-parts",
            "tos://dry-run-bucket/multi",
            "--upload-id",
            "upload",
            "--part-number-marker",
            "1",
            "--max-parts",
            "10",
            "--fetch-from-kv",
        ],
    ),
    # @case-id OBJ-107.D3.page
    CommandCase.build(
        "OBJ-107.D3.page",
        "OBJ",
        [
            "ve-tos",
            "multipart",
            "list",
            "--bucket",
            "dry-run-bucket",
            "--prefix",
            "p/",
            "--delimiter",
            "/",
            "--key-marker",
            "k",
            "--upload-id-marker",
            "u",
            "--max-uploads",
            "10",
            "--encoding-type",
            "url",
            "--fetch-from-kv",
        ],
    ),
    # @case-id OBJ-201.D2.ALL
    CommandCase.build(
        "OBJ-201.D2.ALL",
        "OBJ",
        [
            "ve-tos",
            "turbo",
            "open",
            "tos://dry-run-bucket/turbo",
            "--content-type",
            "text/plain",
            "--content-md5",
            "deadbeef",
            "--hash-crc64ecma",
            "1",
            "--traffic-limit",
            "1024",
            "--if-match-guard-object",
            "etag",
            *COMMON_GRANTS,
        ],
    ),
    # @case-id OBJ-202.D2.ALL
    CommandCase.build(
        "OBJ-202.D2.ALL",
        "OBJ",
        [
            "ve-tos",
            "turbo",
            "append",
            "tos://dry-run-bucket/turbo",
            "--body",
            "/tmp/body.txt",
            "--turbo-token",
            "token",
            "--content-md5",
            "deadbeef",
            "--hash-crc64ecma",
            "1",
            "--traffic-limit",
            "1024",
            "--if-match-guard-object",
            "etag",
            *COMMON_GRANTS,
        ],
    ),
    # @case-id OBJ-203.D3.page
    CommandCase.build(
        "OBJ-203.D3.page",
        "OBJ",
        [
            "ve-tos",
            "turbo",
            "list",
            "--bucket",
            "dry-run-bucket",
            "--key",
            "turbo",
            "--marker",
            "m",
            "--max-keys",
            "10",
            "--prefix",
            "p/",
            "--encoding-type",
            "url",
        ],
    ),
    # @case-id OBJ-204.D2.ALL
    CommandCase.build(
        "OBJ-204.D2.ALL",
        "OBJ",
        [
            "ve-tos",
            "turbo",
            "close",
            "tos://dry-run-bucket/turbo",
            "--turbo-token",
            "token",
            "--traffic-limit",
            "1024",
            "--if-match-guard-object",
            "etag",
            *COMMON_GRANTS,
        ],
    ),
]


@pytest.mark.parametrize("case", CASES, ids=lambda c: c.case_id)
def test_multipart_turbo_full_parameter_dry_run(cli_runner: CliRunner, case: CommandCase) -> None:
    result = cli_runner.run(["--dry-run", *case.args])
    assert result.exit_code == 0, (
        f"{case.case_id} failed: exit={result.exit_code}\nstdout={result.stdout}\nstderr={result.stderr}"
    )
    envelope_assert.assert_success_envelope(result.require_envelope())
    assert result.payload()["dry_run"] is True
