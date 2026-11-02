"""Real-entrypoint share files max-keys parity, using localhost metadata only."""
import http.server
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
from urllib.parse import parse_qs, urlsplit


class Listing(http.server.BaseHTTPRequestHandler):
    requests = []
    entries = ["docs/a/", "docs/b.txt", "docs/c/", "docs/d.txt", "docs/e.txt", "docs/f/"]

    def log_message(self, *_arguments):
        pass

    def reply(self, value):
        encoded = json.dumps(value).encode()
        self.send_response(200)
        self.send_header("Content-Length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)

    def do_POST(self):
        self.rfile.read(int(self.headers.get("Content-Length", "0")))
        self.requests.append(self.path)
        self.reply({"AccessToken": "pagination-token-secret", "ExpiresIn": 60})

    def do_GET(self):
        self.requests.append(self.path)
        path, query = urlsplit(self.path).path, parse_qs(urlsplit(self.path).query)
        if path.endswith("get_by_anonymous"):
            self.reply({"ShareID": query["share_id"][0], "FilePathList": ["docs/"]})
        elif path == "/v1/share_file/get":
            self.reply({"FilePath": query["file_path"][0], "Size": 3})
        elif path == "/v1/share_file/list":
            self.listing(query)
        else:
            raise AssertionError(f"unexpected metadata route: {path}")

    def listing(self, query):
        prefix = query.get("prefix", [""])[0]
        start = 0 if prefix == "stalled/" else int(query.get("marker", ["0"])[0])
        limit = int(query["limit"][0])
        assert query["delimiter"] == ["/"] and 0 < limit <= 1000
        entries = ([f"large/item-{index}/" if index % 3 == 0 else f"large/item-{index}.txt" for index in range(1003)]
                   if prefix == "large/" else self.entries)
        page_size = 500 if prefix == "large/" else 2
        selected = [] if prefix == "empty/" else entries[start:start + min(limit, page_size)]
        if prefix == "overfull/":
            selected = entries[:limit + 1]
        end = start + len(selected)
        marker = str(end) if selected and end < len(entries) else ""
        if prefix == "stalled/":
            selected, marker = [], query.get("marker", ["same"])[0]
        self.reply({"Files": [{"FilePath": item, "Size": 3} for item in selected if not item.endswith("/")],
                    "Folders": [{"Folder": item} for item in selected if item.endswith("/")],
                    "NextMarker": marker, "IsTruncated": bool(marker)})


def invoke(binary, root, origin, extra=()):
    prefix = [] if binary.stem == "ve-adrive-cli" else ["ve-adrive"]
    environment = {key: value for key, value in os.environ.items() if not key.startswith("ADRIVE_")}
    result = subprocess.run([str(binary), *prefix, "--config-path", str(root / "config"),
        "--credentials-path", str(root / "credentials"), "--output", "json", "share", "files",
        f"{origin}/share/s", *extra], capture_output=True, text=True, timeout=12, env=environment)
    assert "pagination-token-secret" not in result.stdout + result.stderr
    return result, json.loads(result.stdout or result.stderr)


def paths(envelope):
    listed = envelope["data"]
    return {entry["FilePath"] for entry in listed["Files"]} | {entry["Folder"] for entry in listed["Folders"]}


def check_pages(binary, root, origin):
    before = len(Listing.requests)
    result, first = invoke(binary, root, origin, ["--path", "docs/", "--max-keys", "3"])
    assert result.returncode == 0, first
    assert len(paths(first)) == 3 and first["pagination"]["total_returned"] == 3
    assert first["data"]["Selection"]["MaxKeys"] == 3
    marker = first["data"]["NextMarker"]
    assert first["data"]["IsTruncated"] and marker == first["pagination"]["next_marker"]
    queries = [parse_qs(urlsplit(path).query) for path in Listing.requests[before:] if "/share_file/list?" in path]
    assert [int(query["limit"][0]) for query in queries] == [3, 1]
    result, second = invoke(binary, root, origin, ["--path", "docs/", "--max-keys", "3", "--marker", marker])
    assert result.returncode == 0 and len(paths(second)) == 3, second
    assert not second["data"]["IsTruncated"] and not second["data"].get("NextMarker")
    assert paths(first).isdisjoint(paths(second)) and paths(first) | paths(second) == set(Listing.entries)


def check_defaults_and_exact(binary, root, origin):
    result, envelope = invoke(binary, root, origin, ["--path", "docs/"])
    assert result.returncode == 0 and paths(envelope) == set(Listing.entries), envelope
    assert envelope["data"]["Selection"]["MaxKeys"] == 1000
    result, envelope = invoke(binary, root, origin, ["--path", "docs/b.txt", "--max-keys", "1"])
    assert result.returncode == 0 and paths(envelope) == {"docs/b.txt"}, envelope
    result, envelope = invoke(binary, root, origin, ["--path", "empty/", "--max-keys", "1"])
    assert result.returncode == 0 and paths(envelope) == set(), envelope


def check_large_default(binary, root, origin):
    result, first = invoke(binary, root, origin, ["--path", "large/"])
    assert result.returncode == 0 and len(paths(first)) == 1000, first
    assert first["data"]["NextMarker"] == "1000" and first["pagination"]["total_returned"] == 1000
    result, second = invoke(binary, root, origin, ["--path", "large/", "--marker", "1000"])
    assert result.returncode == 0 and len(paths(second)) == 3, second
    assert not second["data"]["IsTruncated"] and paths(first).isdisjoint(paths(second))


def check_validation(binary, root, origin):
    before = len(Listing.requests)
    for extra in (["--limit", "1"], ["--max-keys", "0"], ["--max-keys", "-1"],
                  ["--max-keys", "2147483648"], ["--marker", "bad\nmarker"]):
        result, envelope = invoke(binary, root, origin, extra)
        assert result.returncode == 6, envelope
    assert len(Listing.requests) == before
    result, envelope = invoke(binary, root, origin, ["--path", "stalled/", "--max-keys", "2"])
    assert result.returncode == 6 and "repeated a pagination marker" in envelope["error"]["message"], envelope
    result, envelope = invoke(binary, root, origin, ["--path", "overfull/", "--max-keys", "1"])
    assert result.returncode == 6 and "more entries than" in envelope["error"]["message"], envelope


def main():
    binaries = [Path(value).resolve() for value in sys.argv[1:]]
    assert binaries, "pass compiled CLI entrypoint paths"
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Listing)
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    try:
        for binary in binaries:
            with tempfile.TemporaryDirectory(prefix="share-pagination-") as directory:
                root, origin = Path(directory), f"http://127.0.0.1:{server.server_port}"
                check_pages(binary, root, origin)
                check_defaults_and_exact(binary, root, origin)
                check_large_default(binary, root, origin)
                check_validation(binary, root, origin)
        print(f"PASS: {len(binaries)} entrypoints; max-keys, mixed pages, continuation, default, exact, empty and validation")
    finally:
        server.shutdown()
        server.server_close()
        worker.join()


if __name__ == "__main__":
    main()
