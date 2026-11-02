"""Exercise real terminal echo and SIGINT behavior against an isolated HTTP fixture."""
import fcntl
import faulthandler
import http.server
import json
import os
from pathlib import Path
import pty
import select
import signal
import subprocess
import sys
import tempfile
import termios
import threading
import time


def entrypoint_command(binary, arguments):
    # [Review Fix #ShareEndpointPty] Dedicated binaries need no unified command prefix.
    prefix = [] if Path(binary).stem == "ve-adrive-cli" else ["ve-adrive"]
    return [binary, *prefix, *arguments]


def wait_until(predicate, process, label):
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        if predicate():
            return
        if process.poll() is not None:
            error = process.stderr.read() if process.stderr else b""
            raise AssertionError(f"child exited while waiting for {label}: {error!r}")
        time.sleep(0.01)
    raise AssertionError(f"timed out waiting for {label}")


class ShareHandler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def reply(self, payload):
        encoded = json.dumps(payload).encode()
        self.send_response(200)
        self.send_header("Content-Length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)

    def do_GET(self):
        self.server.requests.append(self.path)
        if self.path.startswith("/v1/share_link/get_by_anonymous"):
            self.reply({"ShareID": "s", "HasPassword": True})
        else:
            self.server.is_reading_file.set()
            self.server.can_finish.wait(10)
            try:
                self.reply({"FilePath": "a.txt"})
            except (BrokenPipeError, ConnectionResetError):
                # Cancellation deliberately drops this metadata request.
                return

    def do_POST(self):
        self.server.requests.append(self.path)
        self.rfile.read(int(self.headers.get("Content-Length", "0")))
        if self.path == "/v1/oauth/token":
            self.server.is_refreshing.set()
            self.server.can_finish.wait(10)
            try:
                self.reply({"access_token":"fresh","refresh_token":"next","token_type":"Bearer","expires_in":3600})
            except (BrokenPipeError, ConnectionResetError):
                return
        elif self.path == "/v1/share_link/cancel":
            self.send_response(204)
            self.end_headers()
        else:
            self.reply({"AccessToken": "fixture-token", "ExpiresIn": 60})

    def do_PUT(self):
        self.server.requests.append(self.path)
        self.rfile.read(int(self.headers.get("Content-Length", "0")))
        self.server.is_submitting.set()
        self.server.can_finish.wait(10)
        try:
            self.reply({"ShareID": "requested"})
        except (BrokenPipeError, ConnectionResetError):
            return


def has_echo(master):
    return bool(termios.tcgetattr(master)[3] & termios.ECHO)


def wait_for_exit(process, master, captured):
    deadline = time.monotonic() + 4
    while time.monotonic() < deadline:
        if select.select([master], [], [], 0.01)[0]:
            chunk = os.read(master, 65536)
            if chunk:
                captured.extend(chunk)
        code = process.poll()
        if code is not None:
            return code
    raise AssertionError(f"SIGINT did not exit; echo={has_echo(master)}, output={captured!r}")


def start_fixture():
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), ShareHandler)
    server.daemon_threads = True
    server.is_reading_file = threading.Event()
    server.is_submitting = threading.Event()
    server.is_refreshing = threading.Event()
    server.can_finish = threading.Event()
    server.requests = []
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def stop_fixture(server):
    server.can_finish.set()
    server.shutdown()
    server.server_close()


def spawn_terminal_child(binary, slave, origin):
    def prepare_terminal():
        os.setsid()
        fcntl.ioctl(slave, termios.TIOCSCTTY, 0)

    return subprocess.Popen(
        entrypoint_command(binary, ["share", "files", f"{origin}/share/s", "--path", "a.txt", "--output", "table"]),
        stdin=slave, stdout=slave, stderr=slave, preexec_fn=prepare_terminal,
    )


def run_case(binary, is_after_password):
    server = start_fixture()
    master, slave = pty.openpty()
    process = None
    captured = bytearray()
    try:
        assert has_echo(master), "new PTY must start with ECHO enabled"

        origin = f"http://127.0.0.1:{server.server_address[1]}"
        process = spawn_terminal_child(binary, slave, origin)

        def is_prompting():
            if select.select([master], [], [], 0)[0]:
                captured.extend(os.read(master, 65536))
            return b"Share password:" in captured and not has_echo(master)

        wait_until(is_prompting, process, "hidden password prompt")
        assert not has_echo(master), "ECHO must be disabled only during password input"
        if is_after_password:
            os.write(master, b"fixture-hidden-password\n")
            wait_until(server.is_reading_file.is_set, process, "metadata request after password")
            assert has_echo(master), "ECHO must restore after password input"
        os.kill(process.pid, signal.SIGINT)
        code = wait_for_exit(process, master, captured)
        assert has_echo(master), f"ECHO not restored after SIGINT (exit={code})"
        assert code > 0, f"interrupt must return through the CLI error path, got {code}"
        while select.select([master], [], [], 0)[0]:
            chunk = os.read(master, 65536)
            if not chunk:
                break
            captured.extend(chunk)
        assert b"fixture-hidden-password" not in captured, "password was echoed"
    finally:
        if process is not None and process.poll() is None:
            process.kill()
            process.wait(timeout=2)
        stop_fixture(server)
        os.close(master)
        os.close(slave)


