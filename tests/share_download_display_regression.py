"""Share download summary/denial output using both real CLI entrypoints.

Run: python3 tests/share_download_display_regression.py [unified-binary] [standalone-binary]
Only isolated localhost services, synthetic tokens and temporary files are used.
"""
import http.server
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
from urllib.parse import parse_qs, urlsplit


from share_download_regression import transfer_paths


class DownloadScenario(http.server.BaseHTTPRequestHandler):
    mode = "denied"
    requests = []

    def log_message(self, *_arguments):
        pass

    def respond(self, value, status=200):
        encoded = json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Length", str(len(encoded)))
        self.send_header("x-ids-request-id", "fixture-denial-id")
        self.end_headers()
        self.wfile.write(encoded)

    def do_POST(self):
        self.rfile.read(int(self.headers.get("Content-Length", "0")))
        self.requests.append(self.path)
        assert self.path == "/v1/share_link/get_access_token"
        self.respond({"AccessToken": "fixture-access-token", "ExpiresIn": 60})

    def do_GET(self):
        self.requests.append(self.path)
        path, query = urlsplit(self.path).path, parse_qs(urlsplit(self.path).query)
        assert not self.headers.get("Authorization")
        if path == "/v1/share_link/get_by_anonymous":
            self.respond({"ShareID": "s", "Permission": "view" if self.mode == "denied" else "download",
                          "FilePathList": ["a.txt"] if self.mode == "single" else ["tree/"]})
        elif path == "/v1/share_file/get":
            selected = query["file_path"][0]
            self.respond({"FilePath": selected, "FileType": "folder" if selected.endswith("/") else "file",
                          "Size": 0 if selected.endswith("/") else 3, "ETag": "abc"})
        elif path == "/v1/share_file/list":
            self.respond_listing(query)
        elif path == "/v1/share_file/get_download_url":
            if self.mode == "denied":
                self.respond({"Code": "Forbidden.ShareDownloadDisabled"}, 403)
            else:
                self.respond({"DownloadURL": f"http://127.0.0.1:{self.server.server_port}/content"})
        elif path == "/content":
            assert not self.headers.get("x-share-token")
            self.send_response(200)
            self.send_header("Content-Length", "3")
            self.send_header("ETag", "abc")
            self.end_headers()
            self.wfile.write(b"abc")
        else:
            raise AssertionError(f"unexpected local route: {path}")

    def respond_listing(self, query):
        assert self.headers.get("x-share-token") == "fixture-access-token"
        prefix = query.get("prefix", [""])[0]
        files, folders = [], []
        if self.mode == "single":
            assert query["list_root"] == ["true"]
            files = [{"FilePath": "a.txt", "Size": 3, "ETag": "abc"}]
        elif prefix == "tree/":
            folders = [{"Folder": "tree/sub/"}]
            if self.mode == "denied":
                files = [{"FilePath": f"tree/file-{index:02d}.md", "Size": 3, "ETag": "abc"} for index in range(40)]
        elif prefix == "tree/sub/" and self.mode == "denied":
            files = [{"FilePath": "tree/sub/leaf.md", "Size": 3, "ETag": "abc"}]
        self.respond({"Files": files, "Folders": folders, "NextMarker": "", "IsTruncated": False})


def invoke(binary, root, origin, mode, presentation=("table", None)):
    output, query = presentation
    DownloadScenario.mode = mode
    prefix = [] if binary.stem == "ve-adrive-cli" else ["ve-adrive"]
    environment = {key: value for key, value in os.environ.items() if not key.startswith("ADRIVE_")}
    selector = [] if mode == "single" else ["--path", "tree/", "--recursive"]
    expression = ["--query", query] if query else []
    destination = root / f"{mode}-{output}-{bool(query)}"
    result = subprocess.run([str(binary), *prefix, "share", "download", f"{origin}/share/s",
        str(destination), *selector, "--output", output, *expression,
        *transfer_paths(root)],
        env=environment, text=True, capture_output=True, timeout=15)
    assert "fixture-access-token" not in result.stdout + result.stderr
    return result, destination


def check_empty_and_single(binary, root, origin):
    empty, destination = invoke(binary, root, origin, "empty")
    assert empty.returncode == 0 and (destination / "sub").is_dir()
    assert "complete" in empty.stdout and "summary.total" in empty.stdout, "empty download hid its summary"
    single, destination = invoke(binary, root, origin, "single")
    assert single.returncode == 0 and (destination / "a.txt").read_bytes() == b"abc"
    assert "complete" in single.stdout and "succeeded" in single.stdout


def check_denial(binary, root, origin):
    denied, destination = invoke(binary, root, origin, "denied")
    assert denied.returncode == 5 and not list(destination.rglob("*.md"))
    assert "Forbidden.ShareDownloadDisabled" in denied.stdout and "data.summary.pending" in denied.stdout
    assert max(map(len, denied.stdout.splitlines())) < 240, "files were packed into one oversized JSON table cell"
    assert "LocalPath" not in denied.stdout and "failed" in denied.stdout and "pending" in denied.stdout
    result, _destination = invoke(binary, root, origin, "denied", ("json", None))
    envelope = json.loads(result.stdout)
    assert envelope["ec"] == "Forbidden.ShareDownloadDisabled" and envelope["status_code"] == 403
    assert envelope["request_id"] == "fixture-denial-id" and envelope["data"]["IssuanceOutcomeUnknown"] is False
    assert envelope["data"]["Summary"] == {"Total": 41, "Completed": 0, "Failed": 41, "Pending": 0, "Skipped": 0}
    assert "share owner" in envelope["error"]["suggested_action"] and "--permission download" in envelope["error"]["suggested_action"]
    assert "refresh credentials" not in envelope["error"]["suggested_action"]
    assert "permission" in envelope["error"]["message"] and "quota" in envelope["error"]["message"]
    selected, _destination = invoke(binary, root, origin, "denied", ("table", "data.Files[].{name:Path,result:Status}"))
    assert "name" in selected.stdout and "result" in selected.stdout and "data.Summary" not in selected.stdout


def main():
    repository = Path(__file__).resolve().parent.parent
    suffix = ".exe" if os.name == "nt" else ""
    binaries = [Path(argument).resolve() for argument in sys.argv[1:]] or [
        repository / f"target/debug/ve-storage-uni-cli{suffix}",
        repository / f"packaging/cargo/ve-adrive-cli/target/debug/ve-adrive-cli{suffix}"]
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), DownloadScenario)
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    try:
        origin = f"http://127.0.0.1:{server.server_port}"
        for binary in binaries:
            with tempfile.TemporaryDirectory(prefix="share-download-display-", dir=Path(tempfile.gettempdir()).resolve()) as folder:
                check_empty_and_single(binary, Path(folder), origin)
                check_denial(binary, Path(folder), origin)
        print(f"PASS: {len(binaries)} entrypoints; empty/single summaries, bounded denial table, service guidance and query")
    finally:
        server.shutdown()
        server.server_close()
        worker.join()


if __name__ == "__main__":
    main()
