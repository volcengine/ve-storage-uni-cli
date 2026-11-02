"""Exercise batch share downloads through real CLI binaries and localhost HTTP.

Run: python3 tests/share_batch_download_regression.py [unified-binary] [standalone-binary]
Every invocation isolates reports, manifests, checkpoints, config and credentials.
"""
from contextlib import contextmanager
import csv
from dataclasses import dataclass, field
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
import traceback
from urllib.parse import parse_qs, urlsplit


REMOTE_ROOT = "remote/tree/"
REMOTE_MILLIS = 1_700_000_000_000
CONTENTS = {"a.txt": b"AAA", "b.txt": b"BBB"}


@dataclass
class Scenario:
    barrier_parties: int = 0
    failure_status: int = 0
    missing_content_ids: frozenset = frozenset()
    is_unknown_issuance: bool = False
    is_stalled: bool = False
    directory_mode: str = ""
    remote_millis: int = REMOTE_MILLIS
    requests: list = field(default_factory=list)
    lock: threading.Lock = field(default_factory=threading.Lock)
    second_started: threading.Event = field(default_factory=threading.Event)
    release: threading.Event = field(default_factory=threading.Event)
    active: int = 0
    peak: int = 0
    has_broken_barrier: bool = False

    def __post_init__(self):
        self.barrier = threading.Barrier(self.barrier_parties) if self.barrier_parties > 1 else None

    def record(self, kind, name=""):
        with self.lock:
            self.requests.append((kind, name))

    def issued(self):
        with self.lock:
            return [name for kind, name in self.requests if kind == "issue"]


def metadata(path, modified_millis=REMOTE_MILLIS):
    if path.endswith("/"):
        return {"FilePath": path, "FileType": "folder", "UpdatedAt": modified_millis}
    name = path.removeprefix(REMOTE_ROOT).rsplit("/", 1)[-1]
    assert name in CONTENTS, path
    return {"FilePath": path, "FileType": "file", "Size": len(CONTENTS[name]),
            "ETag": f"etag-{name}", "UpdatedAt": modified_millis}


class Receiver(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_arguments):
        pass

    def respond(self, value, status=200, request_id=None):
        encoded = json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Length", str(len(encoded)))
        if request_id is not None:
            self.send_header("x-ids-request-id", request_id)
        self.end_headers()
        self.wfile.write(encoded)

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
        assert self.path == "/v1/share_link/get_access_token" and body["ShareID"] == "s"
        assert not self.headers.get("Authorization")
        self.server.scenario.record("token")
        self.respond({"AccessToken": "batch-fixture-share-token", "ExpiresIn": 600}, request_id="token-id")

    def do_GET(self):
        parsed = urlsplit(self.path)
        query = parse_qs(parsed.query)
        scenario = self.server.scenario
        if parsed.path == "/v1/share_link/get_by_anonymous":
            scenario.record("info")
            return self.respond({"ShareID": "s", "FilePathList": [REMOTE_ROOT]}, request_id="info-id")
        if parsed.path.startswith("/content/"):
            return self.content(parsed.path.removeprefix("/content/"))
        assert not self.headers.get("Authorization")
        assert self.headers.get("x-share-token") == "batch-fixture-share-token"
        if parsed.path == "/v1/share_file/get":
            path = query["file_path"][0]
            scenario.record("get", path)
            return self.respond(metadata(path, scenario.remote_millis), request_id=f"metadata-{path.rsplit('/', 1)[-1]}")
        if parsed.path == "/v1/share_file/list":
            return self.listing(query)
        assert parsed.path == "/v1/share_file/get_download_url", parsed.path
        return self.issue(query["file_path"][0])

    def issue(self, path):
        relative = path.removeprefix(REMOTE_ROOT)
        name = relative.rsplit("/", 1)[-1]
        assert name in CONTENTS
        self.server.scenario.record("issue", relative)
        if self.server.scenario.is_unknown_issuance and name == "a.txt":
            return self.respond({"Code": "InternalError"}, 503, "issue-a.txt")
        origin = f"http://127.0.0.1:{self.server.server_port}"
        self.respond({"DownloadURL": f"{origin}/content/{name}?signature=synthetic-capability"},
                     request_id=f"issue-{name}")

    def listing(self, query):
        assert query["delimiter"] == ["/"]
        prefix = query["prefix"][0]
        scenario = self.server.scenario
        scenario.record("list", prefix)
        files, folders = [], []
        if prefix == REMOTE_ROOT:
            names = CONTENTS if not scenario.directory_mode else ["b.txt"]
            files = [metadata(REMOTE_ROOT + name, scenario.remote_millis) for name in names]
            if scenario.directory_mode:
                folder = "sub" if scenario.directory_mode == "nested" else "empty"
                folders = [{"Folder": f"{REMOTE_ROOT}{folder}/"}]
        elif prefix == REMOTE_ROOT + "sub/":
            files = [metadata(prefix + "a.txt", scenario.remote_millis)]
        else:
            assert prefix == REMOTE_ROOT + "empty/", prefix
        self.respond({"Files": files, "Folders": folders, "NextMarker": "", "IsTruncated": False},
                     request_id="listing-id")

    def content(self, name):
        assert name in CONTENTS
        assert not self.headers.get("Authorization") and not self.headers.get("x-share-token")
        scenario = self.server.scenario
        scenario.record("content", name)
        with scenario.lock:
            scenario.active += 1
            scenario.peak = max(scenario.peak, scenario.active)
        try:
            wait_for_content(scenario, name)
            if scenario.failure_status and name == "a.txt":
                request_id = None if name in scenario.missing_content_ids else f"content-{name}"
                return self.respond({"Code": "SyntheticContentFailure"}, scenario.failure_status, request_id)
            self.send_response(200)
            self.send_header("Content-Length", str(len(CONTENTS[name])))
            self.send_header("ETag", f'"etag-{name}"')
            if name not in scenario.missing_content_ids:
                self.send_header("x-ids-request-id", f"content-{name}")
            self.end_headers()
            if scenario.is_stalled and name == "b.txt":
                scenario.second_started.set()
                scenario.release.wait(10)
            self.wfile.write(CONTENTS[name])
        except (BrokenPipeError, ConnectionResetError):
            # Cancellation intentionally closes an in-flight synthetic content response.
            if not scenario.is_stalled:
                raise
        finally:
            with scenario.lock:
                scenario.active -= 1


