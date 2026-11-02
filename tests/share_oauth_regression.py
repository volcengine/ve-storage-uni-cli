"""Compiled-entrypoint OAuth share receipt regressions using isolated local fixtures.

Run: python3 tests/share_oauth_regression.py [unified-binary] [standalone-binary]
All credentials below are synthetic test values; no user profile or cloud service is used.
"""
import datetime
import http.server
import hashlib
import shutil
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
from urllib.parse import parse_qs, urlsplit


from share_download_regression import transfer_paths


class Receipt(http.server.BaseHTTPRequestHandler):
    requests = []
    is_rejecting_old = False
    echo_id = "receipt-request"
    exchange_hook = None
    info_hook = None
    is_login_required = True

    def log_message(self, *_arguments):
        pass

    def record(self, body=""):
        self.requests.append((self.path, {name.lower(): value for name,value in self.headers.items()}, body))

    def respond(self, value, status=200):
        encoded = json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Length", str(len(encoded)))
        self.send_header("x-ids-request-id", self.echo_id)
        self.end_headers()
        self.wfile.write(encoded)

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", "0"))).decode()
        self.record(body)
        if self.path == "/v1/oauth/token":
            assert parse_qs(body)["refresh_token"] == ["fixture-refresh"]
            self.respond({"access_token": "fixture-new-account", "refresh_token": "fixture-new-refresh",
                          "token_type": "Bearer", "expires_in": 3600, "instance_id": "inst",
                          "user_id": "refreshed-user", "scope": "file:read file:list"})
            return
        assert self.path == "/v1/share_link/get_access_token"
        if self.is_login_required:
            assert self.headers.get("Authorization", "").startswith("Bearer fixture-")
        if json.loads(body).get("SharePwd") != "fixture-password":
            self.respond({"Code": "Forbidden.SharePassword"}, 403)
            return
        if type(self).exchange_hook:
            type(self).exchange_hook()
        self.respond({"AccessToken": "fixture-share-token", "ExpiresIn": 60})

    def do_GET(self):
        self.record()
        path = urlsplit(self.path).path
        if path == "/v1/share_link/get_by_anonymous":
            if self.is_rejecting_old and self.headers.get("authorization") == "Bearer fixture-account":
                type(self).is_rejecting_old = False
                self.respond({"Code": "Unauthorized"}, 401)
                return
            if type(self).info_hook:
                type(self).info_hook()
            self.respond({"ShareID": "s", "RequireLogin": self.is_login_required, "HasPassword": True})
            return
        if path == "/content":
            assert not self.headers.get("authorization") and not self.headers.get("x-share-token")
            self.send_response(200)
            self.send_header("Content-Length", "3")
            self.send_header("ETag", "abc")
            self.end_headers()
            self.wfile.write(b"abc")
            return
        assert not self.headers.get("authorization")
        assert self.headers.get("x-share-token") == "fixture-share-token"
        self.respond_file_api(path)

    def respond_file_api(self, path):
        query = parse_qs(urlsplit(self.path).query)
        if path == "/v1/share_file/list":
            has_next_page = not query.get("marker")
            self.respond({"Files": [{"FilePath": "docs/a.txt", "Size": 3}], "Folders": [],
                          "NextMarker": "second-page" if has_next_page else "",
                          "IsTruncated": has_next_page})
        elif path == "/v1/share_file/get":
            self.respond({"FilePath": query["file_path"][0], "Size": 3, "ETag": "abc"})
        elif path == "/v1/share_file/get_download_url":
            self.respond({"DownloadURL": f"http://127.0.0.1:{self.server.server_port}/content?signature=fixture-signature"})
        else:
            self.respond({"Code": "UnexpectedRoute"}, 404)


def seed_profile(root, endpoint, auth_endpoint):
    expiry = (datetime.datetime.now(datetime.timezone.utc) + datetime.timedelta(hours=1)).isoformat()
    (root / "config").write_text(
        "".join(f"[{profile}.adrive]\nauth_mode='oauth'\nendpoint='{endpoint}'\nregion='cn-beijing'\nmax_retry_count=0\n"
                for profile in ("receiver", "other")))
    (root / "credentials").write_text("schema_version=1\n" + "".join(
        f"[{profile}.adrive.oauth]\naccess_token='fixture-account'\nrefresh_token='fixture-refresh'\n"
        f"expires_at='{expiry}'\ninstance_id='inst'\nuser_id='{profile}-user'\nauth_endpoint='{auth_endpoint}'\n"
        for profile in ("receiver", "other")))


