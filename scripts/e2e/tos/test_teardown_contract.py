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

"""Static teardown contract checks for real-resource E2E tests."""

from __future__ import annotations

from pathlib import Path

from _lib.registry import command_root
from _lib.surface_matrix import action_matches_command, build_smoke_case, expected_validation_failure


TESTS_DIR = Path(__file__).resolve().parent

def test_destructive_tests_use_teardown_fixtures_or_finally_blocks() -> None:
    """@case-id ERR-010.D7.teardown destructive tests must be idempotent by construction."""
    failures: list[str] = []
    for test_file in TESTS_DIR.glob("test_*.py"):
        source = test_file.read_text()
        if "@pytest.mark.destructive" not in source:
            continue
        if not _has_teardown_contract(source):
            failures.append(test_file.name)
    assert not failures, f"destructive tests without teardown contract: {failures}"


def _has_teardown_contract(source: str) -> bool:
    """Return whether a destructive E2E file declares an explicit cleanup path.

    The audit intentionally avoids filename allowlists: every destructive file
    must carry either isolated-resource fixtures, a finally cleanup, or one of
    the shared idempotent restore helpers from ``LiveBucketConfig``.
    """

    if "temp_bucket" in source or "fresh_bucket_name" in source:
        return True
    if "finally:" in source and ("delete" in source or "_cleanup_bucket" in source):
        return True
    restore_markers = (
        "restore_optional(",
        "restore_notification(",
        "cleanup_inventory(",
        "delete_config(",
        "disable" in source and "get_optional(" in source,
    )
    return any(marker for marker in restore_markers)


def test_temp_bucket_cleanup_uses_recursive_purge_then_bucket_delete() -> None:
    """@case-id ERR-011.D7.teardown temp bucket cleanup removes objects before bucket."""
    conftest = TESTS_DIR.parent / "conftest.py"
    source = conftest.read_text()
    assert '"--confirm",\n            f"tos://{bucket}/",' in source
    assert '"--confirm", f"tos://{bucket}"' in source


def test_temp_bucket_cleanup_removes_mrap_before_bucket_delete() -> None:
    """@case-id ERR-012.D7.teardown temp bucket cleanup drops MRAP configs before bucket."""
    conftest = TESTS_DIR.parent / "conftest.py"
    source = conftest.read_text()
    assert "_cleanup_mrap_configs(runner, bucket)" in source
    assert source.index("_cleanup_mrap_configs(runner, bucket)") < source.index(
        '["ve-tos", "bucket", "delete"'
    )


def test_surface_matrix_preserves_command_prefix(tmp_path: Path) -> None:
    """@case-id ERR-013.D7.matrix generated cases execute the registry command verbatim."""
    leaf = {
        "command": "ve-tos cp",
        "parameters": [
            {"name": "source", "positional": True},
            {"name": "destination", "positional": True},
        ],
    }
    case = build_smoke_case(leaf, tmp_path)
    assert case.args[:2] == ("ve-tos", "cp")


def test_command_root_accepts_public_ve_tos_prefix() -> None:
    """@case-id ERR-014.D7.matrix public ve-tos capability commands remain parseable."""
    assert command_root("ve-tos cp") == "cp"


def test_dry_run_action_matching_accepts_public_ve_tos_prefix() -> None:
    """@case-id ERR-015.D7.matrix internal dry-run actions match public capability commands."""
    assert action_matches_command("ve-tos turbo open", "ve-tos turbo open")
    assert (
        expected_validation_failure(
            {"ve-tos real-time-log set": "requires --use-service-topic"},
            "ve-tos real-time-log set",
        )
        == "requires --use-service-topic"
    )