def wait_for_content(scenario, name):
    if scenario.barrier is not None:
        try:
            scenario.barrier.wait(timeout=3)
        except threading.BrokenBarrierError:
            scenario.has_broken_barrier = True
    time.sleep(0.05 if name == "a.txt" else 0.1)


@contextmanager
def fixture(scenario):
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Receiver)
    server.scenario = scenario
    worker = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.02}, daemon=True)
    worker.start()
    try:
        yield f"http://127.0.0.1:{server.server_port}"
    finally:
        scenario.release.set()
        server.shutdown()
        server.server_close()
        worker.join(timeout=5)
        assert not worker.is_alive()


def environment():
    selected = {key: value for key, value in os.environ.items() if not key.startswith(("ADRIVE_", "TOS_"))}
    selected["TOS_LAST_REQUEST_ID"] = "synthetic-stale-global-id"
    return selected


def arguments(binary, origin, root, options=()):
    prefix = [] if binary.stem == "ve-adrive-cli" else ["ve-adrive"]
    return [str(binary), *prefix, "--config-path", str(root / "config"),
        "--credentials-path", str(root / "missing-credentials"), "--output", "json",
        "share", "download", f"{origin}/share/s", str(root / "out"), "--path", REMOTE_ROOT,
        "--recursive", "--report-path", str(root / "report.csv"),
        "--manifest-path", str(root / "manifest.csv"), "--checkpoint-dir", str(root / "checkpoints"),
        "--no-progress", "--no-list-echo", *options]


def invoke(binary, origin, root, options=()):
    result = subprocess.run(arguments(binary, origin, root, options), env=environment(), cwd=root,
                            text=True, capture_output=True, timeout=15)
    encoded = result.stdout or result.stderr
    envelope = json.loads(encoded)
    for synthetic_secret in ("batch-fixture-share-token", "signature=synthetic-capability"):
        assert synthetic_secret not in result.stdout + result.stderr
    return result, envelope


def report(root):
    with (root / "report.csv").open(newline="") as source:
        rows = list(csv.DictReader(source))
    assert all(row["command"] in ("ve-adrive share download", "ve-adrive-cli share download") for row in rows)
    assert len(rows) == len({row["source"] for row in rows}), "duplicate terminal report rows"
    return {row["source"].rsplit("/", 1)[-1]: row for row in rows}


def succeeded(root, name, parent=""):
    path = root / "out" / parent / name
    assert path.is_file(), f"independent file was not published: {path}"
    assert path.read_bytes() == CONTENTS[name]


