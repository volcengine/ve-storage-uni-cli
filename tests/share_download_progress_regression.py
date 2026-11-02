"""Real stderr TTY/pipe progress and cp-style summaries for both CLI entrypoints."""
import errno
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
from urllib.parse import urlsplit

from share_download_display_regression import DownloadScenario


from share_download_regression import transfer_paths


class ProgressScenario(DownloadScenario):
    content_started = threading.Event()
    can_finish = threading.Event()

    def respond_listing(self, query):
        if self.mode != "tree-success":
            return super().respond_listing(query)
        prefix = query.get("prefix", [""])[0]
        files = [{"FilePath": "tree/a.txt" if prefix == "tree/" else "tree/sub/b.txt", "Size": 3, "ETag": "abc"}]
        folders = [{"Folder": "tree/sub/"}] if prefix == "tree/" else []
        self.respond({"Files": files, "Folders": folders, "NextMarker": "", "IsTruncated": False})

    def do_GET(self):
        path = urlsplit(self.path).path
        if path == "/v1/share_file/list":
            time.sleep(0.3)
        if path == "/content":
            self.respond_content()
            return
        try:
            super().do_GET()
        except (BrokenPipeError, ConnectionResetError):
            return  # An interrupted transfer deliberately closes its connection.

    def respond_content(self):
        self.requests.append(self.path)
        assert not self.headers.get("Authorization") and not self.headers.get("x-share-token")
        self.content_started.set()
        self.can_finish.wait(8)
        time.sleep(0.3)
        try:
            self.send_response(200)
            self.send_header("Content-Length", "3")
            self.send_header("ETag", "abc")
            self.end_headers()
            self.wfile.write(b"abc")
        except (BrokenPipeError, ConnectionResetError):
            return  # Cancellation deliberately drops the held content request.


def download_arguments(binary, root, origin, mode):
    prefix = [] if binary.stem == "ve-adrive-cli" else ["ve-adrive"]
    selector = [] if mode in ("single", "interrupt") else ["--path", "tree/", "--recursive"]
    return [str(binary), *prefix, "share", "download", f"{origin}/share/s", str(root / "out"),
        *selector, "--checkpoint", str(root / "checkpoint"), *transfer_paths(root)]


def drain_terminal(master, captured):
    while True:
        try:
            chunk = os.read(master, 65536)
        except OSError as error:
            if error.errno == errno.EIO:
                return  # POSIX PTY reports EIO after its final slave closes.
            raise
        if not chunk:
            return
        captured.extend(chunk)


def invoke(binary, root, origin, case):
    import pty
    mode, flags, is_terminal = case
    ProgressScenario.mode = "single" if mode == "interrupt" else mode
    ProgressScenario.requests = []
    ProgressScenario.content_started.clear()
    if mode == "interrupt":
        ProgressScenario.can_finish.clear()
    else:
        ProgressScenario.can_finish.set()
    environment = {key: value for key, value in os.environ.items() if not key.startswith("ADRIVE_")}
    environment.update(TERM="xterm-256color", COLUMNS="120")
    arguments = [*download_arguments(binary, root, origin, mode), *flags]
    master, slave = pty.openpty() if is_terminal else (None, None)
    captured = bytearray()
    process = subprocess.Popen(arguments, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
        stderr=slave if is_terminal else subprocess.PIPE, env=environment)
    reader = None
    if is_terminal:
        os.close(slave)
        reader = threading.Thread(target=drain_terminal, args=(master, captured), daemon=True)
        reader.start()
    try:
        if mode == "interrupt":
            assert ProgressScenario.content_started.wait(5), "content request did not start"
            process.send_signal(signal.SIGINT)
        output, errors = process.communicate(timeout=15)
        if reader:
            reader.join(timeout=2)
            assert not reader.is_alive(), "progress terminal did not close"
        return process.returncode, output.decode(), bytes(captured).decode() if is_terminal else errors.decode()
    finally:
        ProgressScenario.can_finish.set()
        if process.poll() is None:
            process.kill()
            process.wait(timeout=2)
        if master is not None:
            os.close(master)


