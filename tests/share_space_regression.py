"""Real-entrypoint whole-Space share listing, mapping and recovery on loopback."""
from contextlib import contextmanager
from dataclasses import dataclass, field
import http.server
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
import traceback
from urllib.parse import parse_qs, urlencode, urlsplit


CONTENTS = {"root.txt": b"root", "docs/a.txt": b"docs",
            "other/a.txt": b"other", "docs/skip.log": b"skip"}
TOKEN = "space-fixture-token-secret"


@dataclass
class Scenario:
    is_share_all_files: bool | None = True
    is_whole: bool = True
    is_empty: bool = False
    failure_code: str = "ShareFileOutOfScope"
    failure_status: int = 403
    policy: str = ""
    unsafe_path: str = ""
    denied_issue: str = ""
    is_root_denied: bool = False
    token_status: int = 200
    create_page: str | None = "https://server-pages.example.test/share/s"
    create_status: int = 200
    is_expiring: bool = False
    requests: list = field(default_factory=list)
    is_failed: bool = False


class Receiver(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_arguments):
        pass

    def respond(self, payload, status=200):
        encoded = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Length", str(len(encoded)))
        self.send_header("x-ids-request-id", "space-fixture-request")
        self.end_headers()
        self.wfile.write(encoded)

    def do_POST(self):
        scenario = self.server.scenario
        payload = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
        scenario.requests.append((self.path, payload))
        if self.path == "/v1/share_link/create":
            assert self.headers.get("Authorization")
            if scenario.create_status != 200:
                return self.respond({"Code": "SyntheticCreateFailure"}, scenario.create_status)
            response = {"ShareID": "s", "InstanceID": "inst", "SpaceID": "space"}
            if scenario.create_page:
                response["ShareURL"] = scenario.create_page
            return self.respond(response)
        assert self.path == "/v1/share_link/get_access_token"
        assert payload["ShareID"] == "s"
        assert not self.headers.get("Authorization")
        if scenario.token_status != 200:
            return self.respond({"Code": "Forbidden"}, scenario.token_status)
        count = sum(path.endswith("get_access_token") for path, _ in scenario.requests)
        self.respond({"AccessToken": TOKEN, "ExpiresIn": 1 if scenario.is_expiring and count == 1 else 60})

    def do_GET(self):
        scenario = self.server.scenario
        parsed = urlsplit(self.path)
        query = parse_qs(parsed.query)
        scenario.requests.append((parsed.path, query))
        if parsed.path == "/v1/share_link/get_by_anonymous":
            payload = {"ShareID": "s", "FilePathList": [] if scenario.is_whole else ["docs/"]}
            if scenario.is_share_all_files is not None:
                payload["ShareAllFiles"] = scenario.is_share_all_files
            if scenario.policy:
                payload[scenario.policy] = True
            return self.respond(payload)
        if parsed.path == "/content":
            return self.content(query["path"][0])
        assert self.headers.get("x-share-token") == TOKEN
        assert not self.headers.get("Authorization")
        if parsed.path == "/v1/share_file/list":
            return self.listing(query)
        path = query["file_path"][0]
        if parsed.path == "/v1/share_file/get":
            return self.respond(metadata(path))
        assert parsed.path == "/v1/share_file/get_download_url"
        if path == scenario.denied_issue:
            scenario.is_failed = True
            return self.respond({"Code": "Forbidden.ShareDownloadDisabled"}, 403)
        origin = f"http://127.0.0.1:{self.server.server_port}"
        self.respond({"DownloadURL": f"{origin}/content?{urlencode({'path': path, 'secret': 'signed-secret'})}"})

    def listing(self, query):
        scenario = self.server.scenario
        prefix = query.get("prefix", [""])[0]
        is_virtual = query.get("list_root") == ["true"]
        assert query["delimiter"] == ["/"]
        if is_virtual and scenario.is_whole:
            assert not prefix
            if scenario.is_expiring:
                time.sleep(1.1)
            return self.respond({"Code": scenario.failure_code}, scenario.failure_status)
        assert (prefix == "/") != is_virtual or prefix not in ("", "/")
        if prefix == "/" and not scenario.is_whole:
            return self.respond({"Code": "ShareFileOutOfScope"}, 403)
        if prefix == "/" and scenario.is_root_denied:
            return self.respond({"Code": "ShareFileOutOfScope"}, 403)
        entries = directory_entries(prefix, scenario)
        start = int(query.get("marker", ["0"])[0])
        selected = entries[start:start + min(2, int(query["limit"][0]))]
        end = start + len(selected)
        marker = str(end) if end < len(entries) else ""
        self.respond({"Files": [metadata(path) for path in selected if not path.endswith("/")],
                      "Folders": [{"Folder": path} for path in selected if path.endswith("/")],
                      "NextMarker": marker, "IsTruncated": bool(marker)})

    def content(self, path):
        assert not self.headers.get("x-share-token") and not self.headers.get("Authorization")
        encoded = CONTENTS[path]
        self.send_response(200)
        self.send_header("Content-Length", str(len(encoded)))
        self.send_header("ETag", f'"etag-{path}"')
        self.end_headers()
        self.wfile.write(encoded)


