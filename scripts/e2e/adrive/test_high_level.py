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

"""ADrive high-level command E2E coverage.

These cases are intentionally separate from the TOS suite because ADrive uses
ADrive credentials and a different resource hierarchy: instance -> space -> path.
"""

from __future__ import annotations

import dataclasses
import json
import os
import sys
from pathlib import Path
from typing import Iterator

import pytest

from _lib import ADriveCredentials, CliRunner, envelope_assert, unique_suffix


@dataclasses.dataclass(frozen=True)
class ADriveWorkspace:
    instance: str
    space: str
    root: str
    live: bool


@pytest.fixture()
def adrive_workspace(
    adrive_credentials: ADriveCredentials,
) -> Iterator[ADriveWorkspace]:
    suffix = unique_suffix(10)
    instance = os.environ.get("ADRIVE_E2E_INSTANCE")
    space = os.environ.get("ADRIVE_E2E_SPACE")
    root_prefix = os.environ.get("ADRIVE_E2E_ROOT_PREFIX", adrive_credentials.resource_prefix)
    root = f"{root_prefix.rstrip('-')}-{suffix}"
    if instance and space:
        yield ADriveWorkspace(instance=instance, space=space, root=root, live=True)
        return
    yield ADriveWorkspace(
        instance=f"dry-run-instance-{suffix}",
        space=f"dry-run-space-{suffix}",
        root=root,
        live=False,
    )


@pytest.fixture()
def live_adrive_workspace(
    adrive_cli_runner: CliRunner,
    adrive_credentials: ADriveCredentials,
) -> Iterator[ADriveWorkspace]:
    suffix = unique_suffix(10)
    instance = os.environ.get("ADRIVE_E2E_INSTANCE")
    space = os.environ.get("ADRIVE_E2E_SPACE")
    root_prefix = os.environ.get("ADRIVE_E2E_ROOT_PREFIX", adrive_credentials.resource_prefix)
    root = f"{root_prefix.rstrip('-')}-{suffix}"
    if instance and space:
        yield ADriveWorkspace(instance=instance, space=space, root=root, live=True)
        return

    prefix = root_prefix.rstrip("-")
    instance_name = f"{prefix}-{suffix}-inst"
    space_name = f"{prefix}-{suffix}-space"
    created_instance: str | None = None

    create_instance = adrive_cli_runner.run(
        [
            "ve-adrive",
            "crt",
            f"adrive://{instance_name}",
            "--description",
            "Created by ve-storage-uni-cli ADrive E2E",
        ],
        timeout=240.0,
    )
    if create_instance.exit_code != 0:
        pytest.skip(
            "failed to create ADrive E2E instance with ve-adrive crt: "
            f"{create_instance.stderr[:300] or create_instance.stdout[:300]}"
        )
    envelope_assert.assert_success_envelope(create_instance.require_envelope())
    created_instance = create_instance.payload()["instance_id"]

    try:
        create_space = adrive_cli_runner.run(
            [
                "ve-adrive",
                "crt",
                f"adrive://{created_instance}/{space_name}",
                "--index-enabled",
                "--description",
                "Created by ve-storage-uni-cli ADrive E2E",
            ],
            timeout=240.0,
        )
        if create_space.exit_code != 0:
            pytest.skip(
                "failed to create ADrive E2E space with ve-adrive crt: "
                f"{create_space.stderr[:300] or create_space.stdout[:300]}"
            )
        envelope_assert.assert_success_envelope(create_space.require_envelope())
        created_space = create_space.payload()["space_id"]
        yield ADriveWorkspace(instance=created_instance, space=created_space, root=root, live=True)
    finally:
        if created_instance and not os.environ.get("ADRIVE_E2E_KEEP"):
            delete_target = f"adrive://{created_instance}"
            cleanup = adrive_cli_runner.run(
                [
                    "ve-adrive",
                    "del",
                    delete_target,
                    "--force",
                    "--confirm",
                    delete_target,
                ],
                timeout=300.0,
            )
            if cleanup.exit_code != 0:
                print(
                    "[adrive_setup] WARN: failed to delete "
                    f"{created_instance}: exit={cleanup.exit_code}, "
                    f"stderr={cleanup.stderr[:300]}",
                    file=sys.stderr,
                )


@pytest.mark.adrive
def test_adrive_help_is_grouped(adrive_cli_runner: CliRunner) -> None:
    result = adrive_cli_runner.run(["ve-adrive", "--help"], json_output=False)
    assert result.exit_code == 0, result.stderr
    assert "High-Level Commands:" in result.stdout
    assert "crt           Create an instance or space" in result.stdout
    assert "del           Delete an instance or space" in result.stdout
    assert "Capabilities / Utilities:" in result.stdout
    assert "list_instances" not in result.stdout


