"""MCP discovery and executed download planning through real entrypoints, localhost only."""
import http.server
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading

from share_dry_run_regression import PlanFixture


class Rpc:
    def __init__(self, binary, root):
        prefix = [] if binary.stem == "ve-adrive-cli" else ["ve-adrive"]
        environment = {key: value for key, value in os.environ.items() if not key.startswith("ADRIVE_")}
        # Local transfer profiles are readable during execution; credentials remain absent.
        config = root / "config"
        if not config.exists():
            config.write_text("")
        self.process = subprocess.Popen([str(binary), *prefix, "serve", "--mcp", "--transport", "stdio",
            "--config-path", str(root / "config"), "--credentials-path", str(root / "credentials")],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=environment)
        self.messages = queue.Queue()
        self.reader = threading.Thread(target=self.read, daemon=True)
        self.reader.start()
        try:
            self.call(1, "initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                                       "clientInfo": {"name": "share-regression", "version": "1"}})
            self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        except BaseException:
            self.close()
            raise

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


def payload(result):
    assert not result.get("isError"), result
    return json.loads(result["content"][0]["text"])


def check_rpc(binary, root, origin):
    rpc = Rpc(binary, root)
    try:
        tool = next(item for item in rpc.call(2, "tools/list", {})["tools"]
                    if item["name"] == "ve_adrive_share_download")
        assert tool["annotations"]["readOnlyHint"] is False
        assert tool["annotations"]["destructiveHint"] is False
        assert "metadata" in tool["description"] and "download quota" in tool["description"]
        arguments = {"share-url": f"{origin}/share/s", "destination-dir": str(root / "planned"),
                     "path": "tree/", "recursive": True}
        before = len(PlanFixture.requests)
        check_download_controls(rpc, tool, arguments)
        planned = payload(rpc.call(3, "tools/call", {"name": tool["name"], "arguments": arguments}))
        assert planned["execution_status"] == "planned_not_executed" and len(PlanFixture.requests) == before
        result = payload(rpc.call(4, "tools/call", {"name": tool["name"],
                         "arguments": {**arguments, "execute": True, "dry_run": True}}))
        assert result["exit_code"] == 0, result
        selection = json.loads(result["stdout"])["data"]["selection"]
        assert selection["FileCount"] == 3 and selection["TotalBytes"] == 9
        assert selection["LocalWrites"] is False and not (root / "planned").exists()
        assert not any("download_url" in path or path == "/content" for path in PlanFixture.requests[before:])
        assert (root / "config").read_text() == "" and not (root / "credentials").exists()
        check_files_rpc(rpc, root, origin)
    finally:
        rpc.close()


def check_download_controls(rpc, tool, arguments):
    """Validate discovery types and offline argv without issuing receiver requests."""
    properties = tool["inputSchema"]["properties"]
    assert properties["checkpoint"]["oneOf"] == [
        {"type": "boolean"}, {"type": "string", "minLength": 1}]
    for name, expected in [("batch-concurrency", 16), ("list-concurrency", 4),
                           ("multipart-concurrency", 4)]:
        assert properties[name]["minimum"] == 1 and properties[name]["default"] == expected
    assert properties["overwrite-strategy"]["enum"] == ["force", "no-clobber", "newer"]
    assert properties["progress-granularity"]["enum"] == ["part", "byte"]
    for name in ["include-parent", "force", "no-clobber", "report-failures-only",
                 "no-manifest", "list-echo", "no-list-echo"]:
        assert properties[name]["type"] == "boolean", name
    before = len(PlanFixture.requests)
    for identifier, checkpoint in enumerate([True, False, "./legacy.json"], 30):
        result = payload(rpc.call(identifier, "tools/call", {"name": tool["name"],
                         "arguments": {**arguments, "checkpoint": checkpoint}}))
        argv = result["argv"]
        assert not any(value in {"true", "false"} for value in argv)
        assert ("--checkpoint" in argv) is (checkpoint is not False)
        if isinstance(checkpoint, str):
            position = argv.index("--checkpoint")
            assert argv[position + 1] == checkpoint
    for identifier, invalid in enumerate([{"checkpoint": ""}, {"batch-concurrency": 0},
                                           {"overwrite-strategy": "unknown"},
                                           {"progress-granularity": "unknown"}], 40):
        result = rpc.call(identifier, "tools/call", {"name": tool["name"],
                          "arguments": {**arguments, **invalid}})
        assert result.get("isError"), result
    assert len(PlanFixture.requests) == before


def check_report_profile_resolution(binary, root, origin):
    """Resolve shared/scoped/env CSV settings without credentials or artifact writes."""
    prefix = [] if binary.stem == "ve-adrive-cli" else ["ve-adrive"]
    config = root / "config"
    environment = {key: value for key, value in os.environ.items() if not key.startswith("ADRIVE_")}
    environment.update(ADRIVE_BATCH_REPORT_DIR=str(root / "environment"), ADRIVE_BATCH_REPORT_FORMAT="csv")
    shared = f"[default]\nbatch_report_dir={json.dumps(str(root / 'shared'))}\n"
    scoped = f"[default.adrive]\nbatch_report_dir={json.dumps(str(root / 'scoped'))}\nsecret_access_key='ENC:invalid-unused-secret'\n"
    for contents, expected in [("", "environment"), (shared, "shared"), (shared + scoped, "scoped")]:
        config.write_text(contents)
        before = len(PlanFixture.requests)
        result = subprocess.run([str(binary), *prefix, "share", "download", f"{origin}/share/s",
            str(root / "profile-plan"), "--path", "tree/a.txt", "--dry-run", "--output", "json",
            "--config-path", str(config), "--credentials-path", str(root / "missing-credentials")],
            env=environment, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=15)
        assert result.returncode == 0, result.stdout + result.stderr
        data = json.loads(result.stdout)["data"]
        assert Path(data["report_path"]).parent == root / expected
        assert Path(data["manifest_path"]).parent == root / expected
        assert not (root / expected).exists() and not (root / "profile-plan").exists()
        assert not any("get_download_url" in path or path == "/content" for path in PlanFixture.requests[before:])
        assert not (root / "missing-credentials").exists()


def check_files_rpc(rpc, root, origin):
    tool = next(item for item in rpc.call(5, "tools/list", {})["tools"]
                if item["name"] == "ve_adrive_share_files")
    properties = tool["inputSchema"]["properties"]
    assert tool["annotations"]["readOnlyHint"] is False
    assert "manifest-path" in tool["description"]
    assert "limit" not in properties and properties["max-keys"]["default"] == 1000
    assert properties["human-readable"]["type"] == "boolean"
    assert properties["sort"]["enum"] == ["key", "name", "size", "last-modified", "mtime"]
    assert all(name in properties for name in ["columns", "manifest-path"])
    arguments = {"share-url": f"{origin}/share/s", "path": "tree/", "max-keys": 3,
                 "execute": True}
    result = payload(rpc.call(6, "tools/call", {"name": tool["name"], "arguments": arguments}))
    assert result["exit_code"] == 0, result
    envelope = json.loads(result["stdout"])
    assert envelope["data"]["Selection"]["MaxKeys"] == 3
    assert envelope["pagination"]["total_returned"] == 3
    assert not any(path.name not in {"config", "credentials"} for path in root.iterdir())
    before = len(PlanFixture.requests)
    rejected = rpc.call(7, "tools/call", {"name": tool["name"],
                       "arguments": {"share-url": f"{origin}/share/s", "limit": 1, "execute": True}})
    assert rejected.get("isError") and len(PlanFixture.requests) == before, rejected


def main():
    binaries = [Path(value).resolve() for value in sys.argv[1:]]
    assert binaries, "pass the unified and standalone binary paths"
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), PlanFixture)
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    try:
        for binary in binaries:
            with tempfile.TemporaryDirectory(prefix="share-discovery-") as directory:
                check_rpc(binary, Path(directory), f"http://127.0.0.1:{server.server_port}")
                check_report_profile_resolution(binary, Path(directory), f"http://127.0.0.1:{server.server_port}")
        print(f"PASS: {len(binaries)} entrypoints; MCP annotations, offline argv plan and executed read-only metadata plan")
    finally:
        server.shutdown()
        server.server_close()
        worker.join()


if __name__ == "__main__":
    main()