def metadata(path):
    if path.endswith("/"):
        return {"FilePath": path, "FileType": "folder"}
    return {"FilePath": path, "Size": len(CONTENTS.get(path, b"unsafe")),
            "ETag": f"etag-{path}", "UpdatedAt": 1700000000000}


def directory_entries(prefix, scenario):
    if scenario.unsafe_path:
        return [scenario.unsafe_path]
    if scenario.is_empty:
        return []
    return {"/": ["root.txt", "docs/", "other/", "empty/"],
            "": ["docs/"], "docs/": ["docs/a.txt", "docs/skip.log"],
            "other/": ["other/a.txt"], "empty/": []}[prefix]


@contextmanager
def fixture(scenario):
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Receiver)
    server.scenario = scenario
    worker = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.01}, daemon=True)
    worker.start()
    try:
        yield f"http://127.0.0.1:{server.server_port}"
    finally:
        server.shutdown()
        server.server_close()
        worker.join(timeout=5)
        assert not worker.is_alive()


def invoke(binary, root, origin, command, options=()):
    prefix = [] if binary.stem == "ve-adrive-cli" else ["ve-adrive"]
    command_args = [f"{origin}/share/s"]
    if command == "download":
        command_args += [str(root / "out"), "--report-path", str(root / "report.csv"),
                         "--manifest-path", str(root / "manifest.csv"),
                         "--checkpoint-dir", str(root / "checkpoints"), "--no-progress", "--no-list-echo"]
    environment = {key: value for key, value in os.environ.items()
                   if not key.startswith(("ADRIVE_", "TOS_"))}
    result = subprocess.run([str(binary), *prefix, "--config-path", str(root / "config"),
        "--credentials-path", str(root / "missing-credentials"), "--output", "json",
        "share", command, *command_args, *options], env=environment, cwd=root,
        capture_output=True, text=True, timeout=15)
    assert not any(secret in result.stdout + result.stderr for secret in (TOKEN, "signed-secret"))
    return result, json.loads(result.stdout or result.stderr)


def listings(scenario):
    return [query for path, query in scenario.requests if path == "/v1/share_file/list"]


def issued(scenario):
    return [query["file_path"][0] for path, query in scenario.requests
            if path == "/v1/share_file/get_download_url"]


def check_files(binary, root, scenario, options=()):
    with fixture(scenario) as origin:
        result, envelope = invoke(binary, root, origin, "files", ["--max-keys", "3", *options])
    assert result.returncode == 0, envelope
    assert len(envelope["data"]["Files"]) + len(envelope["data"]["Folders"]) == (3 if scenario.is_whole else 1)
    requests = listings(scenario)
    expected_virtual = 1 if scenario.is_share_all_files is None and "--path" not in options else 0
    if scenario.is_whole:
        assert sum(query.get("list_root") == ["true"] for query in requests) == expected_virtual, requests
        assert all(query["prefix"] == ["/"] for query in requests[expected_virtual:])
        assert [query["limit"] for query in requests[expected_virtual:]] == [["3"], ["1"]]
        assert envelope["data"]["NextMarker"] == "3"
        assert {item["FilePath"] for item in envelope["data"]["Files"]} == {"root.txt"}
    else:
        assert requests[0].get("list_root") == ["true"]
    assert sum(path.endswith("get_access_token") for path, _ in scenario.requests) == 1


def check_denied(binary, root, scenario):
    with fixture(scenario) as origin:
        result, envelope = invoke(binary, root, origin, "files")
    assert result.returncode != 0 and not envelope["success"], envelope
    expected = 0 if scenario.policy or scenario.token_status != 200 else (2 if scenario.is_root_denied else 1)
    assert len(listings(scenario)) == expected, scenario.requests
    if scenario.policy:
        assert not any(path.endswith("get_access_token") for path, _ in scenario.requests)


