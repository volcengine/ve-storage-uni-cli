"""Audit executable instructions across dedicated and unified CLI surfaces.

Uses isolated profiles, local validation, offline doctor and metadata only.
"""
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile

from directory_upload_regression import fixture_environment


SURFACES = {"ve-adrive-cli": "ve-adrive", "ve-tos-cli": "ve-tos", "tos-cli": "tos"}


def invoke(binary, surface, root, arguments, environment=None):
    tool = [surface] if binary.name == "ve-storage-uni-cli" else []
    result = subprocess.run(
        [str(binary), *tool, "--config-path", str(root / "config"), "--credentials-path",
         str(root / "credentials"), "--output", "json", *arguments],
        capture_output=True, text=True, env=environment or fixture_environment(), timeout=20)
    return result, json.loads(result.stdout or result.stderr)


def check_doctor(binary, surface, root, prefix):
    for check in ("auth", "config", "network", "registry", "mcp", "completion"):
        result, envelope = invoke(binary, surface, root, ["doctor", "--check", check])
        assert result.returncode == 0, envelope
        assert envelope["command"] == surface + " doctor", envelope
        for item in envelope["data"]["checks"]:
            command = item.get("fix_command")
            if command and command not in ("ve login", "ve logout"):
                assert command.startswith(prefix + " "), item


def check_local_errors(binary, surface, root, prefix):
    result, envelope = invoke(binary, surface, root, ["config"])
    assert result.returncode == 6, envelope
    assert f"{prefix} config --help" in envelope["error"]["message"], envelope
    if surface == "ve-adrive":
        result, envelope = invoke(binary, surface, root,
                                  ["--auth-mode", "oauth", "ls", "adrive://instance/space/"])
        assert result.returncode == 3, envelope
        assert f"run {prefix} auth login" in envelope["error"]["message"], envelope
        result, envelope = invoke(binary, surface, root,
                                  ["crt", "adrive://instance/space/path"])
        assert result.returncode == 6, envelope
        assert f"use {prefix} mkdir" in envelope["error"]["message"], envelope
        environment = fixture_environment()
        environment["ADRIVE_ACCESS_TOKEN"] = "fixture-token"
        result, envelope = invoke(binary, surface, root,
                                  ["crt", "adrive://instance/newspace", "--auth-mode", "oauth", "--dry-run"],
                                  environment)
        assert result.returncode == 6, envelope
        for field in ("message", "suggested_action"):
            assert f"{prefix} auth login --instance" in envelope["error"][field], envelope
        source = "adrive://instance/space/ve-adrive auth login"
        confirm = "adrive://instance/space/ve-adrive doctor"
        result, envelope = invoke(binary, surface, root,
                                  ["rm", source, "--force", "--confirm", confirm])
        assert result.returncode == 6, envelope
        assert source in envelope["error"]["message"] and confirm in envelope["error"]["message"], envelope
    target = root / "unreadable-request"
    target.mkdir(exist_ok=True)
    result, envelope = invoke(binary, surface, root,
                              ["api", "file", "list", "--request", f"file://{target}"])
    assert result.returncode == 1, envelope
    assert f"run {prefix} doctor" in envelope["error"]["suggested_action"], envelope


def check_cross_service(binary, surface, root):
    if surface != "tos":
        return
    for key, value, recommended in (
        ("default.adrive.auth_mode", "oauth", "ve-adrive config set auth_mode"),
        ("default.tos.control_endpoint", "https://example.invalid", "ve-tos config set control_endpoint"),
    ):
        result, envelope = invoke(binary, surface, root, ["config", "set", key, value, "--dry-run"])
        assert result.returncode == 6, envelope
        assert f"ve-storage-uni-cli {recommended}" in envelope["error"]["message"], envelope