def invoke(binary, prefix, root, arguments, password=None):
    environment = {key: value for key, value in os.environ.items() if not key.startswith("ADRIVE_")}
    # A configured/environment default must not silently opt a receiver into OAuth.
    environment["ADRIVE_AUTH_MODE"] = "oauth"
    profile = [] if "--profile" in arguments else ["--profile", "receiver"]
    artifacts = transfer_paths(root, False) if arguments[:2] == ["share", "download"] else []
    result = subprocess.run(
        [str(binary), *prefix, "--config-path", str(root / "config"), "--credentials-path",
         str(root / "credentials"), *profile, "--output", "json", *arguments, *artifacts],
        input=password, text=True, capture_output=True, timeout=15, env=environment)
    for secret in ("fixture-account", "fixture-new-account", "fixture-password", "fixture-share-token", "fixture-signature"):
        assert secret not in result.stdout + result.stderr, "secret leaked from receipt"
    return result, json.loads(result.stdout or result.stderr)


def signed_count():
    return sum("get_download_url" in request[0] for request in Receipt.requests)


def check_modes(binary, prefix, root, url, endpoint):
    start = len(Receipt.requests)
    result, envelope = invoke(binary, prefix, root, ["share", "files", url, "--endpoint", endpoint])
    assert result.returncode == 5 and envelope["ec"] == "share_login_required"
    assert len(Receipt.requests) == start + 1
    assert not Receipt.requests[-1][1].get("authorization")
    result, _envelope = invoke(binary, prefix, root, ["share", "inspect", url, "--auth-mode", "oauth"])
    assert result.returncode == 0
    assert Receipt.requests[-1][1].get("authorization") == "Bearer fixture-account"
    start = len(Receipt.requests)
    wrong = url.replace("pages.test", "other.idsapp-boe.volces.com")
    result, _envelope = invoke(binary, prefix, root, ["share", "inspect", wrong, "--auth-mode", "oauth"])
    assert result.returncode != 0 and len(Receipt.requests) == start


def check_pages(binary, prefix, root, url):
    arguments = ["share", "files", url, "--auth-mode", "oauth", "--password-stdin", "--path", "docs/", "--max-keys", "1"]
    for marker in (None, "second-page"):
        result, envelope = invoke(binary, prefix, root, arguments + (["--marker", marker] if marker else []), "fixture-password\n")
        assert result.returncode == 0
        assert envelope["data"]["IsTruncated"] is (marker is None)
        if marker is None:
            assert envelope["pagination"]["next_marker"] == "second-page"
        query = parse_qs(urlsplit(Receipt.requests[-1][0]).query)
        assert query["limit"] == ["1"]
        assert query.get("marker") == ([marker] if marker else None)
    start = len(Receipt.requests)
    result, envelope = invoke(binary, prefix, root, arguments, "wrong-password\n")
    assert result.returncode == 5 and envelope["ec"] == "Forbidden.SharePassword"
    assert len(Receipt.requests) == start + 2


def check_download(binary, prefix, root, url):
    arguments = ["share", "download", url, str(root / "out"), "--auth-mode", "oauth", "--password-stdin",
                 "--path", "a.txt", "--checkpoint", str(root / "journal")]
    before = signed_count()
    for resume in ([], ["--resume"]):
        result, envelope = invoke(binary, prefix, root, arguments + resume, "fixture-password\n")
        assert result.returncode == 0 and envelope["data"]["Complete"]
        assert envelope["data"]["Summary"]["Completed"] == 1
        assert (root / "out/a.txt").read_bytes() == b"abc"
    assert signed_count() == before + 1
    journal = (root / "journal").read_text()
    for secret in ("fixture-account", "fixture-password", "fixture-share-token", "fixture-signature"):
        assert secret not in journal
    assert json.loads(journal)["identity"]["recipient"]["profile"] == "receiver"
    result, envelope = invoke(binary, prefix, root, arguments + ["--resume", "--profile", "other"], "fixture-password\n")
    assert result.returncode != 0 and not envelope["success"]
    assert signed_count() == before + 1


def check_read_only_plan(binary, prefix, root, url):
    arguments = ["share", "download", url, str(root / "planned-out"), "--auth-mode", "oauth",
                 "--password-stdin", "--path", "a.txt", "--dry-run"]
    saved = (root / "credentials").read_bytes()
    entries = set(root.iterdir())
    before = signed_count()
    result, envelope = invoke(binary, prefix, root, arguments, "fixture-password\n")
    assert result.returncode == 0 and envelope["data"]["selection"]["FileCount"] == 1, envelope
    assert signed_count() == before and set(root.iterdir()) == entries
    assert (root / "credentials").read_bytes() == saved
    expired = saved.decode().replace("expires_at='", "expires_at='2000-01-01T00:00:00Z' # ")
    (root / "credentials").write_text(expired)
    start = len(Receipt.requests)
    try:
        result, envelope = invoke(binary, prefix, root, arguments, "fixture-password\n")
        assert result.returncode != 0 and len(Receipt.requests) == start, envelope
        assert (root / "credentials").read_text() == expired and set(root.iterdir()) == entries
    finally:
        (root / "credentials").write_bytes(saved)