def check_download(binary, root, scenario, options=()):
    with fixture(scenario) as origin:
        result, envelope = invoke(binary, root, origin, "download", ["--recursive", *options])
    assert result.returncode == 0, envelope
    expected = {} if scenario.is_empty else CONTENTS
    if "--include" in options:
        expected = {path: content for path, content in expected.items() if path.endswith(".txt")}
    if not scenario.is_whole:
        expected = {path.removeprefix("docs/"): content for path, content in CONTENTS.items() if path.startswith("docs/")}
    assert {path.relative_to(root / "out").as_posix(): path.read_bytes()
            for path in (root / "out").rglob("*") if path.is_file()} == expected
    if scenario.is_whole and not scenario.is_empty and "--include" not in options:
        assert (root / "out/empty").is_dir()
    if "--include" in options:
        assert not (root / "out/empty").exists()
    assert len(issued(scenario)) == len(expected), scenario.requests
    assert len(issued(scenario)) == len(set(issued(scenario))), "URL issuance replayed"


def check_recursive(binary, root, is_explicit):
    scenario = Scenario(is_empty=True)
    with fixture(scenario) as origin:
        result, envelope = invoke(binary, root, origin, "download", ["--path", "/"] if is_explicit else [])
    assert result.returncode == 6 and "recursive" in envelope["error"]["message"], envelope
    assert not issued(scenario)


def snapshot(root):
    return {path.relative_to(root).as_posix(): ("directory" if path.is_dir() else path.read_bytes())
            for path in root.rglob("*")}


def check_dry_run(binary, root):
    scenario = Scenario(is_share_all_files=None)
    before = snapshot(root)
    with fixture(scenario) as origin:
        result, envelope = invoke(binary, root, origin, "download", ["--recursive", "--dry-run",
            "--checkpoint", str(root / "journal"), "--include-parent"])
    assert result.returncode == 0, envelope
    files = envelope["data"]["selection"]["Manifest"]["Files"]
    assert {entry["LocalPath"] for entry in files} == set(CONTENTS), files
    assert not issued(scenario) and before == snapshot(root)


def check_resume(binary, root):
    scenario = Scenario(is_share_all_files=None, denied_issue="other/a.txt")
    with fixture(scenario) as origin:
        options = ["--recursive", "--checkpoint", str(root / "journal"), "--batch-concurrency", "1"]
        failed, first = invoke(binary, root, origin, "download", options)
        assert failed.returncode != 0 and scenario.is_failed, first
        journal = json.loads((root / "journal").read_text())
        assert journal["manifest"]["roots"] == ["/"] and journal["manifest"]["policy"]["mode"] == "space_root"
        count = len(issued(scenario))
        scenario.denied_issue = ""
        rejected, invalid = invoke(binary, root, origin, "download", [*options, "--resume", "--include-parent"])
        assert rejected.returncode == 6 and not invalid["success"], invalid
        before = snapshot(root)
        planned, plan = invoke(binary, root, origin, "download", [*options, "--resume", "--dry-run"])
        assert planned.returncode == 0 and plan["success"], plan
        assert len(issued(scenario)) == count and before == snapshot(root)
        result, envelope = invoke(binary, root, origin, "download", [*options, "--resume"])
    assert result.returncode == 0 and envelope["data"]["Summary"]["Skipped"] == 3, envelope
    assert issued(scenario)[count:] == ["other/a.txt"], scenario.requests
    assert (root / "out/docs/a.txt").read_bytes() == CONTENTS["docs/a.txt"]
    assert (root / "out/other/a.txt").read_bytes() == CONTENTS["other/a.txt"]


def check_unsafe(binary, root, path):
    scenario = Scenario(unsafe_path=path)
    with fixture(scenario) as origin:
        result, envelope = invoke(binary, root, origin, "download", ["--recursive"])
    assert result.returncode == 6 and not envelope["success"], envelope
    assert not issued(scenario)


def check_cursor(binary, root):
    scenario = Scenario(is_share_all_files=None)
    with fixture(scenario) as origin:
        result, envelope = invoke(binary, root, origin, "files", ["--marker", "2", "--max-keys", "1"])
    assert result.returncode == 0 and envelope["data"]["Folders"][0]["Folder"] == "other/", envelope
    requests = listings(scenario)
    assert len(requests) == 2 and all(query["marker"] == ["2"] for query in requests)
    assert all(query["limit"] == ["1"] for query in requests)


def check_explicit_ordinary_root(binary, root):
    scenario = Scenario(is_share_all_files=None, is_whole=False)
    with fixture(scenario) as origin:
        result, envelope = invoke(binary, root, origin, "files", ["--path", "/"])
    assert result.returncode == 5 and not envelope["success"], envelope
    assert len(listings(scenario)) == 1 and listings(scenario)[0]["prefix"] == ["/"]


