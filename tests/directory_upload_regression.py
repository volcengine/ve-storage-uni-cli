"""Actual CLI directory uploads and discovery against isolated TOS/IDS fixtures.

Run with unified and any direct binary paths; no account or cloud access is used.
"""
import http.server
import json
import os
from pathlib import Path
import queue
import shutil
import subprocess
import sys
import tempfile
import threading
from urllib.parse import parse_qs, unquote, urlsplit


def fixture_environment():
    return {key: value for key, value in os.environ.items()
            if not key.startswith(("TOS_", "ADRIVE_", "BYTETOS_", "BYTE_TOS_"))}


class UploadService(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_arguments):
        pass

    def route(self):
        parsed = urlsplit(self.path)
        path = unquote(parsed.path)
        if path.startswith("/v1/"):
            return "adrive", path.split("/files/", 1)[-1], parse_qs(parsed.query)
        return "tos", path.removeprefix("/bucket/").removeprefix("/bucket"), parse_qs(parsed.query)

    def respond(self, value=None, status=200, headers=None):
        encoded = json.dumps(value or {}).encode()
        self.send_response(status)
        self.send_header("Content-Length", str(len(encoded)))
        self.send_header("Content-Type", "application/json")
        self.send_header("x-tos-request-id", "directory-fixture")
        self.send_header("x-ids-request-id", "directory-fixture")
        for key, value in (headers or {}).items():
            self.send_header(key, str(value))
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(encoded)

    def do_HEAD(self):
        service, key, _query = self.route()
        self.server.events.append(("HEAD", key))
        if service == "tos" and not key:
            return self.respond(headers={"x-tos-bucket-type": self.server.bucket_type})
        folder_key = key.rstrip("/") + "/"
        is_folder = folder_key in self.server.objects
        if key not in self.server.objects and not (service == "adrive" and is_folder):
            return self.respond({"Code": "NoSuchKey"}, 404)
        body = self.server.objects.get(key, b"")
        self.send_response(200)
        for name, value in {"Content-Length": len(body), "ETag": '"fixture"',
                            "Last-Modified": "Thu, 01 Jan 2026 00:00:00 GMT",
                            "x-ids-file-type": "folder" if is_folder else "file",
                            "x-ids-is-folder": str(is_folder).lower()}.items():
            self.send_header(name, str(value))
        self.end_headers()

    def do_GET(self):
        service, _key, query = self.route()
        prefix = query.get("prefix", [""])[0]
        delimiter = query.get("delimiter", [""])[0]
        self.server.events.append(("LIST", prefix))
        keys = sorted(key for key in self.server.objects if key.startswith(prefix))
        if service == "adrive":
            files = [{"FilePath": key, "Size": len(self.server.objects[key]),
                      "FileType": "folder" if key.endswith("/") else "file"}
                     for key in keys if not delimiter or "/" not in key[len(prefix):].rstrip("/")]
            folders = [{"Folder": key} for key in keys if key.endswith("/")
                       and (not delimiter or "/" not in key[len(prefix):].rstrip("/"))]
            return self.respond({"Files": files, "Folders": folders, "IsTruncated": False})
        contents, folders = [], set()
        for key in keys:
            suffix = key[len(prefix):]
            if delimiter and "/" in suffix:
                folders.add(prefix + suffix.split("/", 1)[0] + "/")
            else:
                contents.append({"Key": key, "Size": len(self.server.objects[key]), "ETag": '"fixture"'})
        self.respond({"Contents": contents, "CommonPrefixes": [{"Prefix": key} for key in sorted(folders)],
                      "IsTruncated": False, "KeyCount": len(contents) + len(folders)})

    def write_object(self, key, body):
        self.server.events.append((self.command, key))
        if key.endswith("/"):
            assert not body, "directory content must be empty"
            assert self.headers.get("x-tos-storage-class", "STANDARD") == "STANDARD"
            if self.server.fail_directory:
                return self.respond({"Code": "AccessDenied", "Message": "directory denied"}, 403)
            self.server.created.append(key)
        if self.server.bucket_type == "hns" or self.path.startswith("/v1/"):
            segments = key.rstrip("/").split("/")[:-1]
            for index in range(1, len(segments) + 1):
                self.server.objects.setdefault("/".join(segments[:index]) + "/", b"")
        self.server.objects[key] = body
        self.respond({"FilePath": key, "Size": len(body), "ETag": "fixture"})

    def do_PUT(self):
        _service, key, _query = self.route()
        self.write_object(key, self.rfile.read(int(self.headers.get("Content-Length", "0"))))

    def do_POST(self):
        content = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        if self.path.endswith("/folders"):
            key = json.loads(content)["FolderPath"].rstrip("/") + "/"
            return self.write_object(key, b"")
        if "/files/" in self.path:
            return self.write_object(self.route()[1], content)
        self.server.events.append(("DELETE", self.path))
        requested = json.loads(content)
        for item in requested.get("Objects", []):
            self.server.objects.pop(item["Key"], None)
        self.respond({"Deleted": requested.get("Objects", [])})

    def do_DELETE(self):
        _service, key, _query = self.route()
        self.server.events.append(("DELETE", key))
        self.server.objects.pop(key, None)
        self.respond()


