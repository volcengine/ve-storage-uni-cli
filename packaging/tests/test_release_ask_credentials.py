"""Synthetic-only release preflight and bundled Ask artifact checks."""

import argparse
import os
import subprocess
import traceback

import pytest

from test_release import load_release_module


ENV_NAMES = (
    "STORAGE_CLI_BUILD_DOUBAO_APP_ID",
    "STORAGE_CLI_BUILD_DOUBAO_APP_SECRET",
)
VALID_VALUES = ("synthetic_app-1", "synthetic-secret-value")
TARGET = "x86_64-unknown-linux-gnu.2.17"


@pytest.fixture
def release(monkeypatch):
    # Replace the mapping wholesale so tests never inspect publisher environment values.
    monkeypatch.setattr(os, "environ", {})
    return load_release_module()


def set_bundle(monkeypatch, values=VALID_VALUES):
    monkeypatch.setattr(os, "environ", dict(zip(ENV_NAMES, values)))


def archive_args(tmp_path, is_required=False, package="ve-adrive-cli"):
    return argparse.Namespace(
        target=[TARGET], skip_build=False, target_dir=tmp_path / "target",
        out_dir=tmp_path / "dist", metadata=tmp_path / "platforms.json",
        package=package, require_ask_credentials=is_required,
    )


def isolate_archives(release, monkeypatch):
    events = []
    monkeypatch.setattr(release, "run_command", lambda command: events.append(command))
    monkeypatch.setattr(release, "verify_native_bytetos_auth", lambda target: None)
    monkeypatch.setattr(release, "native_target_matches_host", lambda target: False)
    return events


@pytest.mark.parametrize("command", ["archives", "all"])
def test_archive_commands_accept_require_ask_credentials(release, command):
    argv = [command, "--target", TARGET, "--require-ask-credentials"]
    if command == "all":
        argv.extend(["--version", "1.2.3"])
    assert release.build_parser().parse_args(argv).require_ask_credentials is True


@pytest.mark.parametrize("is_required", [False, True])
def test_complete_bundle_builds_using_inherited_environment(
    release, tmp_path, monkeypatch, capsys, is_required
):
    set_bundle(monkeypatch)
    events = isolate_archives(release, monkeypatch)
    release.run_archives(archive_args(tmp_path, is_required))
    assert len(events) == 2
    assert events[0][0:2] == ["cargo", "zigbuild"]
    assert VALID_VALUES[1] == os.environ[ENV_NAMES[1]]
    assert not any(value in str(events) for value in VALID_VALUES)
    captured = capsys.readouterr()
    assert not any(value in captured.out + captured.err for value in VALID_VALUES)


def test_missing_optional_bundle_preserves_legacy_namespace(release, tmp_path, monkeypatch):
    events = isolate_archives(release, monkeypatch)
    args = archive_args(tmp_path)
    del args.require_ask_credentials
    args.skip_build = True
    release.run_archives(args)
    assert len(events) == 1


@pytest.mark.parametrize("command", ["archives", "all"])
def test_missing_required_bundle_stops_before_all_side_effects(
    release, tmp_path, monkeypatch, capsys, command
):
    events = isolate_archives(release, monkeypatch)
    argv = [command, "--target", TARGET, "--require-ask-credentials"]
    if command == "all":
        argv.extend(["--version", "1.2.3"])
    args = release.build_parser().parse_args(argv)
    with pytest.raises(SystemExit, match="Ask"):
        args.func(args)
    assert events == []
    assert capsys.readouterr().out == ""


@pytest.mark.parametrize("is_required", [False, True])
@pytest.mark.parametrize("mask", range(1, 3))
def test_partial_bundle_always_rejected_without_side_effects(
    release, tmp_path, monkeypatch, is_required, mask
):
    monkeypatch.setattr(os, "environ", {
        name: value for index, (name, value) in enumerate(zip(ENV_NAMES, VALID_VALUES))
        if mask & (1 << index)
    })
    events = isolate_archives(release, monkeypatch)
    with pytest.raises(SystemExit):
        release.run_archives(archive_args(tmp_path, is_required))
    assert events == []


INVALID_FIELDS = [
    (0, ""), (0, "bad/id"), (0, "a" * 257), (0, "应用"), (0, "bad\udcff"),
    (1, ""), (1, " spaced"), (1, "space inside"), (1, "line\nbreak"),
    (1, "\x7f"), (1, "☃"), (1, "s" * 4097), (1, "bad\udcff"),

]


