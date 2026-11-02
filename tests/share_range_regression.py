"""Concurrent share byte ranges against both real CLI entrypoints; localhost only."""
import http.server
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import tempfile
import threading
import time
from urllib.parse import urlsplit

PART = 16 * 1024 * 1024
PAYLOAD = b"0123456789abcdef" * (2 * PART // 16) + b"last-part"


from share_download_regression import transfer_paths


class RangeFixture(http.server.BaseHTTPRequestHandler):
    mode = "normal"
    payload = PAYLOAD
    ranges = []
    issued = 0
    active = 0
    peak = 0
    lock = threading.Lock()
    started = threading.Event()
    release = threading.Event()

    def log_message(self, *_args):
        pass

    def reply(self, body, status=200, headers=None):
        if isinstance(body, dict):
            body = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Length", str(len(body)))
        for key, value in (headers or {}).items():
            self.send_header(key, str(value))
        self.end_headers()
        try:
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def do_POST(self):
        self.rfile.read(int(self.headers.get("Content-Length", "0")))
        self.reply({"AccessToken": "range-secret", "ExpiresIn": 600})

    def do_GET(self):
        path = urlsplit(self.path).path
        if path == "/v1/share_link/get_by_anonymous":
            self.reply({"ShareID": "s", "Permission": "download"})
        elif path in ("/v1/share_file/get", "/v1/share_file/list"):
            metadata = {"FilePath": "large.bin", "Size": len(self.payload), "ETag": "snapshot"}
            if self.mode == "no-identity":
                metadata.pop("ETag")
            if self.mode == "crc":
                metadata["HashCRC64ECMA"] = 123
            self.reply(metadata if path.endswith("/get") else {
                "Files": [metadata], "Folders": [], "IsTruncated": False})
        elif path == "/v1/share_file/get_download_url":
            type(self).issued += 1
            self.reply({"DownloadURL": f"http://127.0.0.1:{self.server.server_port}/content?signed=secret"})
        elif path == "/content":
            self.content()
        else:
            self.reply({}, 404)

    def content(self):
        assert not self.headers.get("Authorization")
        assert not self.headers.get("x-share-token")
        assert self.headers.get("If-Match") == (None if self.mode == "no-identity" else '"snapshot"')
        if self.mode == "cancel":
            self.wait_for_cancellation()
            return
        span = self.headers.get("Range")
        type(self).ranges.append(span)
        if not span:
            self.reply(self.payload, headers={"ETag": "snapshot"})
            return
        match = re.fullmatch(r"bytes=(\d+)-(\d+)", span)
        assert match
        start, end = map(int, match.groups())
        assert 0 <= start <= end < len(self.payload)
        if self.mode == "retry" and start == PART and self.ranges.count(span) == 1:
            self.reply({}, 503)
            return
        if self.mode == "interrupted" and start == PART and self.ranges.count(span) == 1:
            self.send_response(206)
            self.send_header("Content-Length", str(end - start + 1))
            self.send_header("ETag", "snapshot")
            self.send_header("Content-Range", f"bytes {start}-{end}/{len(self.payload)}")
            self.end_headers()
            self.wfile.write(self.payload[start:start + 4096])
            self.wfile.flush()
            self.close_connection = True
            return
        with self.lock:
            type(self).active += 1
            type(self).peak = max(self.peak, self.active)
        time.sleep(0.1)
        try:
            content_range = f"bytes {start}-{end}/{len(self.payload)}"
            if self.mode == "bad-range":
                content_range = f"bytes {start + 1}-{end}/{len(self.payload)}"
            body = self.payload[start:end + 1]
            if self.mode == "short":
                body = body[:-1]
            self.reply(body, 200 if self.mode == "ignored" else 206,
                       {"ETag": "wrong" if self.mode == "etag" else "snapshot", "Content-Range": content_range})
        finally:
            with self.lock:
                type(self).active -= 1

    def wait_for_cancellation(self):
        # [Review Fix #ShareRange2] Drain blocked handlers before another binary resets fixture state.
        with self.lock:
            type(self).active += 1
        self.started.set()
        try:
            self.release.wait(10)
            self.reply(b"", 503)
        finally:
            with self.lock:
                type(self).active -= 1


def invoke(binary, root, origin, mode, extra=()):
    RangeFixture.mode = mode
    RangeFixture.ranges = []
    RangeFixture.issued = RangeFixture.peak = 0
    destination = root / mode
    prefix = [] if binary.name == "ve-adrive-cli" else ["ve-adrive"]
    environment = {key: value for key, value in os.environ.items() if not key.startswith("ADRIVE_")}
    progress_flags = [] if "--progress" in extra else ["--no-progress"]
    result = subprocess.run([str(binary), *prefix, "share", "download", f"{origin}/share/s",
        str(destination), "--output", "json", *progress_flags, *transfer_paths(root), *extra],
        capture_output=True, text=True, env=environment, timeout=40)
    for _ in range(100):
        if RangeFixture.active == 0:
            break
        time.sleep(0.02)
    assert "range-secret" not in result.stdout + result.stderr
    assert "signed=secret" not in result.stdout + result.stderr
    return result, destination


def check_binary(binary, origin):
    with tempfile.TemporaryDirectory(prefix="share-range-") as name:
        root = Path(name)
        result, destination = invoke(binary, root, origin, "normal")
        assert result.returncode == 0, result.stdout + result.stderr
        assert all(RangeFixture.ranges) and len(RangeFixture.ranges) == 3, RangeFixture.ranges
        assert RangeFixture.peak >= 2, "ranges did not overlap"
        assert RangeFixture.issued == 1
        assert (destination / "large.bin").read_bytes() == PAYLOAD
        result, destination = invoke(binary, root, origin, "retry", ["--multipart-concurrency", "2"])
        assert result.returncode == 0, result.stdout + result.stderr
        assert len(RangeFixture.ranges) == 4 and RangeFixture.issued == 1
        assert 1 < RangeFixture.peak <= 2
        assert (destination / "large.bin").read_bytes() == PAYLOAD
        result, destination = invoke(binary, root, origin, "interrupted")
        assert result.returncode == 0 and len(RangeFixture.ranges) == 4
        assert RangeFixture.issued == 1 and (destination / "large.bin").read_bytes() == PAYLOAD
        for mode in ("ignored", "bad-range", "etag", "short", "crc"):
            result, destination = invoke(binary, root, origin, mode)
            assert result.returncode != 0, mode
            assert not (destination / "large.bin").exists(), mode
            assert not list(destination.glob(".adrive-share-*")), mode
            assert RangeFixture.issued == 1
        result, destination = invoke(binary, root, origin, "serial", ["--multipart-concurrency", "1"])
        assert result.returncode == 0 and RangeFixture.peak == 1
        result, _ = invoke(binary, root, origin, "invalid", ["--multipart-concurrency", "0"])
        assert result.returncode != 0 and RangeFixture.issued == 0
        result, destination = invoke(binary, root, origin, "threshold", ["--checkpoint-threshold", "64MB"])
        assert result.returncode == 0 and RangeFixture.ranges == [None]
        assert (destination / "large.bin").read_bytes() == PAYLOAD
        result, _ = invoke(binary, root, origin, "progress", ["--progress"])
        assert result.returncode == 0 and "[ranges 3/3]" in result.stderr
        assert "[ranges" not in result.stdout
        result, _ = invoke(binary, root, origin, "quiet", ["--progress", "--quiet"])
        assert result.returncode == 0 and "[ranges" not in result.stderr
        result, destination = invoke(binary, root, origin, "no-identity")
        assert result.returncode == 0 and RangeFixture.ranges == [None], RangeFixture.ranges
        assert (destination / "large.bin").read_bytes() == PAYLOAD
        check_default_threshold(binary, root, origin)
        check_cancellation(binary, root, origin)


def check_default_threshold(binary, root, origin):
    original = RangeFixture.payload
    try:
        for size, mode in [(20 * 1024 * 1024 - 1, "below-threshold"),
                           (20 * 1024 * 1024, "at-threshold")]:
            RangeFixture.payload = PAYLOAD[:size]
            result, destination = invoke(binary, root, origin, mode)
            assert result.returncode == 0, result.stdout + result.stderr
            if mode == "below-threshold":
                assert RangeFixture.ranges == [None]
            else:
                assert all(RangeFixture.ranges) and len(RangeFixture.ranges) == 2
            assert (destination / "large.bin").read_bytes() == RangeFixture.payload
    finally:
        RangeFixture.payload = original


def check_cancellation(binary, root, origin):
    RangeFixture.mode = "cancel"
    RangeFixture.started.clear()
    RangeFixture.release.clear()
    destination = root / "cancel"
    prefix = [] if binary.name == "ve-adrive-cli" else ["ve-adrive"]
    process = subprocess.Popen([str(binary), *prefix, "share", "download", f"{origin}/share/s",
        str(destination), "--output", "json", "--no-progress", "--checkpoint", str(root / "recovery.json"),
        *transfer_paths(root)],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        env={key: value for key, value in os.environ.items() if not key.startswith("ADRIVE_")})
    try:
        assert RangeFixture.started.wait(5), "range content did not begin"
        process.send_signal(signal.SIGINT)
        stdout, stderr = process.communicate(timeout=5)
        assert process.returncode != 0 and json.loads(stdout)["data"]["CheckpointAvailable"]
        assert not (destination / "large.bin").exists()
        assert not list(destination.glob(".adrive-share-*"))
        assert "range-secret" not in stdout + stderr
    finally:
        RangeFixture.release.set()
        for _ in range(100):
            if RangeFixture.active == 0:
                break
            time.sleep(0.02)
        assert RangeFixture.active == 0, "cancelled fixture handlers did not drain"
        if process.poll() is None:
            process.kill()
            process.communicate()


def main():
    binaries = [Path(value).resolve() for value in sys.argv[1:]] or [Path("target/debug/ve-storage-uni-cli").resolve()]
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), RangeFixture)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        for binary in binaries:
            check_binary(binary, f"http://127.0.0.1:{server.server_port}")
        print(f"share range regressions passed ({len(binaries)} entrypoints)")
    finally:
        server.shutdown()
        server.server_close()


if __name__ == "__main__":
    main()
