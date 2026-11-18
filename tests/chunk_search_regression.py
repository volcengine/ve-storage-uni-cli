"""Space Chunk CLI contracts through real binaries; isolated localhost fixtures only."""
import datetime
import http.server
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
from urllib.parse import parse_qs

from share_discovery_regression import Rpc, payload


HIT = {"SpaceId": "space", "FilePath": "handbook/中 #.txt", "ETag": '"version-1"',
       "ChunkOrdinal": 0, "ChunkCount": 2, "ChunkTruncated": True,
       "TextContent": "出差住宿标准", "FutureField": {"preserve": True}}


class Service(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_arguments):
        pass

    def do_POST(self):
        raw = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        self.server.events.append((self.path, dict(self.headers), raw))
        if self.path == "/v1/oauth/token":
            assert parse_qs(raw.decode())["grant_type"] == ["refresh_token"]
            body, status = {"access_token": "fixture-new-access", "refresh_token": "fixture-new-refresh",
                "expires_in": 3600, "token_type": "Bearer", "scope": "file:read", "instance_id": self.server.refresh_instance,
                "user_id": "fixture-user"}, 200
        else:
            assert self.path in ("/v2/file/search_chunk", "/v2/file/read_chunk")
            assert self.headers["Authorization"] in ("Bearer fixture-access", "Bearer fixture-new-access")
            assert not any(name.lower().startswith("x-tos-") for name in self.headers)
            body, status = {"Items": [HIT]}, 200
            if self.server.mode == "empty":
                body = {"Items": []}
            elif self.server.mode in ("403", "404", "409", "503", "409-instance", "always401"):
                status = {"always401": 401, "409-instance": 409}.get(self.server.mode)
                status = int(self.server.mode) if status is None else status
                body = {"Code": {403: "AccessDenied", 404: "NotFound", 409: "Conflict.Space", 401: "Unauthorized", 503: "ServiceUnavailable"}[status],
                        "Message": "fixture rejection", "RequestId": "fixture-request"}
                if self.server.mode == "409-instance":
                    body["Code"] = "Conflict.Instance"
            elif self.server.mode == "malformed":
                body = {"Code": "UnexpectedSuccessBody"}
            elif self.server.mode == "refresh" and self.headers["Authorization"] == "Bearer fixture-access":
                body, status = {"Code": "Unauthorized", "Message": "expired"}, 401
        encoded = json.dumps(body, ensure_ascii=False).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(encoded)))
        self.send_header("x-ids-request-id", "fixture-request")
        self.end_headers()
        self.wfile.write(encoded)


def seed(root, server, instance="inst"):
    origin = f"http://127.0.0.1:{server.server_port}"
    expiry = (datetime.datetime.now(datetime.timezone.utc) + datetime.timedelta(hours=1)).isoformat()
    (root / "config").write_text(f"[selected.adrive]\nauth_mode='oauth'\nendpoint='{origin}'\n"
                               "region='cn-beijing'\nmax_retry_count=0\n")
    (root / "credentials").write_text("schema_version=1\n[selected.adrive.oauth]\n"
        "access_token='fixture-access'\nrefresh_token='fixture-refresh'\n"
        f"expires_at='{expiry}'\ninstance_id='{instance}'\nuser_id='fixture-user'\nauth_endpoint='{origin}'\n")
    server.mode, server.refresh_instance = "hits", "inst"
    server.events.clear()


def invoke(binary, root, args, *, stdin=None):
    prefix = [] if binary.name == "ve-adrive-cli" else ["ve-adrive"]
    env = {key: value for key, value in os.environ.items() if not key.startswith(("ADRIVE_", "TOS_"))}
    result = subprocess.run([str(binary), *prefix, "--config-path", str(root / "config"),
        "--credentials-path", str(root / "credentials"), "--profile", "selected", "--output", "json", *args],
        env=env, input=stdin, text=True, capture_output=True, timeout=30)
    for secret in ("fixture-access", "fixture-new-access", "fixture-refresh", "fixture-new-refresh"):
        assert secret not in result.stdout + result.stderr, "token leaked"
    return result


