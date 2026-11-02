"""Read-only share download planning through both real entrypoints, localhost only."""
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


class PlanFixture(http.server.BaseHTTPRequestHandler):
    requests = []
    can_download = False
    has_size = True

    def log_message(self, *_arguments):
        pass

    def reply(self, value):
        body = json.dumps(value).encode()
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        self.rfile.read(int(self.headers.get("Content-Length", "0")))
        self.requests.append(self.path)
        self.reply({"AccessToken": "plan-token-secret", "ExpiresIn": 600})

    def do_GET(self):
        self.requests.append(self.path)
        parsed = urlsplit(self.path)
        query = parse_qs(parsed.query)
        if parsed.path == "/v1/share_link/get_by_anonymous":
            self.reply({"ShareID": "s", "Permission": "download", "FilePathList": ["tree/"]})
        elif parsed.path == "/v1/share_file/get":
            path = query["file_path"][0]
            metadata = {"FilePath": path, "FileType": "folder" if path.endswith("/") else "file", "ETag": "abc"}
            if self.has_size:
                metadata["Size"] = 0 if path.endswith("/") else 3
            self.reply(metadata)
        elif parsed.path == "/v1/share_file/list":
            self.listing(query)
        elif self.can_download and parsed.path == "/v1/share_file/get_download_url":
            self.reply({"DownloadURL": f"http://127.0.0.1:{self.server.server_port}/content"})
        elif self.can_download and parsed.path == "/content":
            self.send_response(200)
            self.send_header("Content-Length", "3")
            self.send_header("ETag", "abc")
            self.end_headers()
            self.wfile.write(b"abc")
        else:
            raise AssertionError(f"dry-run performed forbidden request: {parsed.path}")

    def listing(self, query):
        prefix = query.get("prefix", [""])[0]
        files, folders, marker = [], [], ""
        if not prefix:
            folders = [{"Folder": "tree/"}]
        elif prefix == "tree/" and not query.get("marker"):
            files = [{"FilePath": "tree/a.txt", "Size": 3, "ETag": "abc"}]
            folders = [{"Folder": "tree/empty/"}, {"Folder": "tree/sub/"}]
            marker = "second"
        elif prefix == "tree/":
            files = [{"FilePath": "tree/b.txt", "Size": 3, "ETag": "abc"}]
        elif prefix == "tree/sub/":
            files = [{"FilePath": "tree/sub/c.txt", "Size": 3, "ETag": "abc"}]
        if not self.has_size:
            for entry in files:
                entry.pop("Size", None)
        self.reply({"Files": files, "Folders": folders, "NextMarker": marker, "IsTruncated": bool(marker)})


def invoke(binary, origin, destination, extra=(), is_dry_run=True):
    prefix = [] if binary.stem == "ve-adrive-cli" else ["ve-adrive"]
    environment = {key: value for key, value in os.environ.items() if not key.startswith("ADRIVE_")}
    arguments = [str(binary), *prefix, "share", "download", f"{origin}/share/s?pwd=plan-password-secret",
                 str(destination), "--output", "json", "--no-progress", *transfer_paths(destination.parent), *extra]
    if is_dry_run:
        arguments.append("--dry-run")
    result = subprocess.run(arguments, capture_output=True, text=True, timeout=15, env=environment)
    assert "plan-token-secret" not in result.stdout + result.stderr
    assert "plan-password-secret" not in result.stdout + result.stderr
    envelope = json.loads(result.stdout or result.stderr)
    return result, envelope


def check_tree(binary, origin, root):
    destination, checkpoint = root / "out", root / "new-state" / "job.json"
    before = len(PlanFixture.requests)
    result, envelope = invoke(binary, origin, destination,
                             ["--path", "tree/", "--recursive", "--include-parent", "--checkpoint", str(checkpoint)])
    assert result.returncode == 0, envelope
    selection = envelope["data"]["selection"]
    assert selection["PlanComplete"] is True, "dry-run still returned an incomplete static plan"
    assert selection["FileCount"] == 3 and selection["TotalBytes"] == 9
    assert selection["DirectoryCount"] == 3 and selection["Manifest"]
    assert selection["Conflicts"] == []
    assert not destination.exists() and not checkpoint.parent.exists()
    requests = PlanFixture.requests[before:]
    assert any("marker=second" in path for path in requests), "did not enumerate all pages"
    assert not any("download_url" in path or path == "/content" for path in requests)


def check_conflicts(binary, origin, root):
    destination = root / "conflicts"
    (destination / "tree").mkdir(parents=True)
    target = destination / "tree/a.txt"
    target.write_bytes(b"keep")
    for extra, action in [([], "overwrite"), (["--no-clobber"], "skip"), (["--overwrite"], "overwrite")]:
        result, envelope = invoke(binary, origin, destination, ["--path", "tree/", "--recursive", "--include-parent", *extra])
        assert result.returncode == 0, envelope
        selection = envelope["data"]["selection"]
        assert selection["PlanComplete"] and selection["Conflicts"] == []
        planned = next(item for item in selection["Manifest"]["Files"] if item["FilePath"] == "tree/a.txt")
        assert planned["Action"] == action
        assert target.read_bytes() == b"keep" and not (destination / "tree/sub").exists()
    target.unlink()
    sentinel = root / "sentinel"
    sentinel.write_bytes(b"outside")
    target.symlink_to(sentinel)
    result, envelope = invoke(binary, origin, destination, ["--path", "tree/", "--recursive", "--include-parent", "--overwrite"])
    assert result.returncode == 9 and envelope["data"]["selection"]["Conflicts"]
    assert target.is_symlink() and sentinel.read_bytes() == b"outside"