def check_fallback_session_expiry(binary, root):
    scenario = Scenario(is_share_all_files=None, is_expiring=True)
    with fixture(scenario) as origin:
        result, envelope = invoke(binary, root, origin, "files", ["--max-keys", "1"])
    assert result.returncode == 0 and len(envelope["data"]["Files"]) == 1, envelope
    assert [path for path, _ in scenario.requests] == ["/v1/share_link/get_by_anonymous",
        "/v1/share_link/get_access_token", "/v1/share_file/list",
        "/v1/share_link/get_access_token", "/v1/share_file/list"], scenario.requests


def invoke_create(binary, root, arguments):
    prefix = [] if binary.stem == "ve-adrive-cli" else ["ve-adrive"]
    environment = {key: value for key, value in os.environ.items()
                   if not key.startswith(("ADRIVE_", "TOS_"))}
    result = subprocess.run([str(binary), *prefix, "--config-path", str(root / "config"),
        "--credentials-path", str(root / "credentials"), "--output", "json", "share", "create",
        *arguments, "--auth-mode", "aksk"], env=environment, cwd=root,
        capture_output=True, text=True, timeout=15)
    assert "fixture-sk" not in result.stdout + result.stderr
    return result, json.loads(result.stdout or result.stderr)


def configure_create(root, origin):
    (root / "config").write_text(f"[default.adrive]\nauth_mode='aksk'\nendpoint='{origin}'\nregion='cn-beijing'\n")
    (root / "credentials").write_text("schema_version=1\n[default.adrive]\n"
                                      "access_key_id='fixture-ak'\nsecret_access_key='fixture-sk'\n")


def check_create(binary, root):
    scenario = Scenario()
    targets = [(["adrive://inst/space"], None), (["adrive://inst/space/"], None),
        (["--instance", "inst", "--space", "space"], None),
        (["--instance", "inst", "--space", "space", "--folder", "/"], None),
        (["adrive://inst/space/docs/a.txt"], "docs/a.txt"),
        (["adrive://inst/space/docs/"], "docs/"),
        (["--instance", "inst", "--space", "space", "--folder", "docs/"], "docs/"),
        (["--instance", "inst", "--space", "space", "--file", "docs/a.txt"], "docs/a.txt")]
    with fixture(scenario) as origin:
        configure_create(root, origin)
        for arguments, path in targets:
            count = len(scenario.requests)
            result, envelope = invoke_create(binary, root, arguments)
            assert result.returncode == 0, envelope
            assert len(scenario.requests) == count + 1, "create replayed or made receiver requests"
            route, payload = scenario.requests[-1]
            assert route == "/v1/share_link/create" and payload["InstanceID"] == "inst" and payload["SpaceID"] == "space"
            assert payload["FilePathList"] == ([] if path is None else [path])
            assert (payload.get("ShareAllFiles") is True) if path is None else ("ShareAllFiles" not in payload)
            assert envelope["data"]["ShareUrl"] == scenario.create_page


def check_create_origin(binary, root):
    scenario = Scenario(create_page=None)
    with fixture(scenario) as origin:
        configure_create(root, origin)
        result, envelope = invoke_create(binary, root, ["adrive://inst/space", "--share-endpoint", "https://explicit-pages.example.test"])
        assert result.returncode == 0 and envelope["data"]["ShareUrl"] == "https://explicit-pages.example.test/share/s", envelope
        scenario.create_page = "https://server-pages.example.test/share/s"
        result, envelope = invoke_create(binary, root, ["adrive://inst/space", "--share-endpoint", "https://explicit-pages.example.test"])
        assert result.returncode == 0 and envelope["data"]["ShareUrl"] == "https://explicit-pages.example.test/share/s", envelope
        scenario.create_status = 503
        count = len(scenario.requests)
        result, envelope = invoke_create(binary, root, ["adrive://inst/space"])
        assert result.returncode != 0 and not envelope["success"], envelope
        assert len(scenario.requests) == count + 1, "ambiguous create was replayed"


def check_create_invalid_scope(binary, root):
    scenario = Scenario()
    targets = [["adrive://inst/space//"], ["adrive://inst/space///"],
        ["--instance", "inst", "--space", "space", "--folder", "//"],
        ["--instance", "inst", "--space", "space", "--folder", "///"],
        ["--instance", "inst", "--space", "space", "--file", "/"],
        ["--instance", "inst", "--space", "space", "--folder", ""],
        ["--instance", "inst", "--space", "space", "--file", ""]]
    with fixture(scenario) as origin:
        configure_create(root, origin)
        for arguments in targets:
            result, envelope = invoke_create(binary, root, arguments)
            assert result.returncode == 6 and not envelope["success"], (arguments, envelope)
            assert not scenario.requests, "an invalid selector reached share creation"