def success(result):
    assert result.returncode == 0, result.stderr or result.stdout
    return json.loads(result.stdout)["data"]


SEARCH = ["chunk", "search", "adrive://inst/space/handbook/", "--text", " 出差 ", "--limit", "5"]
READ = ["chunk", "read", "--instance", "inst", "--space", "space", "--file", HIT["FilePath"],
        "--etag", HIT["ETag"], "--ordinals", "0,1"]


def check_requests(binary, root, server):
    seed(root, server)
    data = success(invoke(binary, root, SEARCH + ["--no-recursive"]))
    assert data["Items"] == [HIT]
    assert json.loads(server.events[-1][2]) == {"SpaceIds": ["space"], "FolderPath": "handbook/",
        "Recursive": False, "SearchQuery": [{"Text": "出差"}], "Limit": 5}
    success(invoke(binary, root, READ))
    assert json.loads(server.events[-1][2]) == {"SpaceId": "space", "FilePath": HIT["FilePath"],
        "ETag": HIT["ETag"], "ChunkOrdinals": [0, 1]}
    success(invoke(binary, root, READ[:-2] + ["--range", "0-1"]))
    request = json.loads(server.events[-1][2])
    assert request["ChunkRange"] == {"Start": 0, "End": 1} and "ChunkOrdinals" not in request
    result = invoke(binary, root, SEARCH + ["--query", "data.Items[0].FilePath"])
    assert result.returncode == 0 and json.loads(result.stdout) == HIT["FilePath"], result.stdout
    config = root / "config"
    config.write_text(config.read_text().replace("max_retry_count=0", "max_retry_count=2"))
    for mode in ("empty", "403", "404", "409", "409-instance", "503", "malformed"):
        server.mode = mode
        before = len(server.events)
        result = invoke(binary, root, READ if mode == "404" else SEARCH)
        assert len(server.events) == before + (3 if mode in ("503", "malformed") else 1), (mode, len(server.events) - before)
        if mode == "empty":
            assert success(result)["Items"] == []
        elif mode == "malformed":
            assert result.returncode != 0
        else:
            assert result.returncode != 0 and "fixture-request" in result.stdout + result.stderr
            if mode.startswith("409"):
                expected = "Conflict.Instance" if mode.endswith("instance") else "Conflict.Space"
                assert expected in result.stdout + result.stderr


def check_auth(binary, root, server):
    seed(root, server)
    server.mode = "refresh"
    success(invoke(binary, root, SEARCH))
    assert [event[0] for event in server.events] == ["/v2/file/search_chunk",
        "/v1/oauth/token", "/v2/file/search_chunk"]
    seed(root, server)
    server.mode = "always401"
    result = invoke(binary, root, SEARCH)
    assert result.returncode != 0 and len(server.events) == 3
    seed(root, server, instance="other")
    result = invoke(binary, root, SEARCH)
    assert result.returncode != 0 and not server.events, "instance guard ran after network"
    seed(root, server)
    server.mode, server.refresh_instance = "refresh", "other"
    result = invoke(binary, root, SEARCH)
    assert result.returncode != 0 and len(server.events) == 2, "refresh changed instance silently"
    seed(root, server)
    server.refresh_instance = "other"
    credentials = root / "credentials"
    contents = credentials.read_text()
    start = contents.index("expires_at='") + len("expires_at='")
    end = contents.index("'", start)
    credentials.write_text(contents[:start] + "2000-01-01T00:00:00Z" + contents[end:])
    result = invoke(binary, root, SEARCH)
    assert result.returncode != 0 and [event[0] for event in server.events] == ["/v1/oauth/token"], "preemptive refresh changed instance silently"
    seed(root, server)
    for mode in ("aksk", "unified"):
        result = invoke(binary, root, SEARCH + ["--auth-mode", mode])
        assert result.returncode != 0 and "oauth" in result.stderr.lower() and not server.events
    config = root / "config"
    config.write_text(config.read_text().replace("auth_mode='oauth'", "auth_mode='aksk'"))
    (root / "credentials").write_text("invalid credentials must not be read")
    result = invoke(binary, root, SEARCH)
    assert result.returncode != 0 and "require --auth-mode oauth" in result.stderr and not server.events