@pytest.mark.adrive
def test_adrive_ls_supports_instance_space_file_targets(
    adrive_cli_runner: CliRunner,
    adrive_workspace: ADriveWorkspace,
) -> None:
    instance = adrive_workspace.instance
    space = adrive_workspace.space

    cases = [
        ["ve-adrive", "ls", "--dry-run"],
        ["ve-adrive", "ls", "--instance", instance, "--dry-run"],
        ["ve-adrive", "ls", "--instance", instance, "--space", space, "--dry-run"],
        ["ve-adrive", "ls", f"adrive://{instance}/{space}/", "--dry-run"],
    ]
    for args in cases:
        result = adrive_cli_runner.run(args)
        assert result.exit_code == 0, f"{args}: {result.stderr}"
        payload = result.payload()
        assert payload["command"] == "ve-adrive ls"
        assert payload["dry_run"] is True


@pytest.mark.adrive
def test_adrive_mkdir_parents_dry_run(
    adrive_cli_runner: CliRunner,
    adrive_workspace: ADriveWorkspace,
) -> None:
    instance = adrive_workspace.instance
    space = adrive_workspace.space
    result = adrive_cli_runner.run(
        ["ve-adrive", "mkdir", f"adrive://{instance}/{space}/a/b/c/", "--parents", "--dry-run"]
    )
    assert result.exit_code == 0, result.stderr
    payload = result.payload()
    assert payload["command"] == "ve-adrive mkdir"
    assert payload["dry_run"] is True
    assert payload["summary"]["recursive"] is True
    assert "parent" in " ".join(payload["request_plan"])


@pytest.mark.adrive
def test_adrive_cp_dry_run_reports_resume_progress_and_manifest_contract(
    adrive_cli_runner: CliRunner,
    adrive_workspace: ADriveWorkspace,
    tmp_path: Path,
) -> None:
    source = tmp_path / "upload-src"
    source.mkdir()
    (source / "a.txt").write_text("checkpoint dry run\n", encoding="utf-8")
    report_path = tmp_path / "cp-report.json"
    checkpoint_dir = tmp_path / "checkpoints"

    result = adrive_cli_runner.run(
        [
            "ve-adrive",
            "cp",
            str(source),
            f"adrive://{adrive_workspace.instance}/{adrive_workspace.space}/{adrive_workspace.root}/",
            "--recursive",
            "--checkpoint",
            "--checkpoint-dir",
            str(checkpoint_dir),
            "--report-path",
            str(report_path),
            "--progress",
            "--dry-run",
        ]
    )
    assert result.exit_code == 0, result.stderr
    payload = result.payload()
    assert payload["command"] == "ve-adrive cp"
    assert payload["dry_run"] is True
    assert payload["checkpoint"]["enabled"] is True
    assert payload["checkpoint"]["directory"] == str(checkpoint_dir)
    assert payload["checkpoint"]["scope"] == "recursive_item_manifest"
    assert payload["report"]["enabled"] is True
    assert payload["report"]["path"] == str(report_path)
    assert payload["progress"]["enabled"] is True
    assert "multipart upload" in " ".join(payload["request_plan"])


@pytest.mark.adrive
def test_adrive_crt_del_dry_run_and_force_contract(
    adrive_cli_runner: CliRunner,
    adrive_workspace: ADriveWorkspace,
) -> None:
    instance = adrive_workspace.instance
    space = adrive_workspace.space

    for args in (
        ["ve-adrive", "crt", f"adrive://{instance}-new", "--dry-run"],
        ["ve-adrive", "crt", f"adrive://{instance}/{space}-new", "--dry-run", "--index-enabled"],
        ["ve-adrive", "del", f"adrive://{instance}", "--dry-run"],
        ["ve-adrive", "del", f"adrive://{instance}/{space}", "--dry-run"],
    ):
        result = adrive_cli_runner.run(args)
        assert result.exit_code == 0, f"{args}: {result.stderr}"
        assert result.payload()["dry_run"] is True

    result = adrive_cli_runner.run(["ve-adrive", "del", f"adrive://{instance}"])
    assert result.exit_code != 0
    assert "--force" in (result.stderr + result.stdout)

    # [Review Fix #5] Critical delete examples and tests must cover the exact
    # non-interactive confirmation gate before any network request can run.
    forced = adrive_cli_runner.run(["ve-adrive", "del", f"adrive://{instance}", "--force"])
    assert forced.exit_code != 0
    assert "--confirm" in (forced.stderr + forced.stdout)