# [Review Fix #1] Group each case to keep test signatures within five arguments.
@pytest.mark.parametrize("invalid_case", [
    (is_required, index, invalid_value)
    for is_required in (False, True) for index, invalid_value in INVALID_FIELDS
], ids=range(2 * len(INVALID_FIELDS)))
def test_invalid_bundle_is_rejected_with_value_free_errors(
    release, tmp_path, monkeypatch, capsys, invalid_case
):
    is_required, index, invalid_value = invalid_case
    values = list(VALID_VALUES)
    values[index] = invalid_value
    set_bundle(monkeypatch, values)
    events = isolate_archives(release, monkeypatch)
    with pytest.raises(SystemExit) as caught:
        release.run_archives(archive_args(tmp_path, is_required))
    assert events == []
    assert VALID_VALUES[1] not in str(caught.value)
    assert "synthetic_app" not in str(caught.value)
    captured = capsys.readouterr()
    assert captured.out == captured.err == ""


def test_valid_boundary_bundles_are_accepted(release, tmp_path, monkeypatch):
    set_bundle(monkeypatch, ("A" * 256, "!" * 4096))
    events = isolate_archives(release, monkeypatch)
    release.run_archives(archive_args(tmp_path, True))
    assert len(events) == 2


def test_required_credentials_reject_skip_build(release, tmp_path, monkeypatch):
    set_bundle(monkeypatch)
    events = isolate_archives(release, monkeypatch)
    args = archive_args(tmp_path, True)
    args.skip_build = True
    with pytest.raises(SystemExit, match="--skip-build"):
        release.run_archives(args)
    assert events == []


def create_binaries(release, tmp_path, package=None, target=TARGET):
    selected = (package,) if package else release.PUBLIC_PACKAGES
    for entry_name in selected:
        binary_dir = release.entry_release_dir(entry_name, release.cargo_artifact_target(target))
        binary_dir.mkdir(parents=True, exist_ok=True)
        suffix = ".exe" if target.endswith("windows-msvc") else ""
        (binary_dir / f"{entry_name}{suffix}").write_text("synthetic", encoding="utf-8")


@pytest.mark.parametrize("selection", [
    (None, TARGET), ("tos-cli", TARGET), ("ve-tos-cli", TARGET), ("ve-adrive-cli", TARGET),
    ("ve-adrive-cli", "x86_64-pc-windows-msvc"),
])
def test_native_selected_artifacts_checked_after_build_before_archive(
    release, tmp_path, monkeypatch, capsys, selection
):
    package, target = selection
    set_bundle(monkeypatch)
    monkeypatch.setattr(release, "ROOT", tmp_path)
    create_binaries(release, tmp_path, package, target)
    events = []
    monkeypatch.setattr(release, "native_target_matches_host", lambda target: True)
    monkeypatch.setattr(release, "verify_native_bytetos_auth", lambda target: None)
    monkeypatch.setattr(release, "run_command", lambda argv: events.append(("command", argv)))

    def describe(argv, **options):
        events.append(("describe", argv))
        assert options == {
            "cwd": tmp_path, "capture_output": True, "text": True,
            "check": False, "timeout": 10,
        }
        return subprocess.CompletedProcess(argv, 0, '{"data":{"application_bundled":true}}', "")

    monkeypatch.setattr(release.subprocess, "run", describe)
    args = archive_args(tmp_path, True, package)
    args.target = [target]
    release.run_archives(args)
    selected = (package,) if package else release.PUBLIC_PACKAGES
    count = len(selected)
    assert [kind for kind, _ in events] == ["command"] * count + ["describe"] * count + ["command"]
    for entry_name, (_, argv) in zip(selected, events[count:-1]):
        suffix = ".exe" if target.endswith("windows-msvc") else ""
        expected = release.entry_release_dir(
            entry_name, release.cargo_artifact_target(target)
        ) / f"{entry_name}{suffix}"
        assert argv == [str(expected), "ask", "--describe", "--output", "json"]
    assert not any(value in str(events) for value in VALID_VALUES)
    captured = capsys.readouterr()
    assert not any(value in captured.out + captured.err for value in VALID_VALUES)


def test_cross_target_reports_pending_target_runner_validation(release, tmp_path, monkeypatch, capsys):
    set_bundle(monkeypatch)
    events = isolate_archives(release, monkeypatch)
    monkeypatch.setattr(release.subprocess, "run", lambda *args, **kwargs: pytest.fail("cross execution"))
    release.run_archives(archive_args(tmp_path, True))
    assert len(events) == 2
    output = capsys.readouterr().out.lower()
    assert "skip" in output and "target-runner validation" in output and "required" in output


BAD_RESULTS = [
    (0, '{"data":{"application_bundled":false}}'),
    (0, '{"data":{"application_bundled":1}}'),
    (0, '{"data":{"application_bundled":"true"}}'),
    (0, '{"data":{}}'), (0, '{"application_bundled":true}'),
    (0, '{"data":null}'), (0, '[]'), (0, 'not json'),
    (0, VALID_VALUES[1]),
    (1, '{"data":{"application_bundled":true}}'),
]


