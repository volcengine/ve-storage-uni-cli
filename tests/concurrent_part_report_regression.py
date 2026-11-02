"""Verify a failed part owns its file report ID despite concurrent successes.

Run against unified and/or standalone binaries. All requests use a loopback
fixture with fake credentials; uploads use sparse local files.
"""
import json
from pathlib import Path
import shutil
import sys
import tempfile
import threading

from directory_upload_regression import Fixture, UploadService
from report_request_id_regression import ReportService, report_rows


MEBIBYTE = 1024 * 1024
PART_SIZE = {"tos": 20 * MEBIBYTE, "ve-tos": 20 * MEBIBYTE,
             "ve-adrive": 16 * MEBIBYTE}


class ConcurrentPartService(UploadService):
    """Delay the failing body until a successful sibling has completed."""

    def handle(self):
        try:
            super().handle()
        except Exception as error:
            self.server.fixture_errors.append(repr(error))

    def do_HEAD(self):
        _, key, _ = self.route()
        if not key:
            return self.respond(headers={"x-tos-bucket-type": "fns"})
        assert key in self.server.objects, key
        self.send_response(200)
        self.send_header("Content-Length", str(len(self.server.objects[key])))
        self.send_header("ETag", '"fixture"')
        self.send_header("x-tos-request-id", "head-id")
        self.send_header("x-ids-request-id", "head-id")
        self.end_headers()

    def do_POST(self):
        self.rfile.read(int(self.headers.get("Content-Length", "0")))
        assert "initiateMultipart" in self.path or "uploads" in self.path, self.path
        self.respond({"UploadId": "upload-1"})

    def do_PUT(self):
        _, _, query = self.route()
        self.rfile.read(int(self.headers.get("Content-Length", "0")))
        part_number = int(query["partNumber"][0])
        if part_number == 1:
            body = json.dumps({"Code": "Forbidden", "Message": "first part denied",
                               "RequestId": "untrusted-error-body-id"}).encode()
        else:
            body = json.dumps({"PartNumber": part_number, "ETag": "fixture"}).encode()
        self.send_part_response(part_number, body, is_upload=True)

    def do_GET(self):
        _, _, query = self.route()
        if "uploadId" in query or "uploadID" in query:
            return self.respond({"Parts": [], "IsTruncated": False})
        range_header = self.headers.get("Range")
        if not range_header:
            return super().do_GET()
        start, end = map(int, range_header.removeprefix("bytes=").split("-"))
        part_number = start // self.server.part_size + 1
        if part_number == 1 and self.server.surface == "ve-adrive":
            body = b"x"
        elif part_number == 1:
            body = json.dumps({"Code": "Forbidden", "Message": "first range denied",
                               "RequestId": "untrusted-error-body-id"}).encode()
        else:
            body = bytes(end - start + 1)
        self.send_part_response(part_number, body, is_upload=False,
                                content_range=f"bytes {start}-{end}/{self.server.file_size}")

    def send_part_response(self, part_number, body, is_upload, content_range=None):
        is_short_range = not is_upload and self.server.surface == "ve-adrive"
        is_denied = part_number == 1 and not is_short_range
        if part_number != 1:
            assert self.server.first_headers.wait(5), "first part did not send headers"
        self.send_response(403 if is_denied else (200 if is_upload else 206))
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Content-Type", "application/json" if is_upload or is_denied
                         else "application/octet-stream")
        if is_upload and not is_denied:
            self.send_header("ETag", '"fixture"')
        if content_range:
            self.send_header("Content-Range", content_range)
        if part_number != 1 or self.server.has_failure_id:
            identifier = "first-part-failure" if part_number == 1 else f"success-part-{part_number}"
            self.send_header("x-tos-request-id", identifier)
            self.send_header("x-ids-request-id", identifier)
        self.end_headers()
        self.wfile.flush()
        self.finish_part_response(part_number, body)

    def finish_part_response(self, part_number, body):
        self.server.parts_received.add(part_number)
        if part_number == 1:
            self.server.first_headers.set()
            # [Review Fix #ConcurrentPartFixture] The third request proves the
            # client consumed the successful second part before this error body.
            assert self.server.third_headers.wait(10), "successful sibling did not finish"
        elif part_number == 3:
            self.server.third_headers.set()
        self.wfile.write(body)


def configure_service(fixture, has_failure_id):
    fixture.server.RequestHandlerClass = ConcurrentPartService
    fixture.server.first_headers = threading.Event()
    fixture.server.third_headers = threading.Event()
    fixture.server.parts_received = set()
    fixture.server.fixture_errors = []
    fixture.server.surface = fixture.surface
    fixture.server.has_failure_id = has_failure_id
    fixture.server.part_size = PART_SIZE[fixture.surface]
    fixture.server.file_size = 2 * fixture.server.part_size + 1