@pytest.mark.adrive
def test_adrive_rm_requires_file_or_folder_target_and_force(
    adrive_cli_runner: CliRunner,
    adrive_workspace: ADriveWorkspace,
) -> None:
    instance = adrive_workspace.instance
    space = adrive_workspace.space

    invalid_resource_targets = [
        ["ve-adrive", "rm", f"adrive://{instance}"],
        ["ve-adrive", "rm", f"adrive://{instance}/{space}"],
        ["ve-adrive", "rm", "--instance", instance],
        ["ve-adrive", "rm", "--instance", instance, "--space", space],
    ]
    for args in invalid_resource_targets:
        result = adrive_cli_runner.run(args)
        assert result.exit_code != 0, f"{args} should be routed to ve-adrive del"
        assert "ve-adrive del" in (result.stderr + result.stdout)

    cases = [
        ["ve-adrive", "rm", "--instance", instance, "--space", space, "--folder", "missing"],
        [
            "ve-adrive",
            "rm",
            "--instance",
            instance,
            "--space",
            space,
            "--folder",
            "missing",
            "--file",
            "missing.txt",
        ],
    ]
    for args in cases:
        result = adrive_cli_runner.run(args)
        assert result.exit_code != 0, f"{args} should require explicit confirmation"
        assert "--force" in (result.stderr + result.stdout)

        forced = adrive_cli_runner.run([*args, "--force"])
        assert forced.exit_code != 0, f"{args} should require exact confirmation after --force"
        assert "--confirm" in (forced.stderr + forced.stdout)

    dry_run = adrive_cli_runner.run(
        [
            "ve-adrive",
            "rm",
            f"adrive://{instance}/{space}/missing/",
            "--recursive",
            "--recursive-delete-mode",
            "bottom-up",
            "--force",
            "--confirm",
            f"adrive://{instance}/{space}/missing/",
            "--dry-run",
        ]
    )
    assert dry_run.exit_code == 0, dry_run.stderr
    payload = dry_run.payload()
    assert payload["dry_run"] is True
    assert payload["summary"]["recursive_delete_mode"] == "bottom-up"


@pytest.mark.adrive
def test_adrive_sync_delete_requires_force_before_mutation(
    adrive_cli_runner: CliRunner,
    adrive_workspace: ADriveWorkspace,
    tmp_path: Path,
) -> None:
    source = tmp_path / "sync-src"
    source.mkdir()
    (source / "local.txt").write_text("sync safety\n", encoding="utf-8")

    result = adrive_cli_runner.run(
        [
            "ve-adrive",
            "sync",
            str(source),
            f"adrive://{adrive_workspace.instance}/{adrive_workspace.space}/{adrive_workspace.root}/sync/",
            "--delete",
        ]
    )
    assert result.exit_code != 0
    assert "--force" in (result.stderr + result.stdout)


@pytest.mark.adrive
@pytest.mark.live
def test_adrive_live_ls_instances_spaces_and_files(
    adrive_cli_runner: CliRunner,
    live_adrive_workspace: ADriveWorkspace,
) -> None:
    instance = live_adrive_workspace.instance
    space = live_adrive_workspace.space

    for args in (
        ["ve-adrive", "ls"],
        ["ve-adrive", "ls", "--instance", instance],
        ["ve-adrive", "ls", "--instance", instance, "--space", space],
    ):
        result = adrive_cli_runner.run(args)
        assert result.exit_code == 0, f"{args}: {result.stderr}"
        envelope_assert.assert_success_envelope(result.require_envelope())


@pytest.mark.adrive
def test_adrive_mv_same_space_dry_run(
    adrive_cli_runner: CliRunner,
    adrive_workspace: ADriveWorkspace,
) -> None:
    instance = adrive_workspace.instance
    space = adrive_workspace.space
    result = adrive_cli_runner.run(
        [
            "ve-adrive",
            "mv",
            f"adrive://{instance}/{space}/src.txt",
            f"adrive://{instance}/{space}/dst.txt",
            "--dry-run",
        ]
    )
    assert result.exit_code == 0, result.stderr
    payload = result.payload()
    assert payload["command"] == "ve-adrive mv"
    assert payload["dry_run"] is True