def check_discovery_layers(binary, root):
    for language, high_title, low_title, utility_title in (
            ("en", "High-Level Commands:", "Low-Level Commands:", "Capabilities / Utilities:"),
            ("zh", "高阶命令:", "低阶命令:", "能力 / 工具:")):
        result = invoke(binary, root, ["--help", "--language", language])
        assert result.returncode == 0, result.stderr
        high = result.stdout.split(high_title, 1)[1].split(low_title, 1)[0]
        low = result.stdout.split(low_title, 1)[1].split(utility_title, 1)[0]
        assert any(line.strip().startswith("share ") for line in high.splitlines())
        assert any(line.strip().startswith("chunk ") for line in low.splitlines())
        assert "share create" not in high and "chunk search" not in low
    root_description = success(invoke(binary, root, ["--describe"]))
    assert root_description["implemented_layers"] == ["high_level", "low_level", "utilities"]
    assert success(invoke(binary, root, ["share", "--describe"]))["layer"] == "high-level"
    full = success(invoke(binary, root, ["capabilities", "--view", "full"]))
    assert "chunk" not in full["high_level_semantics"]
    assert any(group["layer"] == "low_level" and group["command_count"] == 3 for group in full["groups"])
    low = success(invoke(binary, root, ["capabilities", "--view", "full", "--layer", "low_level"]))
    assert {row["command"] for row in low["capabilities"]} == {
        "ve-adrive file set-meta", "ve-adrive chunk search", "ve-adrive chunk read"}
    for action in ([], ["search"], ["read"]):
        assert success(invoke(binary, root, ["chunk", *action, "--describe"]))["layer"] == "low-level"
    checks = success(invoke(binary, root, ["doctor", "--check", "principles"]))["checks"]
    assert checks[0]["status"] == "passed", checks


def check_offline(binary, root, server):
    seed(root, server)
    (root / "credentials").unlink()
    plan = success(invoke(binary, root, SEARCH + ["--dry-run"]))
    assert plan["path"] == "/v2/file/search_chunk" and plan["network_requests"] == 0
    assert plan["instance_id"] == "inst" and plan["endpoint_routing"] == "instance_subdomain"
    assert plan["body"]["Recursive"] is True
    before = (root / "config").read_bytes()
    (root / "config").write_text("invalid config must not be read during offline planning")
    success(invoke(binary, root, SEARCH + ["--dry-run"]))
    for args in (["chunk", "--describe"], ["chunk", "search", "--describe"], ["chunk", "read", "--describe"]):
        assert success(invoke(binary, root, args))["command"].startswith("ve-adrive chunk")
    (root / "config").write_bytes(before)
    check_discovery_layers(binary, root)
    for language in ("en", "zh"):
        for args in (["--help"], ["chunk", "--help"], ["chunk", "search", "--help"], ["chunk", "read", "--help"]):
            result = invoke(binary, root, [*args, "--help-language", language])
            assert result.returncode == 0 and result.stdout.strip(), result.stderr
    assert invoke(binary, root, ["chunk"]).returncode == 0
    for args in (SEARCH[:-2] + ["--limit", "101"], SEARCH + ["--folder", "elsewhere/"],
                 SEARCH[:4] + ["中" * 34], READ[:-2], READ[:-1] + ["0,0"],
                 READ[:-2] + ["--range", "0-20"], READ[:-1] + [str(2**63)],
                 READ + ["--range", "0-0"]):
        result = invoke(binary, root, [*args, "--dry-run"])
        assert result.returncode != 0, args
    capabilities = success(invoke(binary, root, ["capabilities"]))
    assert "ve-adrive chunk search" in json.dumps(capabilities)
    skills = success(invoke(binary, root, ["skill", "list"]))
    assert "ve_adrive_chunk_search" in json.dumps(skills)
    assert all(skill["domain"] == "adrive-chunk" for skill in skills["skills"]
               if skill["name"].startswith("ve_adrive_chunk_"))
    for language in ("en", "zh"):
        directory = root / f"skills-{language}"
        success(invoke(binary, root, ["skill", "export", "--name", "ve_adrive_chunk_search",
                                     "--dir", str(directory), "--language", language]))
        markdown = next(directory.glob("*/ve_adrive_chunk_search/SKILL.md")).read_text()
        assert "--text" in markdown and "file:read" in markdown and "ChunkTruncated" in markdown
        assert (directory / "adrive-chunk" / "ve_adrive_chunk_search" / "SKILL.md").exists()
    assert not server.events and not (root / "credentials").exists()