def check_validation(binary, origin, root):
    before = len(PlanFixture.requests)
    result, _ = invoke(binary, origin, root / "invalid", ["--path", "../outside"])
    assert result.returncode == 6 and len(PlanFixture.requests) == before
    destination, checkpoint = root / "reserved", root / "reserved" / "a.txt"
    result, envelope = invoke(binary, origin, destination, ["--path", "tree/a.txt", "--checkpoint", str(checkpoint)])
    assert result.returncode == 6, envelope
    assert not destination.exists() and not checkpoint.exists()


def check_native_paths(binary, origin, root):
    selected = ["--path", "tree/a.txt"]
    alias = root / "root-alias"
    alias.symlink_to(root, target_is_directory=True)
    result, envelope = invoke(binary, origin, alias / "new-destination", selected)
    assert result.returncode == 0, envelope
    assert envelope["data"]["selection"]["DestinationDir"] == str(root.resolve() / "new-destination")
    assert not (root / "new-destination").exists()
    dangling = root / "dangling"
    dangling.symlink_to(root / "missing-target", target_is_directory=True)
    before = len(PlanFixture.requests)
    result, envelope = invoke(binary, origin, dangling / "out", selected)
    assert result.returncode == 9, envelope
    assert "local destination" in envelope["error"]["suggested_action"]
    assert len(PlanFixture.requests) == before and not (root / "missing-target").exists()


def check_resume(binary, origin, root):
    destination, checkpoint = root / "resume", root / "job.json"
    PlanFixture.can_download = True
    try:
        result, envelope = invoke(binary, origin, destination,
                                  ["--path", "tree/a.txt", "--checkpoint", str(checkpoint)], False)
        assert result.returncode == 0, envelope
    finally:
        PlanFixture.can_download = False
    saved = checkpoint.read_bytes()
    lock = Path(str(checkpoint) + ".lock")
    lock.unlink()
    result, envelope = invoke(binary, origin, destination,
                              ["--path", "tree/a.txt", "--checkpoint", str(checkpoint), "--resume"])
    assert result.returncode == 0, envelope
    selection = envelope["data"]["selection"]
    assert selection["FileCount"] == 1 and selection["SkippedCount"] == 1
    assert checkpoint.read_bytes() == saved and not lock.exists()
    lock.symlink_to(checkpoint)
    result, envelope = invoke(binary, origin, destination,
                              ["--path", "tree/a.txt", "--checkpoint", str(checkpoint), "--resume"])
    assert result.returncode == 9 and envelope["data"]["selection"]["Conflicts"], envelope
    assert lock.is_symlink() and checkpoint.read_bytes() == saved
    lock.unlink()
    (destination / "a.txt").write_bytes(b"changed local content")
    result, envelope = invoke(binary, origin, destination,
                              ["--path", "tree/a.txt", "--checkpoint", str(checkpoint), "--resume"])
    assert result.returncode == 9 and envelope["data"]["selection"]["Conflicts"]
    assert checkpoint.read_bytes() == saved and not lock.exists()


def check_empty_directory(binary, origin, root):
    destination = root / "empty-out"
    result, envelope = invoke(binary, origin, destination, ["--path", "tree/empty/", "--recursive", "--include-parent"])
    assert result.returncode == 0, envelope
    selection = envelope["data"]["selection"]
    assert selection["FileCount"] == 0 and selection["TotalBytes"] == 0 and selection["DirectoryCount"] == 1
    assert not destination.exists()


def check_unknown_sizes(binary, origin, root):
    PlanFixture.has_size = False
    before = len(PlanFixture.requests)
    try:
        for selected in (["--path", "tree/a.txt"], ["--path", "tree/", "--recursive"]):
            result, envelope = invoke(binary, origin, root / "unknown-size", selected)
            assert result.returncode == 6, envelope
    finally:
        PlanFixture.has_size = True
    assert not (root / "unknown-size").exists()
    assert not any("download_url" in path or path == "/content" for path in PlanFixture.requests[before:])


def main():
    binaries = [Path(value).resolve() for value in sys.argv[1:]] or [Path("target/debug/ve-storage-uni-cli").resolve()]
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), PlanFixture)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        for binary in binaries:
            with tempfile.TemporaryDirectory(prefix="share-plan-") as directory:
                root = Path(directory)
                origin = f"http://127.0.0.1:{server.server_port}"
                check_tree(binary, origin, root)
                check_conflicts(binary, origin, root)
                check_validation(binary, origin, root)
                check_native_paths(binary, origin, root)
                check_resume(binary, origin, root)
                check_empty_directory(binary, origin, root)
                check_unknown_sizes(binary, origin, root)
        print(f"PASS: {len(binaries)} entrypoints; paginated plan, bytes, conflicts, traversal, checkpoint and read-only resume")
    finally:
        server.shutdown()
        server.server_close()


if __name__ == "__main__":
    main()
