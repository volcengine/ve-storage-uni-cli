"""Verify command ID bounds and per-row report IDs using isolated HTTP fixtures."""
import csv
import hashlib
import json
from pathlib import Path
import sys
import tempfile

from directory_upload_regression import Fixture, UploadService


class ReportService(UploadService):
    def respond(self, value=None, status=200, headers=None):
        encoded = json.dumps(value or {}).encode()
        key = getattr(self, "report_key", self.route()[1]).rstrip("/")
        request_id = "report-" + hashlib.sha256((self.command + key).encode()).hexdigest()[:20]
        has_id = "no-id" not in key
        self.server.response_ids[(self.command, key)] = request_id if has_id else ""
        self.send_response(status)
        self.send_header("Content-Length", str(len(encoded)))
        self.send_header("Content-Type", "application/json")
        if has_id:
            self.send_header("x-tos-request-id", request_id)
            self.send_header("x-ids-request-id", request_id)
        for name, value in (headers or {}).items():
            self.send_header(name, str(value))
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(encoded)

    def write_object(self, key, body):
        self.report_key = key
        if "failure" in key:
            self.server.events.append((self.command, key))
            return self.respond({"Code": "AccessDenied", "Message": "fixture denied"}, 403)
        return super().write_object(key, body)


def report_rows(envelope):
    report = Path(envelope["data"]["report_path"])
    paths = [report, *sorted(report.parent.glob(report.stem + ".part-*.csv"))]
    rows = []
    for path in paths:
        with path.open(newline="") as stream:
            reader = csv.DictReader(stream)
            assert reader.fieldnames[-1] == "request_id", reader.fieldnames
            rows.extend(reader)
    return rows


def check_rows(fixture, envelope, rows):
    method = "POST" if fixture.surface == "ve-adrive" else "PUT"
    destination_root = fixture.destination("")
    transfer_rows = [row for row in rows if row["status"] in ("succeeded", "failed")]
    assert len(transfer_rows) > 100, len(transfer_rows)
    for row in transfer_rows:
        key = row["destination"].removeprefix(destination_root).rstrip("/")
        expected = fixture.server.response_ids.get((method, key))
        assert expected is not None, (fixture.surface, row, fixture.server.response_ids)
        assert row["request_id"] == expected, (fixture.surface, row, expected)
    assert any(row["status"] == "failed" and row["request_id"] for row in rows)
    assert any(row["status"] == "failed" and not row["request_id"] for row in rows)
    assert any(row["status"] == "succeeded" and not row["request_id"] for row in rows)
    retained = envelope["data"]["service_request_ids"]
    assert len(retained) == 100, len(retained)
    assert envelope["data"]["service_request_ids_omitted"] > 0
    assert any(row["request_id"] and row["request_id"] not in retained for row in rows)


def run_transfer(fixture, action, flags):
    fixture.reset()
    fixture.server.response_ids.clear()
    arguments = [action, str(fixture.source), fixture.destination(),
                 "--checkpoint-dir", str(fixture.root / "checkpoints"), *flags]
    if action == "cp":
        arguments.append("--recursive")
    result = fixture.invoke(arguments)
    assert result.returncode != 0, (result.stdout, result.stderr)
    envelope = json.loads(result.stdout)
    rows = report_rows(envelope)
    check_rows(fixture, envelope, rows)


def check_binary(binary, surface):
    with tempfile.TemporaryDirectory(prefix="report-request-ids-") as directory:
        fixture = Fixture(binary, surface, Path(directory))
        fixture.server.RequestHandlerClass = ReportService
        fixture.server.response_ids = {}
        for index in range(110):
            (fixture.source / f"file-{index:03}.txt").write_text(f"file {index}")
        for name in ["failure.txt", "no-id-failure.txt", "no-id.txt"]:
            (fixture.source / name).write_text(name)
        try:
            for action in ("cp", "sync"):
                for flags in ((), ("--no-manifest",)):
                    run_transfer(fixture, action, flags)
            print(f"PASS: {binary.name} {surface}; per-row success/failure IDs, missing IDs, 100-ID summary")
        finally:
            fixture.close()


if __name__ == "__main__":
    assert len(sys.argv) > 1, "pass unified and/or standalone binaries"
    for argument in sys.argv[1:]:
        binary = Path(argument).resolve()
        surfaces = {"tos-cli": ["tos"], "ve-tos-cli": ["ve-tos"], "ve-adrive-cli": ["ve-adrive"]}
        for surface in surfaces.get(binary.name, ["tos", "ve-tos", "ve-adrive"]):
            check_binary(binary, surface)