def check_controls(binary, root, origin):
    cases = [([], True, True, True), (["--no-progress"], True, False, True),
             (["--quiet"], True, False, False), ([], False, False, False),
             (["--progress"], False, True, False),
             (["--list-echo", "--no-progress"], False, False, True),
             (["--no-list-echo"], True, True, False),
             (["--list-echo", "--quiet"], False, False, False),
             (["--list-echo", "--dry-run"], False, False, True),
             (["--no-list-echo", "--dry-run"], False, False, False),
             (["--list-echo", "--quiet", "--dry-run"], False, False, False),
             (["--list-echo", "--progress", "--dry-run"], False, False, True)]
    for index, (flags, is_terminal, is_transfer_visible, is_scan_visible) in enumerate(cases):
        selected = root / f"control-{index}"
        selected.mkdir()
        code, output, errors = invoke(binary, selected, origin, ("single", ["--output", "json", *flags], is_terminal))
        assert code == 0, (output, errors)
        has_transfer = "0/1" in errors or "1/1" in errors
        has_scan = "ve-adrive share download plan" in errors
        assert has_transfer == is_transfer_visible, ("unexpected transfer progress", flags, errors)
        assert has_scan == is_scan_visible, ("unexpected listing echo", flags, errors)
        assert "fixture-access-token" not in output + errors and "\x1b" not in output
        envelope = json.loads(output)
        if "--dry-run" in flags:
            assert envelope["data"]["selection"]["DownloadsIssued"] == 0
            assert not any("get_download_url" in path or urlsplit(path).path == "/content" for path in ProgressScenario.requests)
            assert not (selected / "out").exists()
            assert not (selected / "report.csv").exists() and not (selected / "manifest.csv").exists()
        else:
            assert envelope["data"]["Summary"] == {"Total": 1, "Completed": 1, "Failed": 0, "Pending": 0, "Skipped": 0}
        if index == 0:
            paths = [urlsplit(path).path for path in ProgressScenario.requests]
            assert paths == ["/v1/share_link/get_by_anonymous", "/v1/share_link/get_access_token",
                             "/v1/share_file/list", "/v1/share_file/get_download_url", "/content"], paths


def check_summary_and_resume(binary, root, origin):
    selected = root / "resume"
    selected.mkdir()
    code, output, _errors = invoke(binary, selected, origin, ("single", ["--output", "table"], True))
    assert code == 0 and "summary.succeeded" in output and "summary.skipped" in output
    assert "LocalPath" not in output and "SHA256" not in output, "default summary expanded file details"
    code, output, errors = invoke(binary, selected, origin, ("single", ["--resume", "--output", "json"], True))
    assert code == 0 and "ve-adrive share download" in errors
    assert json.loads(output)["data"]["Summary"]["Skipped"] == 1
    assert not any("get_download_url" in path for path in ProgressScenario.requests)


def check_denial(binary, root, origin):
    selected = root / "denial"
    selected.mkdir()
    code, output, errors = invoke(binary, selected, origin, ("denied", ["--output", "table"], True))
    assert code == 5 and "Forbidden.ShareDownloadDisabled" in output
    assert "data.summary.failed" in output and "data.summary.pending" in output
    assert "ve-adrive share download" in errors and "LocalPath" not in output
    assert not list((selected / "out").rglob("*.md"))


def table_values(output):
    fields = {}
    for line in output.splitlines():
        columns = line.split("|")
        if len(columns) == 4:
            fields[columns[1].strip()] = columns[2].strip()
    return fields


def check_recursive_success(binary, root, origin):
    selected = root / "recursive-success"
    selected.mkdir()
    code, output, errors = invoke(binary, selected, origin, ("tree-success", ["--output", "table"], True))
    assert code == 0 and table_values(output)["summary.succeeded"] == "2"
    assert (selected / "out/a.txt").read_bytes() == b"abc"
    assert (selected / "out/sub/b.txt").read_bytes() == b"abc"
    assert "0/2" in errors or "1/2" in errors, "recursive transfer progress was missing"


