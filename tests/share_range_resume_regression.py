"""Cross-process share range recovery through both real CLI entrypoints, localhost only.

Run: python3 tests/share_range_resume_regression.py [unified-binary] [standalone-binary]
"""
import csv
import hashlib
import http.server
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import threading
import time

from share_range_regression import PART, PAYLOAD, RangeFixture, transfer_paths


FIRST_RANGE = f"bytes=0-{PART - 1}"
SECOND_RANGE = f"bytes={PART}-{2 * PART - 1}"
LAST_RANGE = f"bytes={2 * PART}-{len(PAYLOAD) - 1}"
FIRST_DIGEST = hashlib.sha256(PAYLOAD[:PART]).hexdigest()
SECRET_MARKERS = (b"range-secret", b"signed=secret", b'"AccessToken"', b'"DownloadURL"')


class ResumeFixture(RangeFixture):
    started = threading.Event()
    release = threading.Event()
    lock = threading.Lock()

    def content(self):
        if self.mode == "hold-second" and self.headers.get("Range") == SECOND_RANGE:
            assert not self.headers.get("Authorization")
            assert not self.headers.get("x-share-token")
            assert self.headers.get("If-Match") == '"snapshot"'
            with self.lock:
                type(self).ranges.append(SECOND_RANGE)
            self.wait_for_cancellation()
            return
        super().content()


def reset_fixture(mode):
    with ResumeFixture.lock:
        assert ResumeFixture.active == 0, "previous content handlers have not drained"
        ResumeFixture.mode = mode
        ResumeFixture.ranges = []
        ResumeFixture.issued = 0
        ResumeFixture.peak = 0
    ResumeFixture.started.clear()
    ResumeFixture.release.clear()


def arguments(binary, root, origin, is_resuming):
    prefix = [] if binary.name == "ve-adrive-cli" else ["ve-adrive"]
    selected = [str(binary), *prefix, "share", "download", f"{origin}/share/s",
                str(root / "out"), "--output", "json", "--no-progress",
                "--multipart-concurrency", "1", "--checkpoint", str(root / "checkpoint.json"),
                *transfer_paths(root)]
    return [*selected, "--resume"] if is_resuming else selected


def isolated_environment():
    return {key: value for key, value in os.environ.items() if not key.startswith("ADRIVE_")}


def wait_for_receipt(root, process):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        assert process.poll() is None, "download ended before its range could be interrupted"
        receipts = list((root / "checkpoint.json.parts").glob("*/part-1.json"))
        if receipts:
            assert len(receipts) == 1, receipts
            receipt = json.loads(receipts[0].read_text())
            assert (receipt["offset"], receipt["size"], receipt["sha256"]) == (0, PART, FIRST_DIGEST)
            part = receipts[0].with_suffix(".bin")
            assert part.stat().st_size == PART
            assert hashlib.sha256(part.read_bytes()).hexdigest() == FIRST_DIGEST
            return
        time.sleep(0.02)
    raise AssertionError("the first completed range was not durably recorded")


def readable_report(root, expected_status):
    with (root / "report.csv").open(newline="") as source:
        rows = list(csv.DictReader(source))
    assert len(rows) == 1 and rows[0]["source"] == "large.bin", rows
    assert rows[0]["status"] == expected_status, rows
    return rows[0]


def assert_no_secrets(root, output):
    encoded = output.encode()
    assert all(marker not in encoded for marker in SECRET_MARKERS), "CLI output exposed a credential"
    # Check every persisted artifact, including checkpoint, receipt, ledger and partial binary bytes.
    for path in root.rglob("*"):
        if path.is_file():
            contents = path.read_bytes()
            assert all(marker not in contents for marker in SECRET_MARKERS), path


def drain_handlers():
    ResumeFixture.release.set()
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        with ResumeFixture.lock:
            if ResumeFixture.active == 0:
                return
        time.sleep(0.02)
    raise AssertionError("interrupted content handlers did not drain")


def interrupted_process(binary, root, origin):
    reset_fixture("hold-second")
    process = subprocess.Popen(arguments(binary, root, origin, False),
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                               env=isolated_environment())
    try:
        assert ResumeFixture.started.wait(10), "the second range did not reach its barrier"
        wait_for_receipt(root, process)
        process.send_signal(signal.SIGINT)
        stdout, stderr = process.communicate(timeout=10)
        envelope = json.loads(stdout)
        assert process.returncode != 0, (stdout, stderr)
        assert envelope["data"]["Summary"]["Failed"] == 1, envelope
        assert envelope["data"]["Summary"]["Completed"] == 0, envelope
        assert envelope["data"]["CheckpointAvailable"], envelope
        assert envelope["data"]["Files"][0]["Status"] == "failed", envelope
        assert Path(envelope["data"]["report_path"]) == root / "report.csv", envelope
        row = readable_report(root, "failed")
        assert "interrupted" in row["error"] and row["request_id"] == "", row
        assert not (root / "out/large.bin").exists()
        assert not list((root / "out").glob(".adrive-share-*"))
        assert ResumeFixture.ranges == [FIRST_RANGE, SECOND_RANGE], ResumeFixture.ranges
        assert ResumeFixture.issued == 1, "issuance was repeated before cancellation"
        assert_no_secrets(root, stdout + stderr)
        return process.pid
    finally:
        ResumeFixture.release.set()
        if process.poll() is None:
            process.kill()
            process.communicate(timeout=5)
        drain_handlers()


def resumed_process(binary, root, origin, interrupted_pid):
    reset_fixture("normal")
    process = subprocess.Popen(arguments(binary, root, origin, True),
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                               env=isolated_environment())
    try:
        assert process.pid != interrupted_pid, "recovery did not start a new process"
        stdout, stderr = process.communicate(timeout=40)
        envelope = json.loads(stdout)
        assert process.returncode == 0, (stdout, stderr)
        assert envelope["data"]["Summary"]["Completed"] == 1, envelope
        assert envelope["data"]["Summary"]["Failed"] == 0, envelope
        assert (root / "out/large.bin").read_bytes() == PAYLOAD
        assert ResumeFixture.ranges == [SECOND_RANGE, LAST_RANGE], ResumeFixture.ranges
        assert FIRST_RANGE not in ResumeFixture.ranges, "the verified first range was downloaded again"
        assert ResumeFixture.issued == 1, "resume must issue exactly one fresh download URL"
        readable_report(root, "succeeded")
        assert_no_secrets(root, stdout + stderr)
        assert not (root / "missing-credentials").exists()
    finally:
        if process.poll() is None:
            process.kill()
            process.communicate(timeout=5)
        drain_handlers()


def check_binary(binary, origin):
    with tempfile.TemporaryDirectory(prefix="share-range-resume-") as name:
        root = Path(name).resolve()
        interrupted_pid = interrupted_process(binary, root, origin)
        resumed_process(binary, root, origin, interrupted_pid)


def main():
    binaries = [Path(value).resolve() for value in sys.argv[1:]] or [
        Path("target/debug/ve-storage-uni-cli").resolve(),
        Path("packaging/cargo/ve-adrive-cli/target/debug/ve-adrive-cli").resolve(),
    ]
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), ResumeFixture)
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    try:
        for binary in binaries:
            check_binary(binary, f"http://127.0.0.1:{server.server_port}")
        print(f"share range cross-process resume regressions passed ({len(binaries)} entrypoints)")
    finally:
        ResumeFixture.release.set()
        server.shutdown()
        server.server_close()
        worker.join(timeout=2)


if __name__ == "__main__":
    main()