@pytest.mark.parametrize("bad_result", BAD_RESULTS, ids=range(len(BAD_RESULTS)))
def test_native_invalid_description_blocks_archive_and_redacts_payload(
    release, tmp_path, monkeypatch, capsys, bad_result
):
    returncode, stdout = bad_result
    set_bundle(monkeypatch)
    monkeypatch.setattr(release, "ROOT", tmp_path)
    create_binaries(release, tmp_path, "ve-adrive-cli")
    events = isolate_archives(release, monkeypatch)
    monkeypatch.setattr(release, "native_target_matches_host", lambda target: True)
    monkeypatch.setattr(release.subprocess, "run", lambda argv, **kwargs: subprocess.CompletedProcess(
        argv, returncode, stdout, VALID_VALUES[1],
    ))
    with pytest.raises(SystemExit) as caught:
        release.run_archives(archive_args(tmp_path, True))
    assert len(events) == 1
    formatted = "".join(traceback.format_exception(caught.type, caught.value, caught.tb))
    assert VALID_VALUES[1] not in formatted
    captured = capsys.readouterr()
    assert captured.out == captured.err == ""


def test_build_and_archive_subprocesses_inherit_only_environment_credentials(
    release, tmp_path, monkeypatch, capsys
):
    set_bundle(monkeypatch)
    monkeypatch.setattr(release, "native_target_matches_host", lambda target: False)
    calls = []

    def run(argv, **options):
        assert "env" not in options
        assert os.environ == dict(zip(ENV_NAMES, VALID_VALUES))
        calls.append(argv)
        return subprocess.CompletedProcess(argv, 0)

    monkeypatch.setattr(release.subprocess, "run", run)
    release.run_archives(archive_args(tmp_path, True))
    assert len(calls) == 2
    captured = capsys.readouterr()
    assert not any(value in str(calls) + captured.out + captured.err for value in VALID_VALUES)


def test_optional_bundle_does_not_add_native_artifact_checks(release, tmp_path, monkeypatch):
    set_bundle(monkeypatch)
    events = isolate_archives(release, monkeypatch)
    monkeypatch.setattr(release, "native_target_matches_host", lambda target: True)
    monkeypatch.setattr(release.subprocess, "run", lambda *args, **kwargs: pytest.fail("new check"))
    release.run_archives(archive_args(tmp_path))
    assert len(events) == 2


@pytest.mark.parametrize("failure", ["missing", "oserror", "timeout", "unicode"])
def test_native_inspection_failures_are_redacted(release, tmp_path, monkeypatch, capsys, failure):
    set_bundle(monkeypatch)
    monkeypatch.setattr(release, "ROOT", tmp_path)
    if failure != "missing":
        create_binaries(release, tmp_path, "ve-adrive-cli")
    events = isolate_archives(release, monkeypatch)
    monkeypatch.setattr(release, "native_target_matches_host", lambda target: True)

    def describe(argv, **kwargs):
        if failure == "timeout":
            raise subprocess.TimeoutExpired(argv, 10, output=VALID_VALUES[1], stderr=VALID_VALUES[1])
        if failure == "unicode":
            raise UnicodeError(VALID_VALUES[1])
        raise OSError(VALID_VALUES[1])

    monkeypatch.setattr(release.subprocess, "run", describe)
    with pytest.raises(SystemExit) as caught:
        release.run_archives(archive_args(tmp_path, True))
    formatted = "".join(traceback.format_exception(caught.type, caught.value, caught.tb))
    assert VALID_VALUES[1] not in formatted
    assert len(events) == 1
    captured = capsys.readouterr()
    assert captured.out == captured.err == ""


def test_publisher_build_only_requires_app_id_and_secret(release, tmp_path, monkeypatch):
    monkeypatch.setattr(os, "environ", dict(zip(ENV_NAMES[:2], VALID_VALUES[:2])))
    events = isolate_archives(release, monkeypatch)
    release.run_archives(archive_args(tmp_path, True))
    assert len(events) == 2


@pytest.mark.parametrize("retired_scope_value", ["", "unrelated:admin", "bad\udcff"])
def test_retired_build_scopes_cannot_change_preflight(
    release, tmp_path, monkeypatch, retired_scope_value
):
    set_bundle(monkeypatch)
    monkeypatch.setitem(os.environ, "STORAGE_CLI_BUILD_DOUBAO_SCOPES", retired_scope_value)
    events = isolate_archives(release, monkeypatch)
    release.run_archives(archive_args(tmp_path, True))
    assert len(events) == 2