def check_concurrency(binary, root, concurrency):
    expected = 1 if concurrency == 1 else 2
    scenario = Scenario(barrier_parties=expected)
    options = [] if concurrency is None else ["--batch-concurrency", str(concurrency)]
    with fixture(scenario) as origin:
        result, envelope = invoke(binary, origin, root, options)
    assert result.returncode == 0, envelope
    assert scenario.peak == expected and not scenario.has_broken_barrier, (scenario.peak, expected)
    assert sorted(scenario.issued()) == sorted(CONTENTS)
    rows = report(root)
    for name in CONTENTS:
        succeeded(root, name)
        assert rows[name]["status"] == "succeeded" and rows[name]["request_id"] == f"content-{name}"


def check_known_content_failure(binary, root, status, has_request_id=True):
    missing = frozenset() if has_request_id else frozenset({"a.txt"})
    scenario = Scenario(failure_status=status, missing_content_ids=missing)
    with fixture(scenario) as origin:
        result, envelope = invoke(binary, origin, root, ["--batch-concurrency", "1"])
    assert result.returncode != 0 and envelope["success"] is False, envelope
    assert envelope["data"]["Summary"]["Failed"] == 1
    assert envelope["data"]["Summary"]["Completed"] == 1
    assert envelope["data"]["IssuanceOutcomeUnknown"] is False
    assert scenario.issued() == ["a.txt", "b.txt"], scenario.requests
    assert not (root / "out/a.txt").exists()
    succeeded(root, "b.txt")
    rows = report(root)
    assert rows["a.txt"]["status"] == "failed" and rows["b.txt"]["status"] == "succeeded"
    expected_id = "content-a.txt" if has_request_id else ""
    assert rows["a.txt"]["request_id"] == expected_id, rows
    assert rows["b.txt"]["request_id"] == "content-b.txt", rows
    assert envelope["request_id"] == (expected_id or None), envelope


def check_unknown_issuance(binary, root):
    scenario = Scenario(is_unknown_issuance=True)
    with fixture(scenario) as origin:
        result, envelope = invoke(binary, origin, root, ["--batch-concurrency", "1"])
    assert result.returncode != 0 and envelope["data"]["IssuanceOutcomeUnknown"] is True, envelope
    assert scenario.issued() == ["a.txt"], scenario.requests
    assert not any(kind == "content" for kind, _ in scenario.requests)
    assert all(not (root / "out" / name).exists() for name in CONTENTS)
    assert envelope["data"]["Summary"]["Pending"] == 1
    files = {item["LocalPath"]: item for item in envelope["data"]["Files"]}
    assert files["b.txt"]["Status"] == "pending"
    rows = report(root)
    assert set(rows) == {"a.txt"} and rows["a.txt"]["status"] == "failed"
    assert rows["a.txt"]["request_id"] == "issue-a.txt"


def check_success_without_content_id(binary, root):
    scenario = Scenario(barrier_parties=2, missing_content_ids=frozenset({"b.txt"}))
    with fixture(scenario) as origin:
        result, envelope = invoke(binary, origin, root, ["--batch-concurrency", "2"])
    assert result.returncode == 0, envelope
    rows = report(root)
    assert rows["a.txt"]["request_id"] == "content-a.txt"
    assert rows["b.txt"]["request_id"] == "" and rows["b.txt"]["status"] == "succeeded", rows
    selected = {item["LocalPath"]: item for item in envelope["data"]["Files"]}
    assert selected["b.txt"].get("RequestID") is None, selected
    succeeded(root, "a.txt")
    succeeded(root, "b.txt")


def existing(root, name, modified_seconds=None, parent=""):
    path = root / "out" / parent / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b"old-local-content")
    if modified_seconds is not None:
        os.utime(path, (modified_seconds, modified_seconds))


def check_skip_policy(binary, root, is_newer):
    existing(root, "a.txt", REMOTE_MILLIS / 1000 + 100)
    if is_newer:
        existing(root, "b.txt", REMOTE_MILLIS / 1000 - 100)
    options = ["--overwrite-strategy", "newer"] if is_newer else ["--no-clobber"]
    scenario = Scenario()
    with fixture(scenario) as origin:
        result, envelope = invoke(binary, origin, root, options)
    assert result.returncode == 0, envelope
    assert scenario.issued() == ["b.txt"], scenario.requests
    assert (root / "out/a.txt").read_bytes() == b"old-local-content"
    succeeded(root, "b.txt")
    assert envelope["data"]["Summary"]["Skipped"] == 1
    rows = report(root)
    assert rows["a.txt"]["status"] == "skipped" and rows["a.txt"]["request_id"] == ""
    assert rows["b.txt"]["status"] == "succeeded" and rows["b.txt"]["request_id"] == "content-b.txt"


