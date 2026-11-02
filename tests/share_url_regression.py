"""Share URL compatibility through both compiled entrypoints and local fixtures.

Run: python3 tests/share_url_regression.py [unified-binary] [standalone-binary]
Credentials and passwords are synthetic; no user config or cloud service is used.
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


class Receiver(http.server.BaseHTTPRequestHandler):
    requests = []

    def log_message(self, *_arguments):
        pass

    def respond(self, value):
        encoded = json.dumps(value).encode()
        self.send_response(200)
        self.send_header("Content-Length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)

    def do_POST(self):
        self.requests.append(self.path)
        body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        assert self.path == "/v1/share_link/get_access_token"
        assert json.loads(body) == {"ShareID": "s", "SharePwd": "fixture-password"}
        self.respond({"AccessToken": "fixture-share-token", "ExpiresIn": 60})

    def do_GET(self):
        self.requests.append(self.path)
        path = urlsplit(self.path).path
        query = parse_qs(urlsplit(self.path).query)
        assert not self.headers.get("Authorization")
        if path == "/v1/share_link/get_by_anonymous":
            assert query["share_id"] == ["s"]
            self.respond({"ShareID": "s", "HasPassword": True})
        elif path == "/v1/share_file/list":
            assert self.headers.get("x-share-token") == "fixture-share-token"
            assert query["prefix"] == ["docs/"] and query["limit"] == ["1"]
            assert query["marker"] == ["previous"] and query["delimiter"] == ["/"]
            self.respond({"Files": [{"FilePath": "docs/report.txt", "Size": 3}],
                          "Folders": [], "NextMarker": "next", "IsTruncated": True})
        elif path == "/v1/share_file/get":
            assert self.headers.get("x-share-token") == "fixture-share-token"
            self.respond({"FilePath": "docs/report.txt", "Size": 3, "ETag": "abc"})
        elif path == "/v1/share_file/get_download_url":
            assert self.headers.get("x-share-token") == "fixture-share-token"
            self.respond({"DownloadURL": f"http://127.0.0.1:{self.server.server_port}/content"})
        elif path == "/content":
            assert not self.headers.get("x-share-token")
            self.send_response(200)
            self.send_header("Content-Length", "3")
            self.send_header("ETag", "abc")
            self.end_headers()
            self.wfile.write(b"abc")
        else:
            raise AssertionError(f"unexpected fixture route: {path}")


def invoke(binary, root, arguments):
    # [Review Fix #ShareTrailingSlash5] Windows's .exe suffix must not make the dedicated entrypoint look like the unified CLI.
    prefix = [] if binary.stem == "ve-adrive-cli" else ["ve-adrive"]
    environment = {key: value for key, value in os.environ.items() if not key.startswith("ADRIVE_")}
    if arguments[0] == "download":
        from share_download_regression import transfer_paths
        arguments = [*arguments, *transfer_paths(root, is_isolated_profile=False)]
        config = root / "config"
        config.write_text("")
    result = subprocess.run([str(binary), *prefix, "--config-path", str(root / "config"),
                             "--credentials-path", str(root / "credentials"), "--profile", "default",
                             "--output", "json", "share", *arguments],
                            env=environment, text=True, capture_output=True, timeout=15)
    assert "fixture-password" not in result.stdout + result.stderr
    assert "fixture-share-token" not in result.stdout + result.stderr
    return result, json.loads(result.stdout or result.stderr)


def check_dry_runs(binary, root):
    for action, scheme in ((action, scheme) for action in ("inspect", "files")
                           for scheme in ("http", "https")):
        address = f"{scheme}://vtagt3g9n.idsapp-boe.volces.com/share/2T4U8z0lY1n/"
        extra = [str(root / "dry-download")] if action == "download" else []
        result, envelope = invoke(binary, root, [action, address, *extra, "--dry-run"])
        assert result.returncode == 0, envelope
        # [Review Fix #ShareTrailingSlash2] Dry-run exposes the canonical URL in selection, not the execution payload.
        assert envelope["data"]["selection"]["ShareUrl"] == address[:-1]
    assert not (root / "dry-download").exists()
    assert not (root / "config").exists() and not (root / "credentials").exists()


def check_invalid_urls(binary, root, origin):
    before = len(Receiver.requests)
    paths = ("/share/", "/share//", "/share/s//", "/share/s/child/",
             "/share/../", "/share/%2e%2e/", "/share/%00/", "/share/%ZZ/")
    for path in paths:
        result, envelope = invoke(binary, root, ["files", f"{origin}{path}?pwd=fixture-password"])
        assert result.returncode != 0 and envelope["success"] is False
    assert len(Receiver.requests) == before, "invalid share URL reached the network"


def check_executable_alias(binary, root):
    if os.name == "nt" or binary.stem != "ve-adrive-cli":
        return
    alias = root / "ve-adrive-cli.exe"
    alias.symlink_to(binary)
    check_dry_runs(alias, root)


def check_receipt_and_resume(binary, root, origin):
    canonical = f"{origin}/share/s"
    result, envelope = invoke(binary, root, ["download", canonical + "/?pwd=fixture-password",
        str(root / "dry-download"), "--path", "docs/report.txt", "--dry-run"])
    assert result.returncode == 0 and envelope["data"]["selection"]["ShareUrl"] == canonical
    assert envelope["data"]["selection"]["FileCount"] == 1 and not (root / "dry-download").exists()
    for suffix in ("", "/"):
        address = f"{canonical}{suffix}?pwd=fixture-password"
        result, envelope = invoke(binary, root, ["inspect", address])
        assert result.returncode == 0 and envelope["data"]["ShareUrl"] == canonical
        result, envelope = invoke(binary, root, ["files", address, "--path", "docs/",
                                                  "--max-keys", "1", "--marker", "previous"])
        assert result.returncode == 0 and envelope["data"]["ShareUrl"] == canonical
        assert envelope["data"]["NextMarker"] == "next" and envelope["pagination"]["next_marker"] == "next"
    destination, checkpoint = root / "download", root / "checkpoint"
    result, envelope = invoke(binary, root, ["download", canonical + "/?pwd=fixture-password",
        str(destination), "--path", "docs/report.txt", "--checkpoint", str(checkpoint)])
    assert result.returncode == 0 and envelope["data"]["Complete"] is True, envelope
    assert (destination / "report.txt").read_bytes() == b"abc"
    assert "fixture-password" not in checkpoint.read_text() and "fixture-share-token" not in checkpoint.read_text()
    before = sum("get_download_url" in request for request in Receiver.requests)
    result, envelope = invoke(binary, root, ["download", canonical + "?pwd=fixture-password",
        str(destination), "--path", "docs/report.txt", "--checkpoint", str(checkpoint), "--resume"])
    assert result.returncode == 0 and envelope["data"]["Complete"] is True, envelope
    assert sum("get_download_url" in request for request in Receiver.requests) == before


def main():
    repository = Path(__file__).resolve().parent.parent
    binary_suffix = ".exe" if os.name == "nt" else ""
    binaries = [Path(argument).resolve() for argument in sys.argv[1:]] or [
        repository / f"target/debug/ve-storage-uni-cli{binary_suffix}",
        repository / f"packaging/cargo/ve-adrive-cli/target/debug/ve-adrive-cli{binary_suffix}"]
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Receiver)
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    try:
        origin = f"http://127.0.0.1:{server.server_port}"
        for binary in binaries:
            # [Review Fix #ShareTrailingSlash4] Use the platform's resolved temporary root so this regression also runs outside macOS.
            with tempfile.TemporaryDirectory(prefix="share-url-", dir=Path(tempfile.gettempdir()).resolve()) as folder:
                root = Path(folder)
                before = len(Receiver.requests)
                check_dry_runs(binary, root)
                check_executable_alias(binary, root)
                assert len(Receiver.requests) == before
                check_invalid_urls(binary, root, origin)
                check_receipt_and_resume(binary, root, origin)
        print(f"PASS: {len(binaries)} entrypoints; optional slash, HTTP/HTTPS, prefix/pagination, resume, redaction and invalid routes")
    finally:
        server.shutdown()
        server.server_close()
        worker.join()


if __name__ == "__main__":
    main()
