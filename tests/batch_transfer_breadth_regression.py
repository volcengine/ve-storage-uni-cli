"""Exercise non-upload batch directions with exact names, bytes and report IDs."""
import hashlib
import json
from pathlib import Path
import shutil
import sys
import tempfile
from urllib.parse import unquote, urlsplit

from directory_upload_regression import Fixture
from report_request_id_regression import ReportService, report_rows


class BreadthService(ReportService):
    def metadata(self, key):
        body = self.server.objects[key]
        is_folder = key.endswith("/")
        return {"ETag": '"fixture"', "Content-Length": len(body),
                "Last-Modified": "Thu, 01 Jan 2026 00:00:00 GMT",
                "x-ids-file-etag": "fixture", "x-ids-file-type": "folder" if is_folder else "file",
                "x-ids-is-folder": str(is_folder).lower(),
                "x-ids-file-is-folder": str(is_folder).lower()}

    def route(self):
        service, key, query = super().route()
        if service == "adrive" and "/folders/" in unquote(urlsplit(self.path).path):
            key = unquote(urlsplit(self.path).path).split("/folders/", 1)[1]
        return service, key, query

    def do_HEAD(self):
        service, key, _ = self.route()
        if service == "tos" and not key:
            return self.respond(headers={"x-tos-bucket-type": self.server.bucket_type})
        if key not in self.server.objects and key.rstrip("/") + "/" in self.server.objects:
            key = key.rstrip("/") + "/"
        self.report_key = key
        if key not in self.server.objects:
            return self.respond({"Code": "NoSuchKey"}, 404)
        self.send_object(key)

    def do_GET(self):
        _, key, query = self.route()
        if "prefix" in query or not key or key.endswith("/files"):
            if getattr(self.server, "listing_failure", None) and query.get("prefix", [""])[0].startswith("src"):
                return self.failed_listing(query)
            return super().do_GET()
        self.report_key = key
        if key not in self.server.objects:
            return self.respond({"Code": "NoSuchKey"}, 404)
        self.send_object(key)

    def failed_listing(self, query):
        if any(query.get(name) for name in ("marker", "continuation-token", "start-after")):
            self.report_key = "failed-list" + ("-no-id" if self.server.listing_failure == "no-id" else "")
            return self.respond({"Code": "AccessDenied", "Message": "listing denied", "RequestId": "untrusted-body-id"}, 403)
        entry = {"Key": "src/plain.txt", "FilePath": "src/plain.txt", "FileType": "file",
                 "Size": len(self.server.objects["src/plain.txt"]), "ETag": "fixture"}
        self.respond({"Files": [entry], "Contents": [entry], "Folders": [], "CommonPrefixes": [],
                      "IsTruncated": True, "NextMarker": "next", "NextContinuationToken": "next"})

    def send_object(self, key):
        body = self.server.objects[key]
        request_id = "report-" + hashlib.sha256((self.command + key.rstrip("/")).encode()).hexdigest()[:20]
        self.server.response_ids[(self.command, key.rstrip("/"))] = request_id
        self.send_response(200)
        self.send_header("x-tos-request-id", request_id)
        self.send_header("x-ids-request-id", request_id)
        for name, value in self.metadata(key).items():
            self.send_header(name, str(value))
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    def copy_object(self):
        service, key, _ = self.route()
        if service == "adrive":
            payload = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
            source, destination = key.removesuffix(":copy"), payload["CopyToPath"]
        else:
            source = self.headers["x-tos-copy-source"]
            if source.startswith("%2F"):
                source = unquote(source)
            else:
                source = source.encode("latin1").decode("utf8")
            source, destination = source.removeprefix("/bucket/"), key
        assert source in self.server.objects, (source, list(self.server.objects))
        self.report_key = destination
        self.server.objects[destination] = self.server.objects[source]
        self.respond({"ETag": "fixture", "CopyToFilePath": destination, "Status": "success"})

    def do_PUT(self):
        if self.headers.get("x-tos-copy-source"):
            return self.copy_object()
        return super().do_PUT()

    def do_POST(self):
        _, key, query = self.route()
        if key.endswith(":copy") or self.headers.get("x-tos-copy-source"):
            return self.copy_object()
        if "restore" in query:
            self.rfile.read(int(self.headers.get("Content-Length", "0")))
            return self.respond()
        return super().do_POST()

    def do_DELETE(self):
        _, key, _ = self.route()
        self.report_key = key
        if "/folders/" in unquote(urlsplit(self.path).path):
            prefix = key.rstrip("/") + "/"
            for name in list(self.server.objects):
                if name.startswith(prefix):
                    del self.server.objects[name]
            return self.respond()
        return super().do_DELETE()


NAMES = ["中文 文件.txt", "符号+%25%2F?#.txt", "plain.txt"]


def seed_remote(fixture):
    fixture.reset()
    fixture.server.response_ids.clear()
    fixture.server.objects["src/"] = b""
    for name in NAMES:
        fixture.server.objects["src/" + name] = name.encode()