def check_newer_timestamp_boundary(binary, root, remote_millis):
    for name in CONTENTS:
        existing(root, name, REMOTE_MILLIS / 1000 + 100)
    before = snapshot(root)
    scenario = Scenario(remote_millis=remote_millis)
    with fixture(scenario) as origin:
        planned, plan = invoke(binary, origin, root, ["--overwrite-strategy", "newer", "--dry-run"])
        assert planned.returncode == 0, plan
        selection = plan["data"]["selection"]
        assert selection["SkippedCount"] == 2 and all(item["Action"] == "skip"
            for item in selection["Manifest"]["Files"]), selection
        assert snapshot(root) == before
        result, envelope = invoke(binary, origin, root, ["--overwrite-strategy", "newer"])
    assert result.returncode == 0 and envelope["data"]["Summary"]["Skipped"] == 2, envelope
    assert not scenario.issued(), scenario.requests
    rows = report(root)
    for name in CONTENTS:
        assert (root / "out" / name).read_bytes() == b"old-local-content"
        assert rows[name]["status"] == "skipped" and rows[name]["request_id"] == ""


def check_force_and_mapping(binary, root, is_include_parent):
    parent = "tree" if is_include_parent else ""
    existing(root, "a.txt", REMOTE_MILLIS / 1000 + 100, parent)
    options = ["--include-parent"] if is_include_parent else []
    with fixture(Scenario()) as origin:
        result, envelope = invoke(binary, origin, root, options)
    assert result.returncode == 0 and envelope["data"]["Summary"]["Skipped"] == 0, envelope
    succeeded(root, "a.txt", parent)
    succeeded(root, "b.txt", parent)
    assert not (root / "out/remote").exists()
    assert is_include_parent or not (root / "out/tree").exists()


def check_local_directory_failure(binary, root, is_empty):
    name = "empty" if is_empty else "sub"
    (root / "out").mkdir()
    (root / "out" / name).write_bytes(b"ordinary-local-file")
    scenario = Scenario(directory_mode="empty" if is_empty else "nested")
    with fixture(scenario) as origin:
        result, envelope = invoke(binary, origin, root, ["--batch-concurrency", "1"])
    assert result.returncode == 9 and envelope["success"] is False, envelope
    succeeded(root, "b.txt")
    assert scenario.issued() == ["b.txt"], scenario.requests
    assert (root / "out" / name).read_bytes() == b"ordinary-local-file"
    assert envelope["data"]["Summary"]["Completed"] == 1
    failures = envelope["data"]["DirectoryFailures"]
    assert len(failures) == 1 and failures[0]["LocalPath"] == name, failures
    rows = report(root)
    assert rows["b.txt"]["status"] == "succeeded" and rows["b.txt"]["request_id"] == "content-b.txt"
    directory = rows[""]
    assert directory["operation"] == "mkdir" and directory["status"] == "failed" and directory["request_id"] == ""
    if not is_empty:
        assert rows["a.txt"]["status"] == "failed" and rows["a.txt"]["request_id"] == ""
        selected = {item["LocalPath"]: item for item in envelope["data"]["Files"]}
        assert selected["sub/a.txt"]["Status"] == "failed" and selected["sub/a.txt"].get("RequestID") is None


def snapshot(root):
    return {str(path.relative_to(root)): ("directory" if path.is_dir() else
        (path.read_bytes(), path.stat().st_mtime_ns)) for path in root.rglob("*")}


def check_dry_run(binary, root, policy):
    options = ["--checkpoint", str(root / "checkpoint.json"), "--dry-run"]
    expected = {"a.txt": "create", "b.txt": "create"}
    if policy in ("no-clobber", "newer", "force"):
        existing(root, "a.txt", REMOTE_MILLIS / 1000 + 100)
        expected["a.txt"] = "overwrite" if policy == "force" else "skip"
    if policy == "no-clobber":
        options += ["--no-clobber"]
    elif policy == "newer":
        existing(root, "b.txt", REMOTE_MILLIS / 1000 - 100)
        options += ["--overwrite-strategy", "newer"]
        expected["b.txt"] = "overwrite"
    elif policy == "include-parent":
        options += ["--include-parent"]
        expected = {f"tree/{name}": action for name, action in expected.items()}
    before = snapshot(root)
    scenario = Scenario()
    with fixture(scenario) as origin:
        result, envelope = invoke(binary, origin, root, options)
    assert result.returncode == 0, envelope
    selection = envelope["data"]["selection"]
    actual = {item["LocalPath"]: item["Action"] for item in selection["Manifest"]["Files"]}
    assert actual == expected, actual
    assert selection["DownloadsIssued"] == 0 and selection["LocalWrites"] is False
    assert not scenario.issued() and not any(kind == "content" for kind, _ in scenario.requests)
    assert snapshot(root) == before
    assert not (root / "report.csv").exists() and not (root / "manifest.csv").exists()
    assert not (root / "checkpoints").exists()


