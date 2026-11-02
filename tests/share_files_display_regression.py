"""Compare share files against ls through both compiled CLI entrypoints.

Run: python3 tests/share_files_display_regression.py [unified-binary] [standalone-binary]
Uses isolated local fixtures and synthetic credentials; never contacts a cloud service.
"""
import csv
import http.server
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
from urllib.parse import parse_qs, urlsplit


COLUMNS = ["file_path", "size", "file_type", "updated_at", "is_folder"]
CASES = {
    "mixed": {"Files": [{"FilePath": "docs/report.txt", "Size": 12, "UpdatedAt": 10}],
              "Folders": [{"Folder": "docs/sub/", "UpdatedAt": 20}]},
    "empty": {"Files": [], "Folders": []},
    "folders": {"Files": [], "Folders": [{"Folder": "docs/empty/", "UpdatedAt": 20}]},
    "markers": {"Files": [{"FilePath": "docs/", "FileType": "folder"},
        {"FilePath": "docs/sub/", "FileType": "folder", "UpdatedAt": 20},
        {"FilePath": "docs/empty/", "FileType": "folder", "UpdatedAt": 30},
        {"FilePath": "docs/report.txt", "Size": -1, "FileType": "", "UpdatedAt": 10}],
        "Folders": [{"Folder": "docs/sub/", "UpdatedAt": 20}]},
}


class Listing(http.server.BaseHTTPRequestHandler):
    requests = []

    def log_message(self, *_arguments):
        pass

    def respond(self, payload):
        encoded = json.dumps(payload).encode()
        self.send_response(200)
        self.send_header("Content-Length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
        self.requests.append(self.path)
        assert self.path == "/v1/share_link/get_access_token" and body["ShareID"] in [*CASES, "paged"]
        assert not self.headers.get("Authorization")
        self.respond({"AccessToken": f'fixture-share-token-{body["ShareID"]}', "ExpiresIn": 60})

    def do_GET(self):
        self.requests.append(self.path)
        path, query = urlsplit(self.path).path, parse_qs(urlsplit(self.path).query)
        if path == "/v1/share_link/get_by_anonymous":
            self.respond({"ShareID": query["share_id"][0], "FilePathList": ["docs/"]})
        elif path == "/v1/share_file/get":
            self.respond(CASES["mixed"]["Files"][0])
        elif path == "/v1/share_file/list":
            token = self.headers.get("x-share-token", "")
            assert token.startswith("fixture-share-token-")
            assert not self.headers.get("Authorization") and query["prefix"] == ["docs/"]
            self.respond_listing(token.removeprefix("fixture-share-token-"), query)
        else:
            case = path.split("/")[-2]
            assert path == f"/v1/instances/inst/spaces/{case}/files"
            self.respond_listing(case, query)

    def respond_listing(self, case, query):
        if case == "paged":
            assert query["marker"] == ["previous"] and query["limit"] == ["1"]
            self.respond({"Files": [{"FilePath": "docs/page.txt", "Size": 1}],
                          "Folders": [], "NextMarker": "next", "IsTruncated": True})
        else:
            self.respond(dict(CASES[case], NextMarker="", IsTruncated=False))


def invoke(binary, root, arguments, output="table", query=None):
    prefix = [] if binary.stem == "ve-adrive-cli" else ["ve-adrive"]
    environment = {key: value for key, value in os.environ.items() if not key.startswith("ADRIVE_")}
    extra = ["--query", query] if query else []
    result = subprocess.run([str(binary), *prefix, "--config-path", str(root / "config"),
        "--credentials-path", str(root / "credentials"), "--profile", "default",
        "--output", output, *arguments, *extra], env=environment,
        text=True, capture_output=True, timeout=15)
    assert result.returncode == 0, result.stdout or result.stderr
    assert "fixture-share-token" not in result.stdout + result.stderr
    return result.stdout


def share_arguments(origin, case, path="docs/"):
    return ["share", "files", f"{origin}/share/{case}/", "--path", path]


def check_listing(binary, root, origin, case):
    for output in ("table", "csv"):
        ordinary = invoke(binary, root, ["ls", f"adrive://inst/{case}/docs/"], output)
        shared = invoke(binary, root, share_arguments(origin, case), output)
        assert shared == ordinary, f"{case}/{output}: share files differs from ls\n{shared}\n{ordinary}"
        for column in COLUMNS:
            assert column in shared, f"missing stable column: {column}"
    if case == "mixed":
        rows = list(csv.DictReader(io.StringIO(shared)))
        assert [row["file_path"] for row in rows] == ["docs/sub/", "docs/report.txt"]


def check_exact_and_query(binary, root, origin):
    arguments = share_arguments(origin, "mixed", "docs/report.txt")
    exact = invoke(binary, root, arguments, "csv")
    rows = list(csv.DictReader(io.StringIO(exact)))
    assert len(rows) == 1 and rows[0]["file_path"] == "docs/report.txt"
    assert rows[0]["is_folder"] == "false"
    raw = json.loads(invoke(binary, root, share_arguments(origin, "mixed"), "json"))
    assert "entries" not in raw["data"] and "Files" in raw["data"] and "Folders" in raw["data"]
    selected = invoke(binary, root, arguments, "csv", "data.Files[].{path:FilePath,bytes:Size}")
    assert selected == "bytes,path\n12,docs/report.txt\n", selected


def check_pagination(binary, root, origin):
    shared = invoke(binary, root, [*share_arguments(origin, "paged"), "--max-keys", "1", "--marker", "previous"])
    ordinary = invoke(binary, root, ["ls", "adrive://inst/paged/docs/", "--max-keys", "1", "--marker", "previous"])
    assert shared == ordinary and "Total: 1 (next_marker=next)" in shared


def main():
    repository = Path(__file__).resolve().parent.parent
    suffix = ".exe" if os.name == "nt" else ""
    binaries = [Path(argument).resolve() for argument in sys.argv[1:]] or [
        repository / f"target/debug/ve-storage-uni-cli{suffix}",
        repository / f"packaging/cargo/ve-adrive-cli/target/debug/ve-adrive-cli{suffix}"]
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Listing)
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    try:
        origin = f"http://127.0.0.1:{server.server_port}"
        for binary in binaries:
            with tempfile.TemporaryDirectory(prefix="share-files-", dir=Path(tempfile.gettempdir()).resolve()) as folder:
                check_binary(binary, Path(folder), origin)
        print(f"PASS: {len(binaries)} entrypoints; ls parity, folders, empty, markers, exact file, JSON and query")
    finally:
        server.shutdown()
        server.server_close()
        worker.join()


def check_binary(binary, root, origin):
    (root / "config").write_text(f'[default.adrive]\nauth_mode="aksk"\nendpoint="{origin}"\nregion="cn-beijing"\n')
    (root / "credentials").write_text('schema_version=1\n[default.adrive]\naccess_key_id="fixture-ak"\nsecret_access_key="fixture-sk"\n')
    for case in CASES:
        check_listing(binary, root, origin, case)
    check_exact_and_query(binary, root, origin)
    check_pagination(binary, root, origin)


if __name__ == "__main__":
    main()
