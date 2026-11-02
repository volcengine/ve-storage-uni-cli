"""Explicit stdin password validation through real CLI entrypoints and local HTTP.

Run: python3 tests/share_password_regression.py [unified-binary] [standalone-binary]
All credentials and password values are synthetic; no cloud requests are made.
"""
import http.server
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading


class Receiver(http.server.BaseHTTPRequestHandler):
    requests = []

    def log_message(self, *_arguments):
        pass

    def respond(self, payload):
        encoded = json.dumps(payload).encode()
        self.send_response(200)
        self.send_header("Content-Length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)

    def do_GET(self):
        self.requests.append((self.path, None))
        if "get_by_anonymous" in self.path:
            self.respond({"ShareID": "s", "HasPassword": True})
        else:
            self.respond({"Files": [], "Folders": [], "IsTruncated": False})

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        self.requests.append((self.path, body))
        if self.path == "/v1/share_link/create":
            self.respond({"ShareID": "s", "InstanceID": "instance", "UpdatedAt": 7})
        else:
            self.respond({"AccessToken": "fixture-token", "ExpiresIn": 60})

    def do_PUT(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        self.requests.append((self.path, body))
        self.respond({"ShareID": "s", "InstanceID": "instance", "UpdatedAt": 8})


def invoke(binary, root, arguments, password, config_path=None):
    prefix = [] if binary.stem == "ve-adrive-cli" else ["ve-adrive"]
    environment = {key: value for key, value in os.environ.items() if not key.startswith("ADRIVE_")}
    result = subprocess.run(
        [str(binary), *prefix, "--config-path", str(config_path or root / "missing-config"),
         "--credentials-path", str(root / "credentials"), "--output", "json",
         "share", *arguments], input=password, capture_output=True, env=environment, timeout=8)
    for secret in (b"fixture-password", b"url-password", b"fixture-token", b"fixture-ak", b"fixture-sk"):
        assert secret not in result.stdout + result.stderr
    return result, json.loads(result.stdout or result.stderr)


def check_management_dry_run(binary, root):
    actions = [["create", "adrive://instance/space/a.txt"],
               ["update", "s", "--instance", "instance"]]
    for action in actions:
        arguments = [*action, "--password-stdin", "--dry-run"]
        for password in (b"", b"\n", b"\r\n", b"a\nb", b"a\0b", b"\xff", b"a" * 4097):
            result, envelope = invoke(binary, root, arguments, password)
            assert result.returncode == 6 and not envelope["success"], (arguments, result, envelope)
        for password in (b"fixture-password\r\n", b"  fixture-password  ", b" ", b"a" * 4096):
            result, envelope = invoke(binary, root, arguments, password)
            assert result.returncode == 0 and envelope["success"], envelope
        result, envelope = invoke(binary, root, [*action, "--password-stdin", "--describe"], b"a\0b")
        assert result.returncode == 0 and envelope["success"], envelope
    result, envelope = invoke(binary, root, [*actions[0], "--dry-run"], b"")
    assert result.returncode == 0 and envelope["success"]


def check_receiver_empty_stdin(binary, root, origin):
    for password in (b"", b"\n", b"\r\n"):
        Receiver.requests.clear()
        result, envelope = invoke(binary, root,
            ["files", f"{origin}/share/s?pwd=url-password", "--password-stdin"], password)
        assert result.returncode == 6 and not envelope["success"], envelope
        assert all("get_access_token" not in path for path, _body in Receiver.requests)
    Receiver.requests.clear()
    result, envelope = invoke(binary, root,
        ["files", f"{origin}/share/s?pwd=url-password", "--password-stdin"],
        b"  fixture-password  \r\n")
    assert result.returncode == 0 and envelope["success"], envelope
    token_requests = [body for path, body in Receiver.requests if "get_access_token" in path]
    assert token_requests == [{"ShareID": "s", "SharePwd": "  fixture-password  "}]


def check_management_execution(binary, root, origin):
    config_path = root / "management-config"
    config_path.write_text(
        f"[default.adrive]\nendpoint='{origin}'\nregion='cn-beijing'\n"
        "access_key_id='fixture-ak'\nsecret_access_key='fixture-sk'\n"
        "share_endpoint='https://pages.test'\n")
    (root / "credentials").write_text("schema_version=1\n")
    actions = [["create", "adrive://instance/space/a.txt"],
               ["update", "s", "--instance", "instance", "--updated-at", "7"]]
    for action in actions:
        arguments = [*action, "--auth-mode", "aksk", "--password-stdin"]
        for password in (b"", b"\n", b"\r\n"):
            before = len(Receiver.requests)
            result, envelope = invoke(binary, root, arguments, password, config_path)
            assert result.returncode == 6 and not envelope["success"], envelope
            assert len(Receiver.requests) == before
        before = len(Receiver.requests)
        result, envelope = invoke(binary, root, arguments, b"  fixture-password  \r\n", config_path)
        assert result.returncode == 0 and envelope["success"], envelope
        assert len(Receiver.requests) == before + 1
        assert Receiver.requests[-1][1]["SharePwd"] == "  fixture-password  "
        result, envelope = invoke(binary, root,
            [*action, "--auth-mode", "aksk", "--name", "Fixture"], b"", config_path)
        assert result.returncode == 0 and envelope["success"], envelope
        assert "SharePwd" not in Receiver.requests[-1][1]


def main():
    repository = Path(__file__).resolve().parent.parent
    binaries = [Path(value).resolve() for value in sys.argv[1:]] or [
        repository / "target/debug/ve-storage-uni-cli",
        repository / "packaging/cargo/ve-adrive-cli/target/debug/ve-adrive-cli"]
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Receiver)
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    try:
        for binary in binaries:
            with tempfile.TemporaryDirectory(prefix="share-password-") as folder:
                root = Path(folder)
                check_management_dry_run(binary, root)
                check_receiver_empty_stdin(binary, root, f"http://127.0.0.1:{server.server_port}")
                check_management_execution(binary, root, f"http://127.0.0.1:{server.server_port}")
        print(f"PASS: {len(binaries)} entrypoints; explicit empty stdin, bounded dry-run input, spaces, source precedence")
    finally:
        server.shutdown()
        server.server_close()
        worker.join()


if __name__ == "__main__":
    main()