def cases():
    yield "files-known", lambda binary, root: check_files(binary, root, Scenario())
    for code in ("ShareFileOutOfScope", "Forbidden.ShareFileOutOfScope"):
        yield f"files-fallback-{code}", lambda binary, root, code=code: check_files(binary, root, Scenario(is_share_all_files=None, failure_code=code))
    yield "files-explicit-root", lambda binary, root: check_files(binary, root, Scenario(is_share_all_files=None), ["--path", "/"])
    yield "ordinary-root", lambda binary, root: check_files(binary, root, Scenario(is_share_all_files=False, is_whole=False))
    yield "ordinary-unknown-flag", lambda binary, root: check_files(binary, root, Scenario(is_share_all_files=None, is_whole=False))
    yield "cursor", check_cursor
    yield "fallback-session-expiry", check_fallback_session_expiry
    yield "ordinary-explicit-root-denied", check_explicit_ordinary_root
    for code in ("Forbidden", "Forbidden.ShareDownloadDisabled", "NotShareFileOutOfScope", "Forbidden.ShareFileOutOfScope.Other"):
        yield f"denied-{code}", lambda binary, root, code=code: check_denied(binary, root, Scenario(is_share_all_files=None, failure_code=code))
    yield "false-flag-denied", lambda binary, root: check_denied(binary, root, Scenario(is_share_all_files=False))
    yield "wrong-status-denied", lambda binary, root: check_denied(binary, root, Scenario(is_share_all_files=None, failure_status=401))
    yield "root-probe-denied", lambda binary, root: check_denied(binary, root, Scenario(is_share_all_files=None, is_root_denied=True))
    yield "token-denied", lambda binary, root: check_denied(binary, root, Scenario(is_share_all_files=None, token_status=403))
    for policy in ("RequireLogin", "IsExpired", "IsCancelled"):
        yield policy, lambda binary, root, policy=policy: check_denied(binary, root, Scenario(policy=policy))
    yield "download-known", lambda binary, root: check_download(binary, root, Scenario())
    yield "download-fallback", lambda binary, root: check_download(binary, root, Scenario(is_share_all_files=None))
    yield "download-explicit", lambda binary, root: check_download(binary, root, Scenario(is_share_all_files=None), ["--path", "/"])
    yield "include-parent", lambda binary, root: check_download(binary, root, Scenario(), ["--include-parent"])
    yield "filter", lambda binary, root: check_download(binary, root, Scenario(), ["--include", "*.txt"])
    yield "empty", lambda binary, root: check_download(binary, root, Scenario(is_empty=True))
    yield "empty-fallback", lambda binary, root: check_download(binary, root, Scenario(is_share_all_files=None, is_empty=True))
    yield "ordinary-download", lambda binary, root: check_download(binary, root, Scenario(is_share_all_files=False, is_whole=False))
    yield "recursive", lambda binary, root: check_recursive(binary, root, False)
    yield "explicit-recursive", lambda binary, root: check_recursive(binary, root, True)
    yield "dry-run", check_dry_run
    yield "resume", check_resume
    yield "create", check_create
    yield "create-origin-and-replay", check_create_origin
    yield "create-invalid-scope", check_create_invalid_scope
    for path in ("/escape.txt", "../escape.txt", "docs/../escape.txt"):
        yield f"unsafe-{path}", lambda binary, root, path=path: check_unsafe(binary, root, path)


def main():
    binaries = [Path(value).resolve() for value in sys.argv[1:]]
    assert binaries, "pass compiled unified and standalone entrypoints"
    failures = []
    count = 0
    for binary in binaries:
        for name, check in cases():
            count += 1
            with tempfile.TemporaryDirectory(prefix="share-space-") as directory:
                root = Path(directory)
                (root / "config").write_text("[default.adrive]\n")
                try:
                    check(binary, root)
                except Exception as error:
                    location = traceback.extract_tb(error.__traceback__)[-1]
                    failures.append(f"{binary.name}/{name}: line {location.lineno}: {location.line}: {error}")
    assert not failures, "\n" + "\n".join(failures)
    print(f"PASS: {len(binaries)} entrypoints; {count} whole-Space listing/download/recovery/security scenarios")


if __name__ == "__main__":
    main()