@pytest.mark.adrive
@pytest.mark.live
def test_adrive_live_high_level_lifecycle_with_isolated_resources(
    adrive_cli_runner: CliRunner,
    live_adrive_workspace: ADriveWorkspace,
    tmp_path: Path,
) -> None:
    instance = live_adrive_workspace.instance
    space = live_adrive_workspace.space
    root = live_adrive_workspace.root
    local_root = tmp_path / "upload"
    local_root.mkdir()
    payload = local_root / "hello.txt"
    payload.write_text("hello ve-adrive e2e\n", encoding="utf-8")
    download_dir = tmp_path / "download"

    lifecycle_failed = False
    try:
        mkdir = adrive_cli_runner.run(
            ["ve-adrive", "mkdir", f"adrive://{instance}/{space}/{root}/"],
            timeout=180.0,
        )
        assert mkdir.exit_code == 0, mkdir.stderr
        envelope_assert.assert_success_envelope(mkdir.require_envelope())

        upload = adrive_cli_runner.run(
            [
                "ve-adrive",
                "cp",
                str(payload),
                f"adrive://{instance}/{space}/{root}/hello.txt",
            ],
            timeout=300.0,
        )
        assert upload.exit_code == 0, upload.stderr
        envelope_assert.assert_success_envelope(upload.require_envelope())

        listed = adrive_cli_runner.run(["ve-adrive", "ls", f"adrive://{instance}/{space}/{root}/"])
        assert listed.exit_code == 0, listed.stderr
        envelope_assert.assert_success_envelope(listed.require_envelope())
        assert any(entry["file_path"].endswith("hello.txt") for entry in listed.payload()["entries"])

        stat = adrive_cli_runner.run(["ve-adrive", "stat", f"adrive://{instance}/{space}/{root}/hello.txt"])
        assert stat.exit_code == 0, stat.stderr
        envelope_assert.assert_success_envelope(stat.require_envelope())
        assert stat.payload()["size"] > 0

        cat = adrive_cli_runner.run(
            ["ve-adrive", "cat", f"adrive://{instance}/{space}/{root}/hello.txt"],
            json_output=False,
        )
        assert cat.exit_code == 0, cat.stderr
        assert cat.stdout == "hello ve-adrive e2e\n"

        move = adrive_cli_runner.run(
            [
                "ve-adrive",
                "mv",
                f"adrive://{instance}/{space}/{root}/hello.txt",
                f"adrive://{instance}/{space}/{root}/moved.txt",
                "--force",
                "--confirm",
                f"adrive://{instance}/{space}/{root}/hello.txt",
            ],
            timeout=300.0,
        )
        assert move.exit_code == 0, move.stderr
        envelope_assert.assert_success_envelope(move.require_envelope())

        download = adrive_cli_runner.run(
            [
                "ve-adrive",
                "cp",
                f"adrive://{instance}/{space}/{root}/moved.txt",
                str(download_dir / "moved.txt"),
            ],
            timeout=300.0,
        )
        assert download.exit_code == 0, download.stderr
        envelope_assert.assert_success_envelope(download.require_envelope())
        assert (download_dir / "moved.txt").read_text(encoding="utf-8") == "hello ve-adrive e2e\n"

        du = adrive_cli_runner.run(["ve-adrive", "du", f"adrive://{instance}/{space}/{root}/"])
        assert du.exit_code == 0, du.stderr
        envelope_assert.assert_success_envelope(du.require_envelope())
        assert du.payload()["files"] >= 1

        recursive_report = tmp_path / "recursive-report.json"
        recursive = adrive_cli_runner.run(
            [
                "ve-adrive",
                "cp",
                str(local_root),
                f"adrive://{instance}/{space}/{root}/recursive/",
                "--recursive",
                "--report-path",
                str(recursive_report),
            ],
            timeout=300.0,
        )
        assert recursive.exit_code == 0, recursive.stderr
        envelope_assert.assert_success_envelope(recursive.require_envelope())
        recursive_payload = recursive.payload()
        assert recursive_payload["summary"]["succeeded"] >= 1
        assert recursive_payload["manifest"]["succeeded"] >= 1
        assert recursive_payload["manifest"]["items"]
        assert recursive_report.exists()
        assert json.loads(recursive_report.read_text(encoding="utf-8"))["succeeded"] >= 1
    except Exception:
        lifecycle_failed = True
        raise
    finally:
        cleanup_target = f"adrive://{instance}/{space}/{root}/"
        cleanup = adrive_cli_runner.run(
            [
                "ve-adrive",
                "rm",
                cleanup_target,
                "--recursive",
                "--recursive-delete-mode",
                "bottom-up",
                "--force",
                "--confirm",
                cleanup_target,
            ],
            timeout=300.0,
        )
        if cleanup.exit_code == 0:
            envelope_assert.assert_success_envelope(cleanup.require_envelope())
        elif lifecycle_failed:
            print(
                f"[adrive_lifecycle] WARN: failed to delete {root}: "
                f"exit={cleanup.exit_code}, stderr={cleanup.stderr[:300]}",
                file=sys.stderr,
            )
        else:
            assert cleanup.exit_code == 0, cleanup.stderr