def check_invalid_resume(binary, root, origin):
    selected = root / "invalid-resume"
    selected.mkdir()
    code, _output, _errors = invoke(binary, selected, origin, ("single", ["--output", "json"], False))
    assert code == 0
    checkpoint = (selected / "checkpoint").read_bytes()
    ledger_path = selected / "checkpoint.completed/ledger.bin"
    ledger = ledger_path.read_bytes()
    destination = selected / "out/a.txt"
    destination.write_bytes(b"xyz")
    code, output, _errors = invoke(binary, selected, origin, ("single", ["--resume", "--output", "table"], True))
    fields = table_values(output)
    assert code != 0 and fields["data.summary.succeeded"] == fields["data.summary.skipped"] == "0"
    assert fields["data.summary.failed"] == "1"
    assert (selected / "checkpoint").read_bytes() == checkpoint and ledger_path.read_bytes() == ledger
    assert not any("get_download_url" in path for path in ProgressScenario.requests)
    destination.unlink()
    destination.symlink_to(selected / "missing-target")
    code, output, _errors = invoke(binary, selected, origin, ("single", ["--resume", "--output", "table"], True))
    fields = table_values(output)
    assert code != 0 and fields["data.summary.succeeded"] == fields["data.summary.skipped"] == "0"
    assert code == 9 and fields["ec"] == "Conflict"
    assert fields["data.summary.failed"] == "1" and fields["data.summary.pending"] == "0", fields
    assert destination.is_symlink() and not (selected / "missing-target").exists()
    assert (selected / "checkpoint").read_bytes() == checkpoint and ledger_path.read_bytes() == ledger
    assert not any("share_file/get" in path for path in ProgressScenario.requests)


def check_exact_and_interrupt(binary, root, origin):
    for mode, flags in [("single", ["--path", "a.txt"]), ("interrupt", [])]:
        selected = root / mode
        selected.mkdir()
        code, output, errors = invoke(binary, selected, origin, (mode, ["--output", "json", *flags], True))
        envelope = json.loads(output)
        if mode == "single":
            assert code == 0
            paths = [urlsplit(path).path for path in ProgressScenario.requests]
            assert paths == ["/v1/share_link/get_by_anonymous", "/v1/share_link/get_access_token",
                             "/v1/share_file/get", "/v1/share_file/get_download_url", "/content"], paths
        else:
            assert code != 0 and envelope["success"] is False
            assert "interrupted" in envelope["error"]["message"] and "ve-adrive share download" in errors
            assert not (selected / "out/a.txt").exists()
            if "\x1b[?25l" in errors:
                assert errors.rfind("\x1b[?25h") > errors.rfind("\x1b[?25l"), "cursor not restored"


def main():
    if os.name != "posix":
        print("SKIP: POSIX PTY needed; portable CLI/output contracts are covered separately")
        return
    repository = Path(__file__).resolve().parent.parent
    binaries = [Path(argument).resolve() for argument in sys.argv[1:]] or [
        repository / "target/debug/ve-storage-uni-cli",
        repository / "packaging/cargo/ve-adrive-cli/target/debug/ve-adrive-cli"]
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), ProgressScenario)
    server.daemon_threads = True
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    ProgressScenario.can_finish.set()
    try:
        origin = f"http://127.0.0.1:{server.server_port}"
        for binary in binaries:
            with tempfile.TemporaryDirectory(prefix="share-progress-", dir=Path(tempfile.gettempdir()).resolve()) as folder:
                check_controls(binary, Path(folder), origin)
                check_summary_and_resume(binary, Path(folder), origin)
                check_denial(binary, Path(folder), origin)
                check_recursive_success(binary, Path(folder), origin)
                check_invalid_resume(binary, Path(folder), origin)
                check_exact_and_interrupt(binary, Path(folder), origin)
        print(f"PASS: {len(binaries)} entrypoints; real TTY/pipe controls, summary/resume, denial and five-request single file")
    finally:
        ProgressScenario.can_finish.set()
        server.shutdown()
        server.server_close()
        worker.join()


if __name__ == "__main__":
    main()
