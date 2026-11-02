"""Verify same-named profiles through real CLI and MCP entrypoints, without HTTP."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace

from directory_upload_regression import Mcp, fixture_environment


PROFILES = ("tos", "ve-tos", "ve-adrive")


def command_parts(surface):
    return ["share", "download"] if surface == "ve-adrive" else ["cp"]


def check_cli(binary, surface, root):
    command = command_parts(surface)
    prefix = [surface] if binary.name == "ve-storage-uni-cli" else []
    for profile in PROFILES:
        for options in (["--profile", profile], ["-P", profile], [f"--profile={profile}"],
                        [f"-P{profile}"], ["-qP", profile]):
            arguments = [str(binary), *options, *prefix, *command, "--describe", "--output", "json",
                         "--config-path", str(root / "config"),
                         "--credentials-path", str(root / "credentials")]
            result = subprocess.run(arguments, capture_output=True, text=True,
                                    env=fixture_environment(), timeout=15)
            assert result.returncode == 0, (arguments, result.stdout, result.stderr)
            envelope = json.loads(result.stdout)
            expected = " ".join([surface, *command])
            assert envelope["command"] == expected, (arguments, envelope)
            assert envelope["data"]["command"] == expected, (arguments, envelope)


def check_mcp(binary, surface, root):
    rpc = Mcp(SimpleNamespace(binary=binary, surface=surface, root=root))
    try:
        rpc.call(1, "initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                                  "clientInfo": {"name": "describe-identity", "version": "1"}})
        rpc.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        suffix = "_share_download" if surface == "ve-adrive" else "_cp"
        tool = next(value for value in rpc.call(2, "tools/list", {})["tools"]
                    if value["name"].endswith(suffix))
        for identifier, profile in enumerate(PROFILES, 3):
            arguments = {"profile": profile, "describe": True, "execute": True}
            if surface != "ve-adrive":
                arguments.update(source=str(root / "unused"), destination="tos://bucket/unused")
            response = rpc.call(identifier, "tools/call", {"name": tool["name"], "arguments": arguments})
            assert not response.get("isError"), response
            execution = json.loads(response["content"][0]["text"])
            assert execution["exit_code"] == 0, execution
            envelope = json.loads(execution["stdout"])
            expected = " ".join([surface, *command_parts(surface)])
            assert envelope["command"] == expected, execution
            assert envelope["data"]["command"] == expected, execution
            assert not (root / "unused").exists()
    finally:
        rpc.close()


def check_binary(binary, surface):
    with tempfile.TemporaryDirectory(prefix="describe-identity-") as directory:
        root = Path(directory)
        for name in ("config", "credentials"):
            (root / name).write_text("schema_version=1\n")
        check_cli(binary, surface, root)
        check_mcp(binary, surface, root)
        for name in ("config", "credentials"):
            assert (root / name).read_text() == "schema_version=1\n"
        assert {path.name for path in root.iterdir()} == {"config", "credentials"}
        print(f"PASS: {binary.name} {surface}; 15 CLI / 3 MCP profile cases, no filesystem writes")


if __name__ == "__main__":
    assert len(sys.argv) > 1, "pass unified and/or standalone binaries"
    for argument in sys.argv[1:]:
        binary = Path(argument).resolve()
        surfaces = {"tos-cli": ["tos"], "ve-tos-cli": ["ve-tos"], "ve-adrive-cli": ["ve-adrive"]}
        for surface in surfaces.get(binary.name, ["tos", "ve-tos", "ve-adrive"]):
            check_binary(binary, surface)