def transfer_arguments(fixture, direction, is_streaming):
    if direction == "upload":
        shutil.rmtree(fixture.source)
        fixture.source.mkdir()
        with (fixture.source / "中文 large.bin").open("wb") as source:
            source.truncate(fixture.server.file_size)
        source, destination = str(fixture.source), fixture.destination()
    else:
        fixture.server.objects["src/中文 large.bin"] = bytes(fixture.server.file_size)
        source, destination = fixture.destination("src/"), str(fixture.root / "download")
    arguments = ["cp", source, destination, "--recursive", "--checkpoint",
                 "--checkpoint-threshold", "1B", "--multipart-concurrency", "2",
                 "--overwrite-strategy", "force", "--checkpoint-dir",
                 str(fixture.root / "checkpoints")]
    if direction == "upload":
        arguments.append("--disable-dir-object")
    if is_streaming:
        arguments.append("--no-manifest")
    return arguments


def check_case(binary, surface, direction, has_failure_id, is_streaming):
    with tempfile.TemporaryDirectory(prefix="concurrent-report-part-") as directory:
        fixture = Fixture(binary, surface, Path(directory))
        configure_service(fixture, has_failure_id)
        try:
            arguments = transfer_arguments(fixture, direction, is_streaming)
            result = fixture.invoke(arguments)
            envelope = json.loads(result.stdout)
            rows = report_rows(envelope)
            assert result.returncode == 1, (arguments, result.stdout, result.stderr)
            assert not fixture.server.fixture_errors, fixture.server.fixture_errors
            assert fixture.server.parts_received == {1, 2, 3}, fixture.server.parts_received
            assert len(rows) == 1 and rows[0]["status"] == "failed", rows
            expected = "first-part-failure" if has_failure_id else ""
            assert rows[0]["request_id"] == expected, (surface, direction, rows[0], expected)
            assert "中文 large.bin" in rows[0]["source"], rows[0]
            # [Review Fix #ConcurrentSummary] TOS streaming summaries expose
            # counters directly, while manifest and ADrive outputs nest them.
            payload = envelope["data"]
            summary = payload.get("summary", payload)
            assert summary["failed"] == 1 and summary["succeeded"] == 0, summary
            if direction == "download":
                assert not (fixture.root / "download" / "中文 large.bin").exists()
        finally:
            fixture.close()


def check_binary(binary, surface):
    for direction in ["upload", "download"]:
        for has_failure_id in [True, False]:
            for is_streaming in [False, True]:
                check_case(binary, surface, direction, has_failure_id, is_streaming)
    check_local_scan_failure(binary, surface)
    print(f"PASS: {binary.name} {surface}; concurrent part failure IDs, absent IDs, both manifest modes")


def check_local_scan_failure(binary, surface):
    with tempfile.TemporaryDirectory(prefix="local-scan-report-") as directory:
        fixture = Fixture(binary, surface, Path(directory))
        fixture.server.RequestHandlerClass = ReportService
        fixture.server.response_ids = {}
        shutil.rmtree(fixture.source)
        fixture.source.mkdir()
        for name in ["a.txt", "b.txt"]:
            (fixture.source / name).write_text(name)
        denied = fixture.source / "z-denied"
        denied.mkdir()
        denied.chmod(0)
        try:
            if can_read_directory(denied):
                print(f"SKIP: {surface} local scan permission case; process can read mode-000 directories")
                return
            result = fixture.invoke([
                "cp", str(fixture.source), fixture.destination(), "--recursive", "--no-manifest",
                "--disable-dir-object", "--batch-concurrency", "1", "--overwrite-strategy", "force",
                "--checkpoint-dir", str(fixture.root / "checkpoints")])
            envelope = json.loads(result.stdout)
            assert result.returncode == 5, (result.stdout, result.stderr)
            assert envelope.get("request_id") is None, envelope
            assert envelope["status_code"] is None and not envelope["success"], envelope
            payload = envelope["data"]
            summary = payload.get("summary", payload)
            assert summary["succeeded"] == 2, summary
            rows = report_rows(envelope)
            assert len(rows) == 2 and all(row["status"] == "succeeded" for row in rows), rows
            method = "POST" if surface == "ve-adrive" else "PUT"
            for row in rows:
                key = row["destination"].removeprefix(fixture.destination(""))
                assert row["request_id"] == fixture.server.response_ids[(method, key)], row
        finally:
            # [Review Fix #LocalScanFixture] Restore access before temporary
            # directory cleanup, including assertion and subprocess failures.
            denied.chmod(0o700)
            fixture.close()


def can_read_directory(path):
    try:
        tuple(path.iterdir())
        return True
    except PermissionError:
        return False


if __name__ == "__main__":
    assert len(sys.argv) > 1, "pass unified and/or standalone binaries"
    for argument in sys.argv[1:]:
        binary = Path(argument).resolve()
        surfaces = {"tos-cli": ["tos"], "ve-tos-cli": ["ve-tos"], "ve-adrive-cli": ["ve-adrive"]}
        for surface in surfaces.get(binary.name, ["tos", "ve-tos", "ve-adrive"]):
            check_binary(binary, surface)