def check_refresh(binary, prefix, root, url):
    Receipt.is_rejecting_old = True
    Receipt.echo_id = "fixture-account"
    start = len(Receipt.requests)
    result, _envelope = invoke(binary, prefix, root, ["share", "files", url, "--auth-mode", "oauth", "--password-stdin"], "fixture-password\n")
    Receipt.echo_id = "receipt-request"
    assert result.returncode == 0
    requests = Receipt.requests[start:]
    assert sum(path == "/v1/oauth/token" for path, _headers, _body in requests) == 1
    assert requests[0][1].get("authorization") == "Bearer fixture-account"
    assert requests[2][1].get("authorization") == "Bearer fixture-new-account"
    assert not requests[-1][1].get("authorization")
    result, _envelope = invoke(binary, prefix, root, ["share", "download", url, str(root / "refreshed"), "--auth-mode", "oauth",
        "--password-stdin", "--path", "a.txt", "--checkpoint", str(root / "refreshed-journal")], "fixture-password\n")
    assert result.returncode == 0
    context = json.loads((root / "refreshed-journal").read_text())["identity"]["recipient"]
    assert context["user_id"] == "refreshed-user"


def check_applied_subject(binary, prefix, root, url):
    original = (root / "credentials").read_text()
    replacement = original.replace("fixture-account", "fixture-replaced-account").replace("receiver-user", "replaced-user")
    Receipt.exchange_hook = lambda: (root / "credentials").write_text(replacement)
    try:
        result, _envelope = invoke(binary, prefix, root, ["share", "download", url, str(root / "snapshot"),
            "--auth-mode", "oauth", "--password-stdin", "--path", "a.txt", "--checkpoint", str(root / "snapshot-journal")], "fixture-password\n")
        assert result.returncode == 0
        context = json.loads((root / "snapshot-journal").read_text())["identity"]["recipient"]
        assert context["user_id"] == "receiver-user", "checkpoint must bind the applied OAuth subject"
    finally:
        Receipt.exchange_hook = None
        (root / "credentials").write_text(original)


def check_instance_snapshot(binary, prefix, root):
    original = (root / "credentials").read_text()
    replacement = original.replace("instance_id='inst'", "instance_id='other'").replace("fixture-account", "fixture-other-instance")
    Receipt.info_hook = lambda: (root / "credentials").write_text(replacement)
    start = len(Receipt.requests)
    try:
        result, _envelope = invoke(binary, prefix, root, ["share", "files", "http://inst.idsapp-boe.volces.com/share/s",
            "--auth-mode", "oauth", "--password-stdin"], "fixture-password\n")
        assert result.returncode != 0, "known instance must match the applied credential snapshot"
        assert len(Receipt.requests) == start + 1, "mismatched identity must not reach token exchange"
    finally:
        Receipt.info_hook = None
        (root / "credentials").write_text(original)


def check_anonymous_api_binding(binary, prefix, root, endpoint, alternate):
    Receipt.is_login_required = False
    try:
        arguments = ["share", "download", "http://pages.test/share/s", str(root / "anonymous"), "--endpoint", endpoint,
                     "--password-stdin", "--path", "a.txt", "--checkpoint", str(root / "anonymous-journal")]
        result, _envelope = invoke(binary, prefix, root, arguments, "fixture-password\n")
        assert result.returncode == 0
        before = signed_count()
        changed = arguments.copy()
        changed[changed.index("--endpoint") + 1] = alternate
        result, _envelope = invoke(binary, prefix, root, changed + ["--resume"], "fixture-password\n")
        assert result.returncode != 0 and signed_count() == before
        check_legacy_api(binary, prefix, root, endpoint)
    finally:
        Receipt.is_login_required = True


def check_legacy_api(binary, prefix, root, endpoint):
    arguments = ["share", "download", endpoint + "/share/s", str(root / "legacy"),
                 "--password-stdin", "--path", "a.txt", "--checkpoint", str(root / "legacy-journal")]
    result, _envelope = invoke(binary, prefix, root, arguments, "fixture-password\n")
    assert result.returncode == 0
    journal = json.loads((root / "legacy-journal").read_text())
    journal["identity"].pop("api_origin")
    journal["identity"].pop("selection_policy", None)
    journal["manifest"].pop("policy", None)
    journal.pop("completion_ledger", None)
    journal["completed"] = [hashlib.sha256((root / "legacy" / entry["local"]).read_bytes()).hexdigest()
                            for entry in journal["manifest"]["files"]]
    # A genuine pre-ledger snapshot embeds its verified completions in the JSON.
    shutil.rmtree(root / "legacy-journal.completed")
    (root / "legacy-journal").write_text(json.dumps(journal))
    before = signed_count()
    result, envelope = invoke(binary, prefix, root, arguments + ["--resume"], "fixture-password\n")
    assert result.returncode == 0 and signed_count() == before, envelope
    assert json.loads((root / "legacy-journal").read_text())["identity"].get("api_origin") == endpoint
    result, envelope = invoke(binary, prefix, root, arguments + ["--resume"], "fixture-password\n")
    assert result.returncode == 0 and signed_count() == before, envelope