def check_tos_validation_and_plans(binary, surface, root, prefix):
    if surface == "ve-adrive":
        return
    result, envelope = invoke(binary, surface, root,
                              ["cp", "/src", "/dst", "--content-type", "text/plain", "--dry-run"])
    assert result.returncode == 6 and f"{prefix} cp" in envelope["error"]["message"], envelope
    if surface != "ve-tos":
        return
    for group in ("object", "bucket", "multipart", "turbo", "quota", "versioning", "data-process", "ap", "accelerator"):
        result, envelope = invoke(binary, surface, root, [group])
        assert result.returncode == 6, envelope
        assert f"{prefix} {group} --help" in envelope["error"]["message"], envelope
    for command in (["quota", "set"], ["versioning", "get"], ["ap", "create"], ["data-process", "set-template"]):
        for language in ("en", "zh"):
            result, envelope = invoke(binary, surface, root,
                                      [*command, "--describe", "--language", language])
            assert result.returncode == 0, envelope
            assert envelope["data"]["command"] == " ".join([surface, *command]), envelope
    environment = fixture_environment()
    environment.update(TOS_ACCESS_KEY="fixture-ak", TOS_SECRET_KEY="fixture-sk")
    flags = ["--endpoint", "http://127.0.0.1:1", "--region", "fixture-region"]
    result, envelope = invoke(binary, surface, root,
                              [*flags, "bucket", "head", "--bucket", "bucket", "--dry-run"], environment)
    assert result.returncode == 0, envelope
    assert envelope["data"]["confirm_command"] == f"{prefix} bucket head tos://bucket", envelope
    for command in (["bucket", "delete", "--bucket", "bucket"], ["object", "delete", "tos://bucket/key"]):
        result, envelope = invoke(binary, surface, root, [*flags, *command], environment)
        assert result.returncode == 6, envelope
        assert f"'{prefix} {command[0]} delete'" in envelope["error"]["message"], envelope


def check_help_environment(binary, surface):
    environment = fixture_environment()
    variable = "BYTE_TOS_PROFILE" if surface == "tos" else "TOS_PROFILE"
    tool = [surface] if binary.name == "ve-storage-uni-cli" else []
    for profile in (f"{surface} doctor", "staging.ve-tos.endpoint", f"Usage: {surface} doctor",
                    f"x]\n{surface} doctor", f"x\n[env: TOS_PROFILE={surface} doctor"):
        environment[variable] = profile
        for language in ("en", "zh"):
            result = subprocess.run([str(binary), *tool, "ls", "--help", "--language", language],
                                    env=environment, capture_output=True, text=True, timeout=15)
            assert result.returncode == 0, result.stderr
            assert f"{variable}={profile}]" in result.stdout, (profile, language, result.stdout)


def check_discovery(binary, surface, root, prefix):
    if surface == "ve-tos":
        result, envelope = invoke(binary, surface, root, ["rb", "--describe"])
        assert result.returncode == 0, envelope
        assert envelope["data"]["command"] == "ve-tos rb", envelope
        assert f"use {prefix} rm" in envelope["data"]["description"], envelope
    result, envelope = invoke(binary, surface, root, ["config", "init", "--dry-run"])
    assert result.returncode == 0, envelope
    serialized = json.dumps(envelope)
    assert "confirm_command" in serialized, envelope
    # Only command-bearing presentation fields are inspected, never canonical IDs.
    def visit(value):
        if isinstance(value, dict):
            for name, child in value.items():
                if name in ("confirm_command", "fix_command", "doctor_hint") and isinstance(child, str):
                    assert child.startswith(prefix + " "), (name, child, prefix)
                visit(child)
        elif isinstance(value, list):
            for child in value:
                visit(child)
    visit(envelope)
    if surface == "tos":
        arguments = ([surface] if binary.name == "ve-storage-uni-cli" else []) + ["serve", "--help"]
        result = subprocess.run([str(binary), *arguments], capture_output=True, text=True,
                                env=fixture_environment(), timeout=15)
        assert result.returncode == 0, result.stderr
        assert f"  {prefix} serve --mcp" in result.stdout, result.stdout
        assert not re.search(r"(?m)^  tos serve ", result.stdout), result.stdout


def check_binary(binary, surface):
    prefix = f"{binary.name} {surface}" if binary.name == "ve-storage-uni-cli" else binary.name
    with tempfile.TemporaryDirectory(prefix="command-presentation-") as directory:
        root = Path(directory)
        section = "adrive" if surface == "ve-adrive" else surface
        original = f"[default.{section}]\nauth_mode='aksk'\n"
        (root / "config").write_text(original)
        (root / "credentials").write_text("schema_version=1\n")
        check_doctor(binary, surface, root, prefix)
        check_local_errors(binary, surface, root, prefix)
        check_cross_service(binary, surface, root)
        check_tos_validation_and_plans(binary, surface, root, prefix)
        check_discovery(binary, surface, root, prefix)
        check_help_environment(binary, surface)
        assert (root / "config").read_text() == original
        assert (root / "credentials").read_text() == "schema_version=1\n"
    print(f"PASS: {prefix}; doctor, messages, remediation, discovery and dry-run instructions")


if __name__ == "__main__":
    assert len(sys.argv) > 1, "pass unified and/or standalone binary paths"
    for argument in sys.argv[1:]:
        binary = Path(argument).resolve()
        for surface in ([SURFACES[binary.name]] if binary.name in SURFACES else SURFACES.values()):
            check_binary(binary, surface)
