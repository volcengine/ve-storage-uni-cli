"""Local release archives used by npm packaging integration tests."""

import hashlib
import io
import http.server
import shutil
import ssl
import subprocess
import tarfile
import threading
import zipfile

import pytest


NPM_TARGETS = (
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-unknown-linux-gnu.2.17",
    "aarch64-unknown-linux-gnu.2.17",
    "x86_64-pc-windows-msvc",
)
NPM_COMMANDS = ("ve-tos-cli", "ve-adrive-cli", "tos-cli")
FIXTURE_BINARY = b'#!/bin/sh\nprintf "fixture %s\\n" "$*"\n'


@pytest.fixture
def npm_archives(tmp_path):
    """Return a complete, checksummed release without compiling Rust."""
    archive_dir = tmp_path / "npm-release"
    archive_dir.mkdir()
    checksums = []
    for target in NPM_TARGETS:
        is_windows = "windows" in target
        suffix = ".zip" if is_windows else ".tar.gz"
        archive_path = archive_dir / f"ve-storage-uni-cli-{target}{suffix}"
        if is_windows:
            with zipfile.ZipFile(archive_path, "w", zipfile.ZIP_DEFLATED) as archive:
                for command in NPM_COMMANDS:
                    archive.writestr(f"bin/{command}.exe", FIXTURE_BINARY)
        else:
            with tarfile.open(archive_path, "w:gz") as archive:
                for command in NPM_COMMANDS:
                    member = tarfile.TarInfo(f"bin/{command}")
                    member.size = len(FIXTURE_BINARY)
                    member.mode = 0o755
                    archive.addfile(member, io.BytesIO(FIXTURE_BINARY))
        checksum = hashlib.sha256(archive_path.read_bytes()).hexdigest()
        checksums.append(f"{checksum}  {archive_path.name}\n")
    (archive_dir / "SHA256SUMS").write_text("".join(checksums), encoding="utf-8")
    return archive_dir


@pytest.fixture
def npm_https_release(npm_archives, tmp_path):
    """Serve a release over trusted local TLS, including a relative redirect."""
    if shutil.which("openssl") is None:
        pytest.skip("openssl is unavailable")
    certificate = tmp_path / "certificate.pem"
    private_key = tmp_path / "private-key.pem"
    subprocess.run([
        "openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
        "-subj", "/CN=127.0.0.1", "-addext", "subjectAltName=IP:127.0.0.1",
        "-keyout", str(private_key), "-out", str(certificate),
    ], capture_output=True, check=True, timeout=20)
    requests = []

    class ReleaseHandler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            requests.append(self.path)
            filename = self.path.rsplit("/", 1)[-1]
            if self.path.startswith("/repo/releases/download/v1.0.5/"):
                self.send_response(302)
                self.send_header("Location", f"/assets/{filename}")
                self.end_headers()
            elif self.path.startswith("/assets/") and (npm_archives / filename).is_file():
                payload = (npm_archives / filename).read_bytes()
                self.send_response(200)
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)
            else:
                self.send_response(404)
                self.end_headers()

        def log_message(self, *_arguments):
            return

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), ReleaseHandler)
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(certificate, private_key)
    server.socket = context.wrap_socket(server.socket, server_side=True)
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    try:
        yield f"https://127.0.0.1:{server.server_port}/repo", certificate, requests
    finally:
        server.shutdown()
        server.server_close()
        worker.join(timeout=5)