def check_no_io(binary, prefix, root, url):
    (root / "config").write_text("invalid account config")
    (root / "credentials").write_text("invalid credentials")
    start = len(Receipt.requests)
    for action in ("inspect", "files", "download"):
        arguments = ["share", action, url] + ([str(root / "dry-out")] if action == "download" else [])
        # [Review Fix #D02] Download planning now resolves the explicitly selected OAuth profile and performs metadata I/O.
        flags = ("--describe",) if action == "download" else ("--dry-run", "--describe")
        for flag in flags:
            result, _envelope = invoke(binary, prefix, root, arguments + ["--auth-mode", "oauth", flag])
            assert result.returncode == 0 and len(Receipt.requests) == start
    assert not (root / "dry-out").exists()
    for mode in ([], ["--auth-mode", "aksk"], ["--auth-mode", "unified"]):
        result, _envelope = invoke(binary, prefix, root, ["share", "inspect", url, *mode])
        assert result.returncode == 0 and not Receipt.requests[-1][1].get("authorization")
    before = len(Receipt.requests)
    result, _envelope = invoke(binary, prefix, root, ["share", "inspect", url, "--auth-mode", "oauth"])
    assert result.returncode != 0 and len(Receipt.requests) == before


def check_missing_login(binary, prefix, root, endpoint, auth_endpoint):
    seed_profile(root,endpoint,auth_endpoint)
    (root / "credentials").write_text(f"schema_version=1\n[receiver.adrive.oauth]\ninstance_id='inst'\nauth_endpoint='{auth_endpoint}'\n")
    before=len(Receipt.requests)
    for action in ("inspect", "files", "download"):
        arguments=["share",action,"http://pages.test/share/s"] + ([str(root / "missing-login-out")] if action == "download" else [])
        result,envelope=invoke(binary,prefix,root,arguments + ["--auth-mode","oauth"])
        assert result.returncode != 0 and len(Receipt.requests)==before
        assert "auth login" in envelope["error"]["fix_command"], "explicit OAuth must preserve the login workflow hint"


def check_binary(binary, prefix, endpoint, auth_endpoint):
    with tempfile.TemporaryDirectory(prefix="share-oauth-", dir=Path(tempfile.gettempdir()).resolve()) as directory:
        root = Path(directory)
        seed_profile(root, endpoint, auth_endpoint)
        url = "http://pages.test/share/s"
        check_modes(binary, prefix, root, url, endpoint)
        check_pages(binary, prefix, root, url)
        check_download(binary, prefix, root, url)
        check_read_only_plan(binary, prefix, root, url)
        check_applied_subject(binary, prefix, root, url)
        check_instance_snapshot(binary,prefix,root)
        check_refresh(binary, prefix, root, url)
        check_anonymous_api_binding(binary, prefix, root, endpoint, auth_endpoint)
        # Anonymous custom API override remains explicit and needs no account files.
        check_no_io(binary, prefix, root, endpoint + "/share/s")
        check_missing_login(binary,prefix,root,endpoint,auth_endpoint)


def main():
    binaries = [Path(sys.argv[1]) if len(sys.argv) > 1 else Path("target/debug/ve-storage-uni-cli"),
                Path(sys.argv[2]) if len(sys.argv) > 2 else Path("packaging/cargo/ve-adrive-cli/target/debug/ve-adrive-cli")]
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Receipt)
    auth = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Receipt)
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    auth_worker = threading.Thread(target=auth.serve_forever, daemon=True)
    worker.start()
    auth_worker.start()
    try:
        for binary, prefix in zip(binaries, (["ve-adrive"], [])):
            check_binary(binary.resolve(), prefix, f"http://127.0.0.1:{server.server_port}", f"http://127.0.0.1:{auth.server_port}")
            print(f"PASS {binary.name}: mode/profile, pagination/password, download/resume, refresh/redaction, no-I/O")
    finally:
        server.shutdown()
        server.server_close()
        auth.shutdown()
        auth.server_close()
        worker.join(timeout=5)
        auth_worker.join(timeout=5)


if __name__ == "__main__":
    main()