def check_mcp(binary, root, server):
    seed(root, server)
    rpc = Rpc(binary, root)
    try:
        tools = {tool["name"]: tool for tool in rpc.call(2, "tools/list", {})["tools"]}
        search, read = tools["ve_adrive_chunk_search"], tools["ve_adrive_chunk_read"]
        assert all(tool["description"].startswith("Low-level document Chunk API.") for tool in (search, read))
        assert search["annotations"]["readOnlyHint"] is True
        assert read["inputSchema"]["properties"]["ordinals"]["items"]["type"] == "integer"
        described = payload(rpc.call(99, "tools/call", {"name": search["name"],
            "arguments": {"describe": True, "execute": True}}))
        assert described["exit_code"] == 0 and json.loads(described["stdout"])["data"]["layer"] == "low-level"
        assert not server.events
        arguments = {"target": "adrive://inst/space/handbook/", "text": "出差", "profile": "selected"}
        planned = payload(rpc.call(3, "tools/call", {"name": search["name"], "arguments": arguments}))
        assert planned["execution_status"] == "planned_not_executed" and not server.events
        for identifier, bad in enumerate(({"text": " "}, {"text": "中" * 34}, {"auth_mode": "aksk"},
                                           {"target": "adrive://inst"}), 10):
            result = rpc.call(identifier, "tools/call", {"name": search["name"], "arguments": {**arguments, **bad}})
            assert result.get("isError"), result
        response = payload(rpc.call(4, "tools/call", {"name": search["name"],
            "arguments": {**arguments, "execute": True}}))
        assert response["exit_code"] == 0 and json.loads(response["stdout"])["data"]["Items"] == [HIT]
        response = payload(rpc.call(5, "tools/call", {"name": read["name"], "arguments": {
            "instance": "inst", "space": "space", "file": HIT["FilePath"], "etag": HIT["ETag"],
            "ordinals": [0, 1], "profile": "selected", "execute": True}}))
        assert response["exit_code"] == 0 and len(server.events) == 2
        assert json.loads(server.events[-1][2])["ChunkOrdinals"] == [0, 1]
    finally:
        rpc.close()


def main():
    binaries = [Path(path).resolve() for path in sys.argv[1:]]
    assert binaries, "pass unified and standalone binary paths"
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Service)
    server.events = []
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        for binary in binaries:
            with tempfile.TemporaryDirectory(prefix="chunk-cli-") as directory:
                root = Path(directory)
                check_requests(binary, root, server)
                check_auth(binary, root, server)
                check_offline(binary, root, server)
                check_mcp(binary, root, server)
        print(f"PASS: {len(binaries)} entrypoints; Chunk request/read, OAuth, errors, output, discovery, Skill, MCP and offline contracts")
    finally:
        server.shutdown()
        server.server_close()


if __name__ == "__main__":
    main()
