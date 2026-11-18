"""Exercise actual ADrive binaries against an isolated IDS protocol fixture.

Usage: python3 tests/adrive_usermeta_regression.py <unified-binary> [direct-binary]
No cloud credentials or production resources are used.
"""
import http.server
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
from types import SimpleNamespace
from urllib.parse import parse_qs, quote, unquote, urlsplit
from directory_upload_regression import Mcp


class Service(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def read(self):
        return self.rfile.read(int(self.headers.get("Content-Length", "0")))

    def respond(self, value=None, status=200, headers=None, raw=None):
        body = raw if raw is not None else json.dumps(value or {}).encode()
        self.send_response(status)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Content-Type", "application/json")
        self.send_header("x-ids-request-id", "usermeta-fixture")
        for name, content in (headers or {}).items():
            self.send_header(name, content)
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    def target(self):
        path = unquote(urlsplit(self.path).path)
        return path.split("/files/", 1)[-1]

    def metadata(self):
        return {name.lower().removeprefix("x-ids-meta-"): unquote(value)
                for name, value in self.headers.items()
                if name.lower().startswith("x-ids-meta-")}

    def do_HEAD(self):
        key = self.target()
        self.server.events.append(("HEAD", key))
        if key not in self.server.files and key.rstrip("/") + "/" in self.server.files:
            key = key.rstrip("/") + "/"
        if key not in self.server.files:
            return self.respond(status=404)
        data, meta = self.server.files[key]
        headers = {"x-ids-meta-" + key: quote(value, safe="") for key, value in meta.items()}
        headers["x-ids-file-is-folder"] = str(key.endswith("/")).lower()
        headers["x-ids-file-type"] = "folder" if key.endswith("/") else "file"
        self.respond(headers=headers, raw=data)

    def do_GET(self):
        self.server.events.append(("GET", self.target()))
        if urlsplit(self.path).path.endswith("/files"):
            query = parse_qs(urlsplit(self.path).query)
            prefix = query.get("prefix", [""])[0]
            files = [{"FilePath": key, "Size": len(data), "FileType": "folder" if key.endswith("/") else "file"}
                     for key, (data, _meta) in self.server.files.items() if key.startswith(prefix)]
            folders = [{"Folder": item["FilePath"]} for item in files if item["FileType"] == "folder"]
            return self.respond({"Files": files, "Folders": folders, "IsTruncated": False})
        if unquote(urlsplit(self.path).path) == "/v1/instances:getByName":
            return self.respond({"Instance": {"InstanceID": "inst", "Name": "named"}})
        if unquote(urlsplit(self.path).path) == "/v1/spaces:getByName":
            return self.respond({"Space": {"InstanceID": "inst", "SpaceID": "space", "Name": "named-space"}})
        if self.path.startswith("/v1/instances?"):
            return self.respond({"Instances": [{"InstanceID": "inst", "Name": "named"}]})
        data, meta = self.server.files[self.target()]
        self.respond(raw=data, headers={"x-ids-meta-" + key: quote(value, safe="")
                                       for key, value in meta.items()})

    def do_POST(self):
        body, key = self.read(), self.target()
        self.server.events.append(("POST", key))
        if self.path.endswith("/folders"):
            folder = json.loads(body)["FolderPath"]
            self.server.files[folder.rstrip("/") + "/"] = (b"", {})
            return self.respond({"FolderPath": folder})
        if key.endswith(":initiateMultipart"):
            key = key.removesuffix(":initiateMultipart")
            metadata = json.loads(body).get("Meta", {})
            upload = str(len(self.server.uploads) + 1)
            self.server.uploads[upload] = (key, metadata, {})
            return self.respond({"UploadId": upload})
        if key.endswith(":completeMultipart"):
            request = json.loads(body)
            key, meta, parts = self.server.uploads[request["UploadId"]]
            data = b"".join(parts[number] for number in sorted(parts))
        else:
            data, meta = body, self.metadata()
            self.server.wire_headers.append(dict(self.headers))
        self.server.files[key] = (data, meta)
        self.respond({"FilePath": key, "Size": len(data)})

    def do_PUT(self):
        body, key = self.read(), self.target()
        self.server.events.append(("PUT", key))
        if key.endswith(":uploadPart"):
            query = parse_qs(urlsplit(self.path).query)
            upload = query["uploadId"][0]
            number = int(query["partNumber"][0])
            self.server.parts.append((upload, number))
            if self.server.block_second and number == 2:
                self.server.second_seen.set()
                self.server.release_second.wait(20)
                return
            self.server.uploads[upload][2][number] = body
            return self.respond({"PartNumber": number, "ETag": "part-etag"})
        key = key.removesuffix(":meta")
        if key == "denied":
            return self.respond({"Code": "AccessDenied", "Message": "denied"}, 403)
        if key not in self.server.files:
            return self.respond({"Code": "NotFound.File", "Message": "missing"}, 404)
        patch = json.loads(body)
        assert set(patch) == {"Meta"}, patch
        if len(json.dumps(patch["Meta"])) > 8192:
            return self.respond({"Code": "InvalidParameter.MetadataTooLarge", "Message": "too large"}, 400)
        data, meta = self.server.files[key]
        meta.update(patch["Meta"])
        self.server.files[key] = (data, meta)
        self.respond({"InstanceID": "inst", "SpaceID": "space", "FilePath": key, "UpdatedAt": 1})


def run(binary):
    with tempfile.TemporaryDirectory(prefix="adrive-usermeta-") as directory:
        root = Path(directory)
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Service)
        server.files, server.events, server.uploads, server.wire_headers = {}, [], {}, []
        server.parts, server.block_second = [], False
        server.second_seen, server.release_second = threading.Event(), threading.Event()
        worker = threading.Thread(target=server.serve_forever, daemon=True)
        worker.start()
        endpoint = f"http://127.0.0.1:{server.server_port}"
        (root / "config").write_text(
            f"[default.adrive]\nendpoint='{endpoint}'\nregion='test-region'\n"
            "access_key_id='fixture-ak'\nsecret_access_key='fixture-sk'\nmax_retry_count=0\n"
            f"batch_report_dir='{root / 'reports'}'\ncheckpoint_dir='{root / 'checkpoints'}'\n")
        (root / "credentials").write_text("schema_version=1\n")
        env = {k: v for k, v in os.environ.items()
               if not k.startswith(("ADRIVE_", "TOS_", "BYTETOS_", "BYTE_TOS_"))}
        prefix = ["ve-adrive"] if binary.name == "ve-storage-uni-cli" else []

        def invoke(*args, stdin=None, ok=True, allow_empty=False):
            result = subprocess.run(
                [str(binary), *prefix, "--config-path", str(root / "config"),
                 "--credentials-path", str(root / "credentials"), "--output", "json",
                 "--quiet", *args], input=stdin, capture_output=True, env=env, timeout=40)
            assert (result.returncode == 0) == ok, (args, result.stdout, result.stderr)
            if allow_empty and not result.stdout:
                return None
            payload = result.stdout or result.stderr
            if not ok and not payload.lstrip().startswith(b"{"):
                return {"parser_error": payload.decode()}
            return json.loads(payload)

        def uri(name):
            return "adrive://inst/space/" + name

        def interrupted_upload(name, metadata, recursive=False, no_manifest=False):
            checkpoint_dir = root / (name + "-state")
            args = ["cp", str(large_tree if recursive else large), uri(name + "/" if recursive else name), "--checkpoint", "--checkpoint-dir",
                    str(checkpoint_dir), "--checkpoint-threshold", "1B",
                    "--multipart-concurrency", "1"]
            if recursive:
                args.extend(["--recursive", "--batch-concurrency", "1", "--overwrite-strategy", "force"])
            if no_manifest:
                args.append("--no-manifest")
            if metadata is not None:
                args.extend(["--meta", metadata])
            server.second_seen.clear()
            server.release_second.clear()
            server.block_second = True
            process = subprocess.Popen(
                [str(binary), *prefix, "--config-path", str(root / "config"),
                 "--credentials-path", str(root / "credentials"), "--output", "json",
                 "--quiet", *args], stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
            try:
                assert server.second_seen.wait(15), "second part never reached fixture"
                state_path, = checkpoint_dir.glob("cp-file-*.json" if recursive else "cp-*.json")
                state = json.loads(state_path.read_text())
                assert [part["part_number"] for part in state["completed_parts"]] == [1], state
                assert name not in server.files
            finally:
                process.kill()
                process.communicate(timeout=5)
                server.block_second = False
                server.release_second.set()
            return args, state_path, state

        try:
            value = "中文 + %20 # =\nnext"
            meta = json.dumps({"Owner": "xsh", "Note": value, "empty": "", "x-ids-meta-user": "literal"})
            expected = {"owner": "xsh", "note": value, "empty": "", "x-ids-meta-user": "literal"}
            source = root / "file.txt"
            source.write_bytes(b"original-content")
            invoke("cp", str(source), uri("a.txt"), "--meta", meta)
            assert server.files["a.txt"] == (source.read_bytes(), expected)
            header = {k.lower(): v for k, v in server.wire_headers[-1].items()}
            assert unquote(header["x-ids-meta-note"]) == value
            assert "%2520" in header["x-ids-meta-note"], header
            assert invoke("stat", uri("a.txt"))["data"]["meta"] == expected
            download = root / "download.txt"
            invoke("cp", uri("a.txt"), str(download))
            assert download.read_bytes() == source.read_bytes()
            invoke("cp", str(source), uri("a.txt"), "--no-clobber", "--meta", "owner=not-applied", allow_empty=True)
            assert server.files["a.txt"][1] == expected

            invoke("file", "set-meta", uri("a.txt"), "--meta", "owner=updated#new=yes#empty=")
            data, updated = server.files["a.txt"]
            assert data == source.read_bytes()
            assert updated == {**expected, "owner": "updated", "new": "yes"}
            assert invoke("stat", uri("a.txt"))["data"]["meta"] == updated
            invoke("file", "set-meta", uri("a.txt"), "--usermeta", json.dumps({"note": value}))
            assert server.files["a.txt"][1]["note"] == value
            invoke("file", "set-meta", "adrive://named/named-space/a.txt", "--by-name", "--meta", "named=yes")
            assert server.files["a.txt"][1]["named"] == "yes"
            for flag in ["--meta", "--usermeta", "--metadata"]:
                invoke("cp", str(source), uri("alias.txt"), flag, "url=https://example.test/?a=1&b=2")
                invoke("put", uri("alias-stdin.txt"), flag, "owner=xsh", stdin=b"alias")
                invoke("file", "set-meta", uri("alias.txt"), flag, "owner=xsh")
                assert server.files["alias.txt"][1] == {"owner": "xsh", "url": "https://example.test/?a=1&b=2"}
                assert server.files["alias-stdin.txt"] == (b"alias", {"owner": "xsh"})

            for multipart in [False, True]:
                name = f"stdin-{multipart}.txt"
                extra = ["--multipart-threshold", "1B"] if multipart else []
                invoke("put", uri(name), "--meta", meta, *extra, stdin=b"stdin-content")
                assert server.files[name] == (b"stdin-content", expected)
            invoke("cp", str(source), uri("multipart.txt"), "--checkpoint",
                   "--checkpoint-dir", str(root / "cp-state"), "--meta", meta)
            assert server.files["multipart.txt"] == (source.read_bytes(), expected)

            large = root / "large.bin"
            large.write_bytes(b"x" * (16 * 1024 * 1024) + b"last-part")
            large_tree = root / "large-tree"
            large_tree.mkdir()
            (large_tree / "large.bin").write_bytes(large.read_bytes())
            args, state_path, state = interrupted_upload("resume.bin", "B=2#a=1")
            upload = state["upload_id"]
            assert state["meta"] == {"a": "1", "b": "2"}
            args[-1] = "a=1#b=2"
            count = len(server.uploads)
            invoke(*args)
            assert len(server.uploads) == count
            assert server.parts.count((upload, 1)) == 1
            assert server.parts.count((upload, 2)) == 2
            assert not state_path.exists()
            assert server.files["resume.bin"] == (large.read_bytes(), {"a": "1", "b": "2"})

            args, state_path, state = interrupted_upload("changed.bin", "owner=old")
            original = state_path.read_bytes()
            state["meta"] = {"owner": "forged"}
            state_path.write_text(json.dumps(state))
            before = len(server.events)
            result = invoke(*args, ok=False)
            assert "checkpoint metadata does not match" in json.dumps(result), result
            assert not any(method in ["POST", "PUT"] for method, _ in server.events[before:])
            state_path.write_bytes(original)
            args[-1] = "owner=new"
            count = len(server.uploads)
            invoke(*args)
            assert len(server.uploads) == count + 1
            assert state_path.read_bytes() == original
            assert server.parts.count((state["upload_id"], 1)) == 1
            assert server.parts.count((state["upload_id"], 2)) == 1
            assert server.files["changed.bin"] == (large.read_bytes(), {"owner": "new"})

            args, state_path, state = interrupted_upload("legacy.bin", None)
            state.pop("meta")
            state_path.write_text(json.dumps(state))
            count = len(server.uploads)
            invoke(*args)
            assert len(server.uploads) == count
            assert server.parts.count((state["upload_id"], 1)) == 1
            assert server.files["legacy.bin"] == (large.read_bytes(), {})

            for no_manifest in [False, True]:
                for original_meta, next_meta in [("B=2#a=1", "a=1#b=2"),
                                                 ("owner=old", "owner=new"),
                                                 (None, "owner=added"), (None, None)]:
                    name = f"recursive-{no_manifest}-{len(server.uploads)}"
                    args, state_path, state = interrupted_upload(name, original_meta, True, no_manifest)
                    if original_meta is None and next_meta is None:
                        state.pop("meta")
                        state_path.write_text(json.dumps(state))
                    original = state_path.read_bytes()
                    next_args = args.copy()
                    if original_meta is not None:
                        next_args[-1] = next_meta
                    elif next_meta is not None:
                        next_args.extend(["--meta", next_meta])
                    same = next_meta in ["a=1#b=2", None]
                    count = len(server.uploads)
                    invoke(*next_args)
                    assert len(server.uploads) == count + (0 if same else 1)
                    upload = state["upload_id"]
                    assert server.parts.count((upload, 1)) == 1
                    assert server.parts.count((upload, 2)) == (2 if same else 1)
                    wanted = dict(item.split("=", 1) for item in next_meta.split("#")) if next_meta else {}
                    assert server.files[name + "/large.bin"] == (large.read_bytes(), wanted)
                    if same:
                        assert not state_path.exists()
                    else:
                        assert state_path.read_bytes() == original
                        invoke(*args)
                        assert len(server.uploads) == count + 1
                        assert server.parts.count((upload, 1)) == 1
                        assert server.parts.count((upload, 2)) == 2
                        assert not state_path.exists()

            tree = root / "tree"
            tree.mkdir()
            (tree / "one.txt").write_bytes(b"one")
            (tree / "empty").mkdir()
            for mode in [[], ["--no-manifest"], ["--checkpoint", "--checkpoint-threshold", "1B"]]:
                target = f"tree-{len(server.events)}/"
                invoke("cp", str(tree), uri(target), "--recursive",
                       "--overwrite-strategy", "force", "--meta", meta, *mode)
                assert server.files[target + "one.txt"] == (b"one", expected)
                assert server.files[target + "empty/"][1] == {}

            before = len(server.events)
            for command in [
                ["file", "set-meta", uri("a.txt")],
                ["file", "set-meta", "adrive://inst/space/", "--meta", "a=b"],
                ["file", "set-meta", uri("a.txt"), "--meta", "IDS-key=value"],
                ["file", "set-meta", uri("a.txt"), "--meta", "A=1#a=2"],
                ["put", uri("unused"), "--meta", "bad.key=value"],
                ["cp", uri("a.txt"), str(root / "out"), "--meta", "a=b"],
                ["cp", uri("a.txt"), uri("copy"), "--meta", "a=b"],
            ]:
                invoke(*command, ok=False)
            assert len(server.events) == before, server.events[before:]
            for command in [
                ["file", "set-meta", uri("a.txt"), "--meta", meta],
                ["cp", str(source), uri("planned"), "--meta", meta],
                ["put", uri("planned"), "--meta", meta],
            ]:
                result = invoke(*command, "--dry-run")
                assert result["data"]["execution_status"] == "planned_not_executed"
            assert len(server.events) == before, server.events[before:]
            for command, is_set_meta in [(["file", "set-meta"], True), (["cp"], False), (["put"], False)]:
                result = invoke(*command, "--describe")
                parameters = {p["name"] for p in result["data"]["parameters"]}
                assert "meta" in parameters, result
                if is_set_meta:
                    assert result["data"]["command"] == "ve-adrive file set-meta", result
                    assert parameters == {"path", "by-name", "meta"}, result
                    routing = result["data"]["scenario_routing"]
                    assert "--by-name" in routing["target_resolution"]
                    assert "adrive://instance/space/file" in routing["target_resolution"]
                    assert not {"progress", "list_echo", "checkpoint"} & routing.keys(), routing
                    for unsupported in ["--instance", "--space", "--folder", "--file",
                                        "--progress", "--no-progress", "--list-echo",
                                        "--no-list-echo", "--checkpoint"]:
                        assert unsupported not in json.dumps(routing), routing
                        rejected = invoke("file", "set-meta", uri("a.txt"), "--meta", "a=b", unsupported, ok=False)
                        assert "unexpected argument" in json.dumps(rejected), rejected
                    invoke("file", "set-meta", uri("a.txt"), "--by-name", "--meta", "a=b", "--dry-run")
                for arguments in [["--auth-mode", "aksk", *command, "--describe"],
                                  [*command, "--auth-mode", "aksk", "--describe"],
                                  ["--auth-mode=aksk", *command, "--describe"]]:
                    described = invoke(*arguments)["data"]
                    assert described["command"] == result["data"]["command"], described
                    assert described["parameters"] == result["data"]["parameters"], described
            assert len(server.events) == before
            for name in ["missing", "denied"]:
                invoke("file", "set-meta", uri(name), "--meta", "a=b", ok=False)
            result = invoke("file", "set-meta", uri("a.txt"), "--meta", "note=" + "a" * 8193, ok=False)
            assert "MetadataTooLarge" in json.dumps(result), result
            rpc = Mcp(SimpleNamespace(binary=binary, surface="ve-adrive", root=root))
            try:
                rpc.call(1, "initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                                         "clientInfo": {"name": "usermeta-regression", "version": "1"}})
                rpc.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
                tools = rpc.call(2, "tools/list", {})["tools"]
                for suffix in ["_stat", "_share_view"]:
                    tool = next(tool for tool in tools if tool["name"].endswith(suffix))
                    assert tool["annotations"]["readOnlyHint"] is True, tool
                for suffix in ["_ls", "_du", "_find", "_set_meta", "_cp", "_put", "_crt", "_mkdir", "_config", "_skill",
                               "_auth", "_share_create", "_share_update", "_share_files", "_share_download"]:
                    tool = next(tool for tool in tools if tool["name"].endswith(suffix))
                    assert tool["annotations"]["readOnlyHint"] is False, tool
                    assert tool["annotations"]["destructiveHint"] is False, tool
                tool = next(tool for tool in tools if tool["name"].endswith("_rm"))
                assert tool["annotations"]["readOnlyHint"] is False, tool
                assert tool["annotations"]["destructiveHint"] is True, tool
                set_meta_tool = next(tool for tool in tools if tool["name"].endswith("_file_set_meta"))
                assert set_meta_tool["inputSchema"]["required"] == ["path", "meta"], set_meta_tool
                for number, invalid in enumerate([
                    {"path": uri("a.txt")},
                    {"path": uri("a.txt"), "meta": "IDS-key=value"},
                    {"path": uri("dir/"), "meta": "a=b"},
                    {"path": uri("a.txt"), "meta": "bad.key=value"},
                ], 900):
                    rejected = rpc.call(number, "tools/call",
                                        {"name": set_meta_tool["name"], "arguments": invalid})
                    assert rejected.get("isError"), (invalid, rejected)
                planned = rpc.call(950, "tools/call",
                                   {"name": set_meta_tool["name"],
                                    "arguments": {"path": uri("a.txt"), "meta": "plan=yes"}})
                assert not planned.get("isError"), planned
                assert json.loads(planned["content"][0]["text"])["execution_status"] == "planned_not_executed"
                described = rpc.call(951, "tools/call",
                                     {"name": set_meta_tool["name"],
                                      "arguments": {"describe": True, "execute": True}})
                assert not described.get("isError"), described
                for number, (suffix, arguments) in enumerate([
                    ("_set_meta", {"path": uri("a.txt"), "meta": "mcp=yes", "execute": True}),
                    ("_cp", {"source": str(source), "destination": uri("mcp.txt"), "meta": meta, "execute": True}),
                    ("_put", {"path": uri("mcp-put.txt"), "meta": meta, "execute": True}),
                ], 3):
                    tool = next(tool for tool in tools if tool["name"].endswith(suffix))
                    assert tool["inputSchema"]["properties"]["meta"]["type"] == "string"
                    response = rpc.call(number, "tools/call", {"name": tool["name"], "arguments": arguments})
                    assert not response.get("isError"), response
                    execution = json.loads(response["content"][0]["text"])
                    assert execution["exit_code"] == 0, execution
                assert server.files["a.txt"][1]["mcp"] == "yes"
                assert server.files["mcp.txt"] == (source.read_bytes(), expected)
                assert server.files["mcp-put.txt"] == (b"", expected)
                for number, suffix in enumerate(["_ls", "_du", "_find"], 10):
                    tool = next(tool for tool in tools if tool["name"].endswith(suffix))
                    arguments = {"path": uri(""), "execute": True}
                    manifest = root / (suffix + ".csv")
                    arguments["manifest-path"] = str(manifest)
                    response = rpc.call(number, "tools/call", {"name": tool["name"], "arguments": arguments})
                    assert not response.get("isError"), response
                    execution = json.loads(response["content"][0]["text"])
                    assert execution["exit_code"] == 0, execution
                    assert "a.txt" in manifest.read_text(), manifest.read_text()
            finally:
                rpc.close()
            print(f"PASS {binary.name}: metadata aliases, upload, stdin, multipart interruption/resume/identity/legacy, recursive, stat, incremental update, Describe/Clap, MCP annotations/execution, offline, validation and server errors")
        finally:
            server.release_second.set()
            server.shutdown()
            server.server_close()
            worker.join()


if __name__ == "__main__":
    if len(sys.argv) < 2:
        raise SystemExit("usage: adrive_usermeta_regression.py <binary> [binary ...]")
    for path in sys.argv[1:]:
        run(Path(path).resolve())
