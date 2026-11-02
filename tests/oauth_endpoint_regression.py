"""OAuth same-origin login/resource/refresh and endpoint fallback; localhost only."""
import http.server
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import threading
import tomllib
from urllib.parse import parse_qs


class OAuthService(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_arguments):
        pass

    def respond(self, value):
        encoded = json.dumps(value).encode()
        self.send_response(200)
        self.send_header("Content-Length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)

    def do_POST(self):
        fields = parse_qs(self.rfile.read(int(self.headers.get("Content-Length", "0"))).decode())
        self.server.events.append((self.path, fields.get("grant_type", [""])[0]))
        if self.path == "/v1/oauth/device_authorization":
            self.respond({"device_code": "fixture-device-secret", "user_code": "TEST-CODE",
                "verification_uri": "http://login.example.test/authorize",
                "verification_uri_complete": "http://login.example.test/authorize?user_code=TEST-CODE",
                "expires_in": 60, "interval": 1})
        else:
            assert self.path == "/v1/oauth/token", self.path
            self.respond({"access_token": "fixture-access-secret", "refresh_token": "fixture-refresh-secret",
                "token_type": "Bearer", "expires_in": 3600, "scope": "all", "instance_id": "inst"})

    def do_GET(self):
        assert self.headers.get("Authorization") == "Bearer fixture-access-secret"
        self.server.events.append((self.path, "resource"))
        self.respond({"ShareID": "s", "RequireLogin": False, "Spaces": []})


def start_service():
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), OAuthService)
    server.origin = f"http://127.0.0.1:{server.server_port}"
    server.events = []
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def configure(root, endpoint=None, auth_endpoint=None):
    contents = "[selected.adrive]\nauth_mode='oauth'\nregion='cn-beijing'\nmax_retry_count=0\n"
    if endpoint is not None:
        contents += f"endpoint='{endpoint}'\n"
    if auth_endpoint is not None:
        contents += f"auth_endpoint='{auth_endpoint}'\n"
    (root / "config").write_text(contents)


def invoke(binary, root, arguments, environment=None):
    prefix = [] if binary.stem == "ve-adrive-cli" else ["ve-adrive"]
    process_env = {key: value for key, value in os.environ.items()
                   if not key.startswith(("ADRIVE_", "TOS_"))}
    process_env.update(environment or {})
    result = subprocess.run([str(binary), *prefix, "--config-path", str(root / "config"),
        "--credentials-path", str(root / "credentials"), "--profile", "selected",
        "--output", "json", *arguments], env=process_env, text=True, capture_output=True, timeout=15)
    for secret in ("fixture-access-secret", "fixture-refresh-secret", "fixture-device-secret"):
        assert secret not in result.stdout + result.stderr
    return result


def login(binary, root, arguments=(), environment=None):
    result = invoke(binary, root, ["auth", "login", "--instance", "inst", *arguments], environment)
    assert result.returncode == 0, result.stderr
    with (root / "credentials").open("rb") as source:
        return tomllib.load(source)["selected"]["adrive"]["oauth"]["auth_endpoint"].rstrip("/")


def check_fallback(binary, root, primary, other):
    configure(root, primary.origin)
    assert login(binary, root) == primary.origin
    result = invoke(binary, root, ["ls", "adrive://inst"])
    assert result.returncode == 0, result.stderr
    result = invoke(binary, root, ["share", "inspect", primary.origin + "/share/s", "--auth-mode", "oauth"])
    assert result.returncode == 0, result.stderr
    before = len(primary.events)
    credentials = root / "credentials"
    credentials.write_text(re.sub(r'expires_at\s*=\s*"[^"]+"',
        'expires_at = "2000-01-01T00:00:00Z"', credentials.read_text()))
    configure(root, other.origin)
    result = invoke(binary, root, ["share", "inspect", primary.origin + "/share/s", "--auth-mode", "oauth"])
    assert result.returncode == 0, result.stderr
    assert ("/v1/oauth/token", "refresh_token") in primary.events[before:]
    assert any(event[1] == "resource" for event in other.events)


def check_priority(binary, root, primary, other):
    configure(root, primary.origin, primary.origin)
    assert login(binary, root, ["--auth-endpoint", other.origin],
                 {"ADRIVE_AUTH_ENDPOINT": primary.origin}) == other.origin
    configure(root, primary.origin, other.origin)
    assert login(binary, root, environment={"ADRIVE_AUTH_ENDPOINT": primary.origin}) == other.origin
    configure(root, primary.origin)
    assert login(binary, root, environment={"ADRIVE_AUTH_ENDPOINT": other.origin}) == other.origin
    assert login(binary, root, ["--endpoint", other.origin]) == other.origin
    configure(root)
    assert login(binary, root, environment={"ADRIVE_ENDPOINT": primary.origin}) == primary.origin
    configure(root, primary.origin, " ")
    assert login(binary, root, ["--auth-endpoint", " "],
                 {"ADRIVE_AUTH_ENDPOINT": " "}) == primary.origin


def check_plans_and_doctor(binary, root, primary):
    configure(root, primary.origin)
    credential_snapshot = (root / "credentials").read_bytes()
    # [Review Fix #OAuthEndpoint5] A ready token would hide a broken Doctor fallback check.
    (root / "credentials").write_text("schema_version=1\n")
    result = invoke(binary, root, ["doctor", "--check", "auth", "--auth-mode", "oauth"])
    assert result.returncode == 0, result.stderr
    check = json.loads(result.stdout)["data"]["checks"][0]
    assert check["details"]["has_login_auth_endpoint"] is True
    assert check["details"]["ready"] is False and check["status"] == "warning"
    assert check["fix_command"] == "ve-adrive auth login"
    (root / "credentials").write_bytes(credential_snapshot)
    event_count = len(primary.events)
    result = invoke(binary, root, ["auth", "login", "--instance", "inst", "--endpoint",
                                 "http://public-auth.example.test:9080", "--dry-run"])
    assert result.returncode == 0, result.stderr
    assert "http://public-auth.example.test:9080" in result.stdout
    assert (root / "credentials").read_bytes() == credential_snapshot
    assert len(primary.events) == event_count
    configure(root)
    result = invoke(binary, root, ["auth", "login", "--instance", "inst", "--dry-run"])
    assert result.returncode != 0 and "oauth_auth_endpoint_required" in result.stderr
    for language in ("en", "zh"):
        result = invoke(binary, root, ["auth", "login", "--help", "--help-language", language])
        expected = "defaults to the effective endpoint" if language == "en" else "默认使用有效 endpoint"
        assert result.returncode == 0 and expected in result.stdout


def main():
    binaries = [Path(value).resolve() for value in sys.argv[1:]]
    assert binaries, "pass unified and standalone binary paths"
    primary, other = start_service(), start_service()
    try:
        for binary in binaries:
            with tempfile.TemporaryDirectory(prefix="oauth-endpoint-") as directory:
                root = Path(directory)
                check_fallback(binary, root, primary, other)
                check_priority(binary, root, primary, other)
                check_plans_and_doctor(binary, root, primary)
        print(f"PASS: {len(binaries)} entrypoints; same-origin OAuth, fallback priorities, HTTP, refresh pinning and doctor")
    finally:
        for server in (primary, other):
            server.shutdown()
            server.server_close()


if __name__ == "__main__":
    main()