def invoke_case(fixture, arguments):
    result = fixture.invoke([*arguments, "--checkpoint-dir", str(fixture.root / "checkpoints")]
                            if arguments[0] in ("cp", "mv", "sync") else arguments)
    assert result.returncode == 0, (fixture.surface, arguments, result.stdout, result.stderr)
    envelope = json.loads(result.stdout)
    rows = report_rows(envelope)
    assert rows, (arguments, envelope)
    for row in rows:
        if row["status"] != "succeeded":
            continue
        if not any(row[field].startswith(("tos://", "adrive://")) for field in ("source", "destination")):
            assert not row["request_id"], row
            continue
        if row["source"].endswith("/") and not row["destination"].startswith(("tos://", "adrive://")) and row["destination"]:
            assert not row["request_id"], row
            continue
        assert row["request_id"], (arguments, row)
        remote = row["destination"] if row["destination"].startswith(("tos://", "adrive://")) else row["source"]
        key = remote.removeprefix(fixture.destination("")).rstrip("/")
        if row["destination"].startswith(("tos://", "adrive://")):
            methods = ("POST",) if fixture.surface == "ve-adrive" else ("PUT", "POST")
        elif not row["destination"]:
            methods = ("POST",) if arguments[0] == "restore" else ("DELETE",)
        else:
            methods = ("GET",)
        expected = {fixture.server.response_ids.get((method, key)) for method in methods}
        assert row["request_id"] in expected, (arguments, row, expected)
    assert len(envelope["data"].get("service_request_ids", [])) <= 100
    return rows


def check_download(fixture, action, mode):
    seed_remote(fixture)
    destination = fixture.root / ("download-" + action + str(len(mode)))
    shutil.rmtree(destination, ignore_errors=True)
    arguments = [action, fixture.destination("src/"), str(destination), *mode]
    if action in ("cp", "mv"):
        arguments.append("--recursive")
    if action == "mv":
        fixture.server.objects["src/excluded.bin"] = b"preserve excluded source"
        arguments.extend(["--include", "*.txt", "--force", "--confirm", fixture.destination("src/")])
    invoke_case(fixture, arguments)
    for name in NAMES:
        assert (destination / name).read_bytes() == name.encode(), (fixture.surface, name)
        if action == "mv":
            assert "src/" + name not in fixture.server.objects, (fixture.surface, name)
    if action == "mv":
        assert fixture.server.objects["src/excluded.bin"] == b"preserve excluded source"


def check_remote_copy(fixture, action, mode):
    seed_remote(fixture)
    arguments = [action, fixture.destination("src/"), fixture.destination("copied/"), *mode]
    if action in ("cp", "mv"):
        arguments.append("--recursive")
    if action == "mv":
        fixture.server.objects["src/excluded.bin"] = b"preserve excluded source"
        # ADrive same-space filtered moves must exercise copy/delete instead of rename.
        arguments.extend(["--include", "*.txt", "--force", "--confirm", fixture.destination("src/")])
    invoke_case(fixture, arguments)
    for name in NAMES:
        assert fixture.server.objects["copied/" + name] == name.encode(), (fixture.surface, name)
        if action == "mv":
            assert "src/" + name not in fixture.server.objects, (fixture.surface, name)
    if action == "mv":
        assert fixture.server.objects["src/excluded.bin"] == b"preserve excluded source"
        assert "copied/excluded.bin" not in fixture.server.objects


def check_upload_move(fixture, mode):
    fixture.reset()
    fixture.server.response_ids.clear()
    source = fixture.root / "move-upload"
    shutil.rmtree(source, ignore_errors=True)
    source.mkdir()
    for name in NAMES:
        (source / name).write_bytes(name.encode())
    (source / "excluded.bin").write_bytes(b"preserve excluded source")
    invoke_case(fixture, ["mv", str(source), fixture.destination("uploaded/"), "--recursive",
                          "--include", "*.txt", "--force", "--confirm", str(source), *mode])
    for name in NAMES:
        assert fixture.server.objects["uploaded/" + name] == name.encode(), (fixture.surface, name)
        assert not (source / name).exists(), (fixture.surface, name)
    assert (source / "excluded.bin").read_bytes() == b"preserve excluded source"


def check_delete(fixture, mode):
    seed_remote(fixture)
    source = fixture.destination("src/")
    invoke_case(fixture, ["rm", source, "--recursive", "--force", "--confirm", source, *mode])
    assert not any(key.startswith("src/") for key in fixture.server.objects)


def check_restore(fixture, mode):
    seed_remote(fixture)
    invoke_case(fixture, ["restore", fixture.destination("src/"), "--recursive", "--force", *mode])
    assert all("src/" + name in fixture.server.objects for name in NAMES)