def wait_until(predicate, timeout=6):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.02)
    raise AssertionError("timed out waiting for the first successful publication")


def check_interrupt(binary, root):
    scenario = Scenario(is_stalled=True)
    with fixture(scenario) as origin:
        process = subprocess.Popen(arguments(binary, origin, root,
            ["--batch-concurrency", "1", "--checkpoint", str(root / "checkpoint.json")]), env=environment(), cwd=root,
            text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            assert scenario.second_started.wait(6), "second content GET did not start"
            wait_until(lambda: (root / "out/a.txt").exists())
            process.send_signal(signal.SIGINT)
            stdout, stderr = process.communicate(timeout=6)
            envelope = json.loads(stdout or stderr)
            assert process.returncode != 0 and envelope["success"] is False, envelope
            assert envelope["data"]["Summary"]["Completed"] == 1
            assert envelope["data"]["CheckpointAvailable"] is True
            succeeded(root, "a.txt")
            assert not (root / "out/b.txt").exists()
            assert report(root)["a.txt"]["status"] == "succeeded"
            scenario.is_stalled = False
            scenario.release.set()
            before = len(scenario.issued())
            resumed, completed = invoke(binary, origin, root, ["--batch-concurrency", "1",
                "--checkpoint", str(root / "checkpoint.json"), "--resume"])
            assert resumed.returncode == 0 and completed["data"]["Summary"]["Skipped"] == 1, completed
            assert scenario.issued()[before:] == ["b.txt"], scenario.requests
            succeeded(root, "b.txt")
        finally:
            scenario.release.set()
            if process.poll() is None:
                process.kill()
                process.communicate(timeout=5)


def cases():
    yield from ((f"batch-{value}", lambda binary, root, value=value: check_concurrency(binary, root, value))
                for value in (None, 2, 1))
    yield from ((f"content-{status}", lambda binary, root, status=status: check_known_content_failure(binary, root, status))
                for status in (403, 404))
    yield "missing-request-id", lambda binary, root: check_known_content_failure(binary, root, 403, False)
    yield "successful-missing-request-id", check_success_without_content_id
    yield "unknown-issuance", check_unknown_issuance
    yield "interrupt", check_interrupt
    yield "no-clobber", lambda binary, root: check_skip_policy(binary, root, False)
    yield "newer", lambda binary, root: check_skip_policy(binary, root, True)
    yield from ((f"newer-timestamp-{value}", lambda binary, root, value=value: check_newer_timestamp_boundary(binary, root, value))
                for value in (0, -1))
    yield "force-contents", lambda binary, root: check_force_and_mapping(binary, root, False)
    yield "force-include-parent", lambda binary, root: check_force_and_mapping(binary, root, True)
    yield "local-parent-failure", lambda binary, root: check_local_directory_failure(binary, root, False)
    yield "local-empty-directory-failure", lambda binary, root: check_local_directory_failure(binary, root, True)
    yield from ((f"dry-run-{policy}", lambda binary, root, policy=policy: check_dry_run(binary, root, policy))
                for policy in ("create", "no-clobber", "newer", "force", "include-parent"))


def main():
    repository = Path(__file__).resolve().parent.parent
    binaries = [Path(value).resolve() for value in sys.argv[1:]] or [
        repository / "target/debug/ve-storage-uni-cli",
        repository / "packaging/cargo/ve-adrive-cli/target/debug/ve-adrive-cli"]
    failures = []
    count = 0
    for binary in binaries:
        for name, check in cases():
            count += 1
            with tempfile.TemporaryDirectory(prefix=f"share-batch-{name}-") as directory:
                try:
                    root = Path(directory)
                    (root / "config").write_text("[default.adrive]\n")
                    check(binary, root)
                except Exception as error:
                    location = traceback.extract_tb(error.__traceback__)[-1]
                    failures.append(f"{binary.name}/{name}: line {location.lineno}: {location.line}: {error}")
    assert not failures, "\n" + "\n".join(failures)
    print(f"PASS: {len(binaries)} entrypoints, {count} batch/overwrite/cancellation/read-only scenarios")


if __name__ == "__main__":
    main()