class Fixture:
    def __init__(self, binary, surface, root):
        self.binary, self.surface, self.root = binary, surface, root
        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), UploadService)
        self.server.objects, self.server.events, self.server.created = {}, [], []
        self.server.bucket_type, self.server.fail_directory = "fns", False
        self.worker = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.worker.start()
        origin = f"http://127.0.0.1:{self.server.server_port}"
        section = {"tos": "tos", "ve-tos": "ve-tos", "ve-adrive": "adrive"}[surface]
        (root / "config").write_text(f"[default.{section}]\nendpoint='{origin}'\nregion='test-region'\n"
            f"access_key_id='fixture-ak'\nsecret_access_key='fixture-sk'\nmax_retry_count=0\n"
            f"batch_report_dir='{root / 'reports'}'\ncheckpoint_dir='{root / 'checkpoints'}'\n")
        # [Review Fix #DirectoryFixture1] Explicit credential paths must exist, even with fake legacy keys.
        (root / "credentials").write_text("schema_version=1\n")
        self.source = root / "src"
        (self.source / "empty").mkdir(parents=True)
        (self.source / "populated").mkdir()
        (self.source / "populated" / "a.txt").write_bytes(b"abc")

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.worker.join()

    def destination(self, prefix="base/"):
        return ("adrive://inst/space/" if self.surface == "ve-adrive" else "tos://bucket/") + prefix

    def invoke(self, arguments):
        prefix = [self.surface] if self.binary.name == "ve-storage-uni-cli" else []
        result = subprocess.run([str(self.binary), *prefix, "--config-path", str(self.root / "config"),
            "--credentials-path", str(self.root / "credentials"), "--output", "json", "--quiet", *arguments],
            capture_output=True, text=True, env=fixture_environment(), timeout=25)
        return result

    def upload(self, action, flags=(), source=None):
        # [Review Fix #DirectoryFixture3] Sync journals require a CLI override to stay inside the fixture.
        arguments = [action, str(source or self.source), self.destination(),
                     "--checkpoint-dir", str(self.root / "checkpoints"), *flags]
        if action == "cp":
            arguments.append("--recursive")
        result = self.invoke(arguments)
        assert result.returncode == 0, (self.surface, action, flags, result.stdout, result.stderr)
        return json.loads(result.stdout)

    def reset(self):
        # [Review Fix #DirectoryFixture2] Independent cases must not reuse sync's completed-file journal.
        checkpoint_root = self.root / "checkpoints"
        if checkpoint_root.exists():
            shutil.rmtree(checkpoint_root)
        self.server.objects.clear()
        self.server.events.clear()
        self.server.created.clear()