def check_partial_listing(fixture, action, failure_mode):
    seed_remote(fixture)
    fixture.server.listing_failure = failure_mode
    source = fixture.destination("src/")
    arguments = [action, source, "--recursive", "--no-manifest", "--batch-concurrency", "1"]
    if action in ("cp", "mv"):
        destination = str(fixture.root / ("partial-" + action)) if action == "cp" else fixture.destination("partial/")
        arguments.insert(2, destination)
    if action in ("mv", "rm"):
        arguments.extend(["--force", "--confirm", source])
    if action == "mv":
        arguments.extend(["--include", "*.txt"])
    if action == "restore":
        arguments.append("--force")
    try:
        result = fixture.invoke(arguments)
        assert result.stdout or result.stderr, (arguments, result.returncode)
        try:
            envelope = json.loads(result.stdout or result.stderr[result.stderr.index("{\n"):])
        except json.JSONDecodeError as error:
            raise AssertionError((arguments, result.returncode, result.stdout, result.stderr)) from error
        assert result.returncode != 0 and not envelope["success"], (arguments, envelope)
        assert envelope["status_code"] == 403, (arguments, envelope)
        assert envelope["error"]["doctor_hint"] == fixture.surface + " doctor", envelope
        assert envelope["data"]["report_path"], (arguments, envelope)
        rows = report_rows(envelope)
        expected = fixture.server.response_ids[("GET", "failed-list" + ("-no-id" if failure_mode == "no-id" else ""))]
        assert envelope["request_id"] == (expected or None), (arguments, envelope, expected)
        for row in rows:
            if row["status"] == "succeeded":
                assert row["request_id"] in fixture.server.response_ids.values(), row
    finally:
        fixture.server.listing_failure = None


def check_move_overlap_rejected(fixture, mode):
    seed_remote(fixture)
    before = dict(fixture.server.objects)
    source = fixture.destination("src/")
    arguments = ["mv", source, fixture.destination(""), "--recursive", "--include-parent",
                 "--include", "*.txt", "--force", "--confirm", source, *mode]
    result = fixture.invoke(arguments)
    assert result.returncode == 6, (arguments, result.stdout, result.stderr)
    assert fixture.server.objects == before, (fixture.surface, fixture.server.objects)


def check_failure_filter(fixture, mode):
    fixture.reset()
    fixture.server.response_ids.clear()
    for name in ("failure.txt", "no-id-failure.txt"):
        (fixture.source / name).write_text(name)
    arguments = ["cp", str(fixture.source), fixture.destination(), "--recursive",
                 "--report-failures-only", "--checkpoint-dir", str(fixture.root / "checkpoints"), *mode]
    result = fixture.invoke(arguments)
    envelope = json.loads(result.stdout)
    rows = report_rows(envelope)
    assert result.returncode == 1, (arguments, envelope)
    assert len(rows) == 2 and all(row["status"] == "failed" for row in rows), rows
    method = "POST" if fixture.surface == "ve-adrive" else "PUT"
    for row in rows:
        key = row["destination"].removeprefix(fixture.destination(""))
        assert row["request_id"] == fixture.server.response_ids[(method, key)], row


def check_local(fixture):
    destination = fixture.root / "local-copy"
    rows = invoke_case(fixture, ["cp", str(fixture.source), str(destination), "--recursive"])
    assert all(not row["request_id"] for row in rows), rows


def check_binary(binary, surface):
    with tempfile.TemporaryDirectory(prefix="batch-breadth-") as directory:
        fixture = Fixture(binary, surface, Path(directory))
        fixture.server.RequestHandlerClass = BreadthService
        fixture.server.response_ids = {}
        try:
            if surface != "ve-adrive":
                check_local(fixture)
            for mode in [(), ("--no-manifest",)]:
                for action in ["cp", "sync"]:
                    check_download(fixture, action, mode)
                    check_remote_copy(fixture, action, mode)
                check_remote_copy(fixture, "mv", mode)
                check_download(fixture, "mv", mode)
                check_upload_move(fixture, mode)
                check_move_overlap_rejected(fixture, mode)
                check_delete(fixture, mode)
                if surface == "ve-tos":
                    check_restore(fixture, mode)
            for action in ("cp", "mv", "rm", *(["restore"] if surface == "ve-tos" else [])):
                for failure_mode in ("with-id", "no-id"):
                    check_partial_listing(fixture, action, failure_mode)
            for mode in [(), ("--no-manifest",)]:
                check_failure_filter(fixture, mode)
            print(f"PASS: {binary.name} {surface}; download/copy/move/sync/delete/restore, exact names/bytes, reports, paging failures")
        finally:
            fixture.close()


if __name__ == "__main__":
    assert len(sys.argv) > 1, "pass unified and/or standalone binaries"
    for argument in sys.argv[1:]:
        binary = Path(argument).resolve()
        surfaces = {"tos-cli": ["tos"], "ve-tos-cli": ["ve-tos"], "ve-adrive-cli": ["ve-adrive"]}
        for surface in surfaces.get(binary.name, ["tos", "ve-tos", "ve-adrive"]):
            check_binary(binary, surface)
