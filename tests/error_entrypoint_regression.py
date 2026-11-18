"""Verify error usage and repair commands through unified and dedicated binaries.

Run with all four debug binaries. Uses isolated configuration and makes no HTTP requests.
"""
import json
from pathlib import Path
import subprocess
import sys
import tempfile

from directory_upload_regression import fixture_environment


DIRECT_SURFACES = {"ve-adrive-cli": "ve-adrive", "ve-tos-cli": "ve-tos", "tos-cli": "tos"}


def invoke(binary, surface, root, arguments):
    prefix = [surface] if binary.name == "ve-storage-uni-cli" else []
    result = subprocess.run(
        [str(binary), "--config-path", str(root / "config"), "--credentials-path",
         str(root / "credentials"), *prefix, *arguments], capture_output=True, text=True,
        env=fixture_environment(), timeout=15)
    assert result.returncode == 6, (arguments, result.stdout, result.stderr)
    assert not result.stdout, (arguments, result.stdout)
    envelope = json.loads(result.stderr)
    assert envelope["command"] == surface or envelope["command"].startswith(surface + " "), envelope
    assert envelope["status"] == "failed" and not envelope["success"], envelope
    assert envelope["request_id"], envelope
    error = envelope["error"]
    assert error["category"] == "InvalidParam" and error["exit_code"] == 6, error
    assert error["kind"] == "validation_error" and error["code"] == envelope["ec"], error
    return envelope


def check_parse_errors(binary, surface, root, prefix):
    cases = [(["cp"], "cp"), (["skill"], "skill"),
             (["ls", "--unknown-entrypoint-flag"], "ls"),
             (["ls", "--output", "invalid-format"], None),
             (["unknown-entrypoint-command"], "")]
    for arguments, command in cases:
        envelope = invoke(binary, surface, root, arguments)
        error = envelope["error"]
        assert error["code"] == "ValidationError", error
        if command is not None:
            usage = error["message"].split("Usage: ", 1)[1].splitlines()[0]
            assert usage.startswith(prefix + " " + command), error
        assert error["fix_command"] == f"{prefix} <command> --help", error
        assert error["doctor_hint"] == f"{prefix} capabilities --view groups", error
        assert f"run {prefix} <command> --help" in error["suggested_action"], error
        if binary.name != "ve-storage-uni-cli":
            assert "ve-storage-uni-cli" not in json.dumps(error), error
        if arguments == ["skill"]:
            assert f"'{prefix} skill' requires a subcommand" in error["message"], error


def check_user_input(binary, surface, root, prefix):
    # Binary names in invalid user values must remain verbatim in the diagnostic.
    supplied_value = f"ve-storage-uni-cli {surface} user-value"
    envelope = invoke(binary, surface, root, [supplied_value])
    message = envelope["error"]["message"]
    assert supplied_value in message, message
    assert f"Usage: {prefix} " in message, message
    if surface == "ve-adrive":
        secret_url = "https://pages.invalid/share/id?pwd=fixture-secret"
        envelope = invoke(binary, surface, root,
                          ["share", "files", secret_url, "--max-keys", secret_url])
        assert "fixture-secret" not in json.dumps(envelope), envelope
        assert "[redacted share URL]" in envelope["error"]["message"], envelope


def check_runtime_guidance(binary, surface, root, prefix):
    # These fail locally before configuration resolution or any network operation.
    arguments = ["--psm", "invalid", "ls"] if surface != "tos" else ["ls"]
    envelope = invoke(binary, surface, root, arguments)
    error = envelope["error"]
    assert error["code"] == "InvalidParam", error
    for field in ("fix_command", "doctor_hint"):
        if field in error:
            assert error[field].startswith(prefix + " "), (field, error)
    if surface == "ve-adrive":
        envelope = invoke(binary, surface, root, ["auth", "login", "--auth-mode", "unified"])
        error = envelope["error"]
        assert error["fix_command"] == "ve login", error
        assert error["doctor_hint"] == f"{prefix} doctor --check auth", error


def check_download_auth_guidance(binary, surface, root, prefix):
    if surface != "ve-adrive":
        return
    (root / "config").write_text("")
    tool = [surface] if binary.name == "ve-storage-uni-cli" else []
    result = subprocess.run(
        [str(binary), *tool, "--config-path", str(root / "config"), "--credentials-path",
         str(root / "credentials"), "share", "download", "http://127.0.0.1:1/share/id",
         str(root / "download"), "--auth-mode", "oauth", "--output", "json",
         "--report-path", str(root / "report.csv"), "--manifest-path", str(root / "manifest.csv")],
        capture_output=True, text=True, env=fixture_environment(), timeout=15)
    assert result.returncode == 3 and not result.stderr, (result.stdout, result.stderr)
    envelope = json.loads(result.stdout)
    assert envelope["command"] == "ve-adrive share download", envelope
    assert envelope["error"]["code"] == "login_required", envelope
    assert envelope["error"]["doctor_hint"] == f"{prefix} doctor --check auth", envelope
    assert envelope["error"]["fix_command"] == f"{prefix} auth login", envelope
    assert not (root / "download").exists(), list(root.iterdir())
    (root / "config").write_text("schema_version=1\n")


def check_binary(binary, surface):
    prefix = f"{binary.name} {surface}" if binary.name == "ve-storage-uni-cli" else binary.name
    with tempfile.TemporaryDirectory(prefix="error-entrypoint-") as directory:
        root = Path(directory)
        for name in ("config", "credentials"):
            (root / name).write_text("schema_version=1\n")
        check_parse_errors(binary, surface, root, prefix)
        check_user_input(binary, surface, root, prefix)
        check_runtime_guidance(binary, surface, root, prefix)
        check_download_auth_guidance(binary, surface, root, prefix)
        for name in ("config", "credentials"):
            assert (root / name).read_text() == "schema_version=1\n"
        assert {path.name for path in root.iterdir()} == {"config", "credentials"}
    print(f"PASS: {prefix}; parse errors, repair commands, opaque input and password redaction")


if __name__ == "__main__":
    assert len(sys.argv) > 1, "pass unified and/or standalone binary paths"
    for argument in sys.argv[1:]:
        binary = Path(argument).resolve()
        surfaces = ([DIRECT_SURFACES[binary.name]] if binary.name in DIRECT_SURFACES
                    else DIRECT_SURFACES.values())
        for surface in surfaces:
            check_binary(binary, surface)