def run_management_cases(binary):
    server = start_fixture()
    process = None
    try:
        with tempfile.TemporaryDirectory(prefix="share-review-") as directory:
            config_path = os.path.join(directory, "config.toml")
            endpoint = f"http://127.0.0.1:{server.server_address[1]}"
            with open(config_path, "w") as config:
                config.write(f"[default.adrive]\nendpoint='{endpoint}'\nregion='cn-beijing'\naccess_key_id='fixture-ak'\nsecret_access_key='fixture-sk'\nshare_endpoint='https://user:unused-secret@bad.test'\n")
            common = ["--config-path", config_path, "--auth-mode", "aksk", "--output", "json"]
            process = subprocess.Popen(entrypoint_command(binary, ["share", "update", "requested", "--instance", "inst", "--updated-at", "7", "--name", "New", "--share-endpoint", "https://pages.test", *common]), stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            wait_until(server.is_submitting.is_set, process, "submitted update")
            os.kill(process.pid, signal.SIGINT)
            _, errors = process.communicate(timeout=4)
            envelope = json.loads(errors)
            assert process.returncode == 1
            assert envelope["error"]["code"] == "share_result_unknown"
            assert "requested" in envelope["error"]["message"]
            assert "reconcile" in envelope["error"]["message"]
            assert server.requests == ["/v1/share_link/update"]
            server.can_finish.set()
            environment = dict(os.environ, ADRIVE_SHARE_ENDPOINT="https://user:env-secret@bad.test")
            cancel = subprocess.run(entrypoint_command(binary, ["share", "cancel", "s", "--instance", "instance_with_underscore", "--yes", *common]), capture_output=True, env=environment, timeout=4)
            assert cancel.returncode == 0, cancel.stderr
            assert json.loads(cancel.stdout)["data"]["Cancelled"] is True
            assert server.requests == ["/v1/share_link/update", "/v1/share_link/cancel"]
    finally:
        if process is not None and process.poll() is None:
            process.kill()
            process.wait(timeout=2)
        stop_fixture(server)


def run_oauth_refresh_interrupt(binary):
    server = start_fixture()
    resource_server = start_fixture()
    process = None
    try:
        with tempfile.TemporaryDirectory(prefix="share-oauth-review-") as directory:
            endpoint = f"http://127.0.0.1:{resource_server.server_address[1]}"
            auth_endpoint = f"http://127.0.0.1:{server.server_address[1]}"
            config_path = os.path.join(directory, "config.toml")
            credentials_path = os.path.join(directory, "credentials.toml")
            with open(config_path, "w") as config:
                config.write(f"[default.adrive]\nendpoint='{endpoint}'\nregion='cn-beijing'\n")
            with open(credentials_path, "w") as credentials:
                credentials.write(f"schema_version=1\n[default.adrive.oauth]\naccess_token='expired-fixture-token'\nrefresh_token='fixture-refresh-token'\nexpires_at='2000-01-01T00:00:00Z'\ninstance_id='inst'\nauth_endpoint='{auth_endpoint}'\n")
            arguments = entrypoint_command(binary, ["share","update","requested","--instance","inst","--updated-at","7","--name","New","--share-endpoint","https://pages.test","--config-path",config_path,"--credentials-path",credentials_path,"--auth-mode","oauth","--output","json"])
            process = subprocess.Popen(arguments, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            wait_until(server.is_refreshing.is_set, process, "OAuth refresh before resource submission")
            os.kill(process.pid, signal.SIGINT)
            _, errors = process.communicate(timeout=4)
            envelope = json.loads(errors)
            assert process.returncode == 1
            assert envelope["error"]["code"] == "share_interrupted", envelope
            assert "after submission" not in envelope["error"]["message"]
            assert server.requests == ["/v1/oauth/token"]
            assert resource_server.requests == []
    finally:
        if process is not None and process.poll() is None:
            process.kill()
            process.wait(timeout=2)
        stop_fixture(server)
        stop_fixture(resource_server)


if __name__ == "__main__":
    faulthandler.dump_traceback_later(15, exit=True)
    for is_after_password in (False, True):
        run_case(sys.argv[1], is_after_password)
    run_management_cases(sys.argv[1])
    run_oauth_refresh_interrupt(sys.argv[1])