class Mcp:
    def __init__(self, fixture):
        prefix = [fixture.surface] if fixture.binary.name == "ve-storage-uni-cli" else []
        self.process = subprocess.Popen([str(fixture.binary), *prefix, "serve", "--mcp",
            "--transport", "stdio", "--config-path", str(fixture.root / "config"),
            "--credentials-path", str(fixture.root / "credentials")], text=True,
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=fixture_environment())
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
            response = self.messages.get(timeout=15)
            if response.get("id") == identifier:
                assert "error" not in response, response
                return response["result"]

    def close(self):
        self.process.terminate()
        self.process.communicate(timeout=5)
        self.reader.join(timeout=5)


def check_mcp(fixture):
    rpc = Mcp(fixture)
    try:
        rpc.call(1, "initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                                  "clientInfo": {"name": "directory-regression", "version": "1"}})
        rpc.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        tools = rpc.call(2, "tools/list", {})["tools"]
        for identifier, action in enumerate(("cp", "sync"), 3):
            tool = next(value for value in tools if value["name"].endswith("_" + action))
            assert tool["inputSchema"]["properties"]["disable-dir-object"]["type"] == "boolean"
            arguments = {"source": str(fixture.source), "destination": fixture.destination(),
                         "disable-dir-object": True, "execute": True, "dry_run": True}
            if action == "cp":
                arguments["recursive"] = True
            response = rpc.call(identifier, "tools/call", {"name": tool["name"], "arguments": arguments})
            assert not response.get("isError"), response
            execution = json.loads(response["content"][0]["text"])
            assert execution["exit_code"] == 0, execution
            assert "--disable-dir-object" in execution["argv"]
            assert "true" not in execution["argv"]
            malformed = {**arguments, "disable-dir-object": "true"}
            rejected = rpc.call(identifier + 10, "tools/call", {"name": tool["name"], "arguments": malformed})
            assert rejected.get("isError"), rejected
    finally:
        rpc.close()


def check_disabled_modes(fixture, action, mode):
    fixture.reset()
    fixture.upload(action, (*mode, "--disable-dir-object"))
    assert not fixture.server.created, fixture.server.created
    assert "base/empty/" not in fixture.server.objects
    assert fixture.server.objects.get("base/populated/a.txt") == b"abc", (
        fixture.surface, action, mode, fixture.server.objects, fixture.server.events)


def check_upload_modes(fixture, action):
    for mode in ((), ("--no-manifest",)):
        fixture.reset()
        fixture.upload(action, mode)
        expected = {"base/", "base/empty/", "base/populated/"}
        assert set(fixture.server.created) == expected, fixture.server.created
        assert fixture.server.objects["base/populated/a.txt"] == b"abc"
        fixture.upload(action, mode)
        check_disabled_modes(fixture, action, mode)


def check_uploads(fixture):
    for bucket_type in ("fns", "hns") if fixture.surface == "ve-tos" else ("fns",):
        fixture.server.bucket_type = bucket_type
        for action in ("cp", "sync"):
            check_upload_modes(fixture, action)


def check_empty_and_plans(fixture):
    empty = fixture.root / "only-empty"
    empty.mkdir()
    for action in ("cp", "sync"):
        for flags in ((), ("--no-manifest",), ("--include-parent",)):
            fixture.reset()
            fixture.upload(action, flags, empty)
            expected = "base/only-empty/" if "--include-parent" in flags else "base/"
            # [Review Fix #DirectoryFixture4] ADrive explicitly ensures missing destination ancestors.
            assert expected in fixture.server.created, fixture.server.created
            assert set(fixture.server.created) <= {"base/", expected}, fixture.server.created
        fixture.reset()
        credential_before = (fixture.root / "config").read_bytes()
        secrets_before = (fixture.root / "credentials").read_bytes()
        fixture.upload(action, ("--dry-run",), empty)
        assert not fixture.server.events, fixture.server.events
        assert (fixture.root / "config").read_bytes() == credential_before
        assert (fixture.root / "credentials").read_bytes() == secrets_before


def check_discovery(fixture):
    for action in ("cp", "sync"):
        for language in ("en", "zh"):
            result = fixture.invoke([action, "--help", "--help-language", language])
            assert result.returncode == 0 and "--disable-dir-object" in result.stdout
            assert "--ddo" not in result.stdout
            if language == "zh":
                assert "跳过显式目录上传" in result.stdout, result.stdout
        result = fixture.invoke([action, "--describe"])
        assert result.returncode == 0, result.stderr
        parameters = json.loads(result.stdout)["data"]["parameters"]
        option = next(value for value in parameters if value["name"] == "disable-dir-object")
        assert option["schema"]["type"] == "boolean", option
        for alias in ("--ddo", "-ddo"):
            arguments = [action, str(fixture.source), fixture.destination(), "--dry-run", alias]
            if action == "cp":
                arguments.append("--recursive")
            rejected = fixture.invoke(arguments)
            assert rejected.returncode != 0, (fixture.surface, arguments, rejected.stdout)
    result = fixture.invoke(["skill", "list"])
    assert result.returncode == 0, result.stderr
    skills = json.loads(result.stdout)["data"]["skills"]
    for action in ("cp", "sync"):
        skill = next(value for value in skills if value["name"].endswith("_" + action))
        assert skill["input_schema"]["properties"]["disable-dir-object"]["type"] == "boolean", skill


def policy_descriptions(value):
    if isinstance(value, dict):
        if value.get("name") == "disable-dir-object":
            yield value.get("description", "")
        if isinstance(value.get("disable-dir-object"), dict):
            yield value["disable-dir-object"].get("description", "")
        for child in value.values():
            yield from policy_descriptions(child)
    elif isinstance(value, list):
        for child in value:
            yield from policy_descriptions(child)


def check_localized_metadata(fixture):
    for language in ("en", "zh"):
        expected = "preserve directories including empty directories" if language == "en" else "默认保留目录，包括空目录"
        for command in (["capabilities", "--view", "full"], ["skill", "list"],
                        ["cp", "--describe"], ["sync", "--describe"]):
            if language == "zh" and command[0] == "capabilities":
                continue
            language_args = [] if command[0] == "capabilities" else ["--language", language]
            result = fixture.invoke([*command, *language_args])
            assert result.returncode == 0, result.stderr
            descriptions = list(policy_descriptions(json.loads(result.stdout)))
            assert descriptions and all(expected in value for value in descriptions), (
                fixture.surface, command, language, descriptions)
            assert "--ddo" not in result.stdout, (fixture.surface, command, language)


def main():
    binaries = [Path(value).resolve() for value in sys.argv[1:]]
    assert binaries, "pass unified and/or standalone binary paths"
    count = 0
    for binary in binaries:
        surfaces = {"tos-cli": ["tos"], "ve-tos-cli": ["ve-tos"], "ve-adrive-cli": ["ve-adrive"]}
        for surface in surfaces.get(binary.name, ["tos", "ve-tos", "ve-adrive"]):
            with tempfile.TemporaryDirectory(prefix="directory-upload-") as folder:
                fixture = Fixture(binary, surface, Path(folder))
                try:
                    check_discovery(fixture)
                    check_localized_metadata(fixture)
                    check_uploads(fixture)
                    check_empty_and_plans(fixture)
                    fixture.reset()
                    check_mcp(fixture)
                    assert not fixture.server.events, fixture.server.events
                    count += 1
                    print(f"PASS: {binary.name} {surface}; directories, opt-out, modes, repeat, empty root, discovery")
                finally:
                    fixture.close()
    print(f"PASS: {count} entrypoint/surface combinations")


if __name__ == "__main__":
    main()
