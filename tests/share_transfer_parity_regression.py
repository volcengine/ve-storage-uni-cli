"""Share transfer policy and artifacts through real entrypoints, localhost only."""
import csv
import http.server
from pathlib import Path
import sys
import tempfile
import threading

from share_dry_run_regression import PlanFixture, invoke


def check_plan(binary, origin, root):
    destination = root / "planned"
    options = ["--recursive", "--include", "*.txt", "--exclude", "*b.txt",
               "--batch-concurrency", "2", "--list-concurrency", "2"]
    result, envelope = invoke(binary, origin, destination, options)
    assert result.returncode == 0, envelope
    selected = envelope["data"]["selection"]
    files = selected["Manifest"]["Files"]
    assert [item["LocalPath"] for item in files] == ["a.txt", "sub/c.txt"], files
    assert not destination.exists() and not (root / "report.csv").exists()
    assert not (root / "manifest.csv").exists()
    result, envelope = invoke(binary, origin, destination,
                              [*options, "--include-parent"])
    assert result.returncode == 0, envelope
    assert [item["LocalPath"] for item in envelope["data"]["selection"]["Manifest"]["Files"]] == [
        "tree/a.txt", "tree/sub/c.txt"]


def check_execution(binary, origin, root):
    PlanFixture.can_download = True
    destination = root / "downloaded"
    options = ["--recursive", "--include", "*.txt", "--exclude", "*b.txt",
               "--batch-concurrency", "2", "--list-concurrency", "2",
               "--checkpoint", "--checkpoint-dir", str(root / "checkpoints")]
    result, envelope = invoke(binary, origin, destination, options, is_dry_run=False)
    assert result.returncode == 0, envelope
    assert (destination / "a.txt").read_bytes() == b"abc"
    assert (destination / "sub/c.txt").read_bytes() == b"abc"
    assert not (destination / "b.txt").exists() and not (destination / "tree").exists()
    report = list(csv.DictReader((root / "report.csv").open()))
    manifest = list(csv.DictReader((root / "manifest.csv").open()))
    assert len(report) == len(manifest) == 2
    assert all(item["status"] == "succeeded" and "request_id" in item for item in report)
    assert envelope["data"]["report_path"] == str(root / "report.csv")
    assert list((root / "checkpoints").glob("*.json"))
    result, envelope = invoke(binary, origin, destination,
                              [*options, "--resume"], is_dry_run=False)
    assert result.returncode == 0, envelope
    assert envelope["data"]["Summary"]["Skipped"] == 2, envelope


def main():
    binaries = [Path(value).resolve() for value in sys.argv[1:]]
    assert binaries, "pass unified and standalone binary paths"
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), PlanFixture)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        for binary in binaries:
            with tempfile.TemporaryDirectory(prefix="share-parity-") as directory:
                root = Path(directory)
                PlanFixture.can_download = False
                check_plan(binary, f"http://127.0.0.1:{server.server_port}", root)
                check_execution(binary, f"http://127.0.0.1:{server.server_port}", root)
        print(f"PASS: {len(binaries)} entrypoints; filters, include-parent, concurrency, artifacts and checkpoint policy")
    finally:
        server.shutdown()
        server.server_close()


if __name__ == "__main__":
    main()
