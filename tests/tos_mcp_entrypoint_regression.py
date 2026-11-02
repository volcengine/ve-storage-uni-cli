"""Execute read-only MCP Describe through unified/direct ve-tos binaries."""
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading


class Rpc:
    def __init__(self, binary, root):
        prefix = [] if binary.name == "ve-tos-cli" else ["ve-tos"]
        environment = {key: value for key, value in os.environ.items()
                       if not key.startswith(("TOS_", "BYTE_TOS_", "ADRIVE_"))}
        self.process = subprocess.Popen([str(binary), *prefix, "serve", "--mcp", "--transport", "stdio",
            "--config-path", str(root / "config"), "--credentials-path", str(root / "credentials")],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=environment)
        self.messages = queue.Queue()
        self.reader = threading.Thread(target=self.read, daemon=True)
        self.reader.start()

    def read(self):
        for line in self.process.stdout:
            self.messages.put(json.loads(line))

    def send(self, value):
        self.process.stdin.write(json.dumps(value) + "\n")
        self.process.stdin.flush()

    def call(self, identifier, method, params):
        self.send({"jsonrpc": "2.0", "id": identifier, "method": method, "params": params})
        while True:
            message = self.messages.get(timeout=15)
            if message.get("id") == identifier:
                assert "error" not in message, message
                return message["result"]

    def close(self):
        self.process.terminate()
        self.process.communicate(timeout=5)
        self.reader.join(timeout=5)


def check(binary):
    with tempfile.TemporaryDirectory(prefix="tos-mcp-entrypoint-") as directory:
        root = Path(directory)
        (root / "config").write_text("[default.ve-tos]\nauth_mode='aksk'\nendpoint='http://unused.invalid'\nregion='test-region'\n")
        (root / "credentials").write_text("schema_version=1\n")
        rpc = Rpc(binary, root)
        try:
            rpc.call(1, "initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                                      "clientInfo": {"name": "entrypoint-regression", "version": "1"}})
            rpc.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
            result = rpc.call(2, "tools/call", {"name": "ve_tos_ls", "arguments": {"execute": True, "describe": True}})
            assert not result.get("isError"), result
            execution = json.loads(result["content"][0]["text"])
            assert execution["exit_code"] == 0, execution
            assert ("ve-tos" in execution["argv"]) == (binary.name != "ve-tos-cli")
            assert json.loads(execution["stdout"])["data"]["command"].endswith(" ls")
        finally:
            rpc.close()
        assert (root / "credentials").read_text() == "schema_version=1\n"
        print(f"PASS: {binary.name}; MCP executes ls Describe with the correct subprocess surface")


if __name__ == "__main__":
    assert len(sys.argv) > 1, "pass unified and direct ve-tos binary paths"
    for argument in sys.argv[1:]:
        check(Path(argument).resolve())
