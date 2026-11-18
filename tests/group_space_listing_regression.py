"""RootSpace pagination and presentation through actual ADrive CLI entrypoints."""
import csv
import http.server
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
from urllib.parse import parse_qs, urlsplit

from directory_upload_regression import fixture_environment
from share_discovery_regression import Rpc


TOKEN = "group-space-fixture-token"


def space(identifier, **fields):
    return {"SpaceID": identifier, "Name": identifier, "OwnerType": "group", **fields}


class Listing(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_arguments):
        pass

    def do_GET(self):
        parsed = urlsplit(self.path)
        query = parse_qs(parsed.query)
        self.server.requests.append((parsed.path, query))
        assert self.headers.get("Authorization") == f"Bearer {TOKEN}"
        assert parsed.path.endswith(("/mygroupspaces", "/myspaces")), parsed.path
        marker = query.get("marker", ["0"])[0]
        start = int(marker)
        limit = int(query["limit"][0])
        page_size = self.server.page_size or limit
        selected = self.server.spaces[start:start + min(limit, page_size)]
        if self.server.is_overfull:
            selected = self.server.spaces[start:start + 3]
        end = start + len(selected)
        next_marker = str(end) if end < len(self.server.spaces) else ""
        if self.server.is_cyclic:
            selected = [space("a" if marker == "0" else "b")] if self.server.cycle_has_rows else []
            next_marker = "1" if marker == "0" else "0"
        payload = {"Spaces": selected, "NextMarker": next_marker}
        if self.server.root is not None and parsed.path.endswith("/mygroupspaces") and (not self.server.root_markers or marker in self.server.root_markers):
            payload["RootSpace"] = self.server.root
        encoded = json.dumps(payload).encode()
        self.send_response(200)
        self.send_header("Content-Length", str(len(encoded)))
        self.send_header("x-ids-request-id", "group-space-fixture-request")
        self.end_headers()
        self.wfile.write(encoded)


def run(binary, root, arguments, output="json"):
    tool = ["ve-adrive"] if binary.name == "ve-storage-uni-cli" else []
    environment = fixture_environment()
    environment["ADRIVE_ACCESS_TOKEN"] = TOKEN
    result = subprocess.run(
        [str(binary), *tool, "--config-path", str(root / "config"), "--credentials-path",
         str(root / "credentials"), "--output", output, *arguments], env=environment,
        capture_output=True, text=True, timeout=15)
    assert TOKEN not in result.stdout + result.stderr
    return result


def invoke(binary, root, arguments, output="json", owner="group", instance="inst"):
    return run(binary, root, ["ls", f"adrive://{instance}", "--owner-type", owner, *arguments], output)


def envelope(binary, root, arguments):
    result = invoke(binary, root, arguments)
    parsed = json.loads(result.stdout or result.stderr)
    assert result.returncode == 0, parsed
    assert parsed["command"] == "ve-adrive ls" and parsed["success"], parsed
    assert parsed["pagination"]["total_returned"] == len(parsed["data"]["spaces"]), parsed
    return parsed


def traverse(binary, root, limit):
    identifiers, marker, root_count = [], None, 0
    for _ in range(20):
        arguments = ["--max-keys", str(limit)]
        if marker is not None:
            arguments += ["--marker", marker]
        parsed = envelope(binary, root, arguments)
        rows = parsed["data"]["spaces"]
        assert len(rows) <= limit, parsed
        identifiers += [row["space_id"] for row in rows]
        root_count += sum(row["is_root"] for row in rows)
        next_marker = parsed["data"]["next_marker"]
        assert bool(next_marker) == parsed["data"]["is_truncated"], parsed
        if not next_marker:
            assert not parsed["pagination"].get("next_marker"), parsed
            return identifiers, root_count
        assert next_marker != marker, parsed
        assert parsed["pagination"]["next_marker"] == next_marker, parsed
        marker = next_marker
    raise AssertionError("group Space traversal did not terminate")


def check_listing(binary, root, server):
    server.root, server.spaces = space("root", Name="root metadata"), [space("a"), space("b"), space("c")]
    parsed = envelope(binary, root, ["--max-keys", "2"])
    rows = parsed["data"]["spaces"]
    assert [row["space_id"] for row in rows] == ["root", "a"], parsed
    assert rows[0]["owner_type"] == "group" and rows[0]["is_root"], parsed
    assert not rows[1]["is_root"] and parsed["data"]["root_space"]["space_id"] == "root", parsed
    for limit in (1, 2, 1000):
        assert traverse(binary, root, limit) == (["root", "a", "b", "c"], 1)
    server.spaces = [space("a"), space("root", Name="complete root record"), space("b")]
    parsed = envelope(binary, root, ["--max-keys", "1000"])
    assert [row["space_id"] for row in parsed["data"]["spaces"]] == ["root", "a", "b"], parsed
    assert parsed["data"]["spaces"][0]["name"] == "complete root record", parsed
    server.spaces = [{"SpaceID": "root"}]
    server.root = space("root", Name="complete fallback", DisplayName="fallback label", OwnerId="owner", CreatedAt=42)
    parsed = envelope(binary, root, [])
    row = parsed["data"]["spaces"][0]
    assert row["name"] == "complete fallback" and row["owner_type"] == "group", row
    assert row["display_name"] == "fallback label" and row["owner_id"] == "owner" and row["created_at"] == 42, row
    server.spaces = []
    assert traverse(binary, root, 1) == (["root"], 1)
    server.root, server.spaces = None, [space("a"), space("b")]
    assert traverse(binary, root, 1) == (["a", "b"], 0)
    server.spaces = []
    assert traverse(binary, root, 1) == ([], 0)


def check_late_root(binary, root, server):
    server.root, server.page_size = space("root", Name="sparse root"), 1
    server.spaces = [space("root", Name="earlier complete root"), {"SpaceID": "root"}, space("a"), space("b")]
    server.root_markers = {"1", "2", "3"}
    parsed = envelope(binary, root, ["--max-keys", "1000"])
    rows = parsed["data"]["spaces"]
    assert [row["space_id"] for row in rows] == ["root", "a", "b"], parsed
    assert rows[0]["is_root"] and rows[0]["name"] == "earlier complete root", parsed
    server.root_markers = {"0"}
    server.spaces = [space("a"), space("root", Name="later complete root"), space("b")]
    parsed = envelope(binary, root, ["--max-keys", "1000"])
    assert parsed["data"]["spaces"][0]["name"] == "later complete root", parsed
    server.root_markers, server.page_size = set(), 0


def check_output(binary, root, server):
    server.root, server.spaces = space("root"), [space("a")]
    manifest = root / "manifest.csv"
    parsed = envelope(binary, root, ["--manifest-path", str(manifest)])
    assert parsed["data"]["spaces"][0]["owner_type"] == "group"
    with manifest.open() as handle:
        lines = [line for line in handle if not line.startswith("#")]
    assert [row["source"] for row in csv.DictReader(lines)] == ["adrive://inst/root", "adrive://inst/a"]
    for output in ("table", "csv", "markdown"):
        result = invoke(binary, root, ["--columns", "space_id,owner_type,is_root"], output)
        assert result.returncode == 0 and "root" in result.stdout and "Root" not in result.stdout, result
        if output == "csv":
            rows = list(csv.DictReader(result.stdout.splitlines()))
            assert rows[0]["space_id"] == "root" and rows[0]["owner_type"] == "root", rows
            assert rows[1]["owner_type"] == "group", rows
    result = invoke(binary, root, ["--query", "data.spaces[?is_root].owner_type"])
    assert result.returncode == 0 and json.loads(result.stdout) == ["group"], result



def check_pagination(binary, root, server):
    server.root = space("root")
    server.spaces = [space(identifier) for identifier in ("a", "root", "b", "c", "root", "d")]
    for page_size in (0, 1):
        server.page_size = page_size
        for limit in (1, 2, 3, 1000):
            assert traverse(binary, root, limit) == (["root", "a", "b", "c", "d"], 1)
    server.page_size = 0
    server.is_overfull = True
    for limit in (1, 2, 1000):
        assert traverse(binary, root, limit) == (["root", "a", "b", "c", "d"], 1)
    server.is_overfull = False
    server.spaces = [space("a"), space("b"), space("c")]
    first = envelope(binary, root, ["--max-keys", "1"])
    marker = first["data"]["next_marker"]
    second = envelope(binary, root, ["--max-keys", "2", "--marker", marker])
    assert [row["space_id"] for row in second["data"]["spaces"]] == ["a", "b"], second
    assert marker.startswith("adrive-group-spaces:v1:") and TOKEN not in marker
    legacy = envelope(binary, root, ["--marker", "1"])
    assert [row["space_id"] for row in legacy["data"]["spaces"]] == ["root", "b", "c"], legacy
    user = invoke(binary, root, ["--max-keys", "1"], owner="user")
    parsed = json.loads(user.stdout)
    assert user.returncode == 0 and parsed["data"]["next_marker"] == "1", parsed
    assert [row["space_id"] for row in parsed["data"]["spaces"]] == ["a"], parsed
    assert parsed["data"]["spaces"][0]["is_root"] is False
    return marker


def check_invalid_cursors(binary, root, server, marker):
    cases = [
        ("adrive-group-spaces:v1:not-hex", "group", "inst"),
        ("adrive-group-spaces:v2:00", "group", "inst"),
        (marker, "group", "other-instance"),
        (marker, "user", "inst"),
    ]
    state = json.loads(bytes.fromhex(marker.split(":", 2)[2]))
    for update in ({"offset": 1, "limit": None}, {"limit": 0}, {"root_id": ""}, {"unexpected": True},
                   {"cycle": {"checkpoint": None, "power": 0, "distance": 0}},
                   {"cycle": {"checkpoint": None, "power": 2, "distance": 2}}):
        encoded = json.dumps({**state, **update}).encode().hex()
        cases.append(("adrive-group-spaces:v1:" + encoded, "group", "inst"))
    for cursor, owner, instance in cases:
        before = len(server.requests)
        result = invoke(binary, root, ["--marker", cursor], owner=owner, instance=instance)
        assert result.returncode != 0 and not json.loads(result.stdout or result.stderr)["success"], result
        assert len(server.requests) == before, (cursor, server.requests[before:])
    server.is_cyclic = True
    before = len(server.requests)
    result = invoke(binary, root, ["--max-keys", "1000"])
    assert result.returncode != 0 and "cyclic" in result.stdout + result.stderr, result
    assert len(server.requests) - before <= 3
    server.cycle_has_rows = True
    marker = None
    for _ in range(8):
        arguments = ["--max-keys", "1"] + (["--marker", marker] if marker else [])
        result = invoke(binary, root, arguments)
        if result.returncode:
            assert "cyclic" in result.stdout + result.stderr, result
            break
        marker = json.loads(result.stdout)["data"]["next_marker"]
        assert marker
    else:
        raise AssertionError("cyclic pagination survived across CLI continuation calls")
    server.is_cyclic, server.cycle_has_rows = False, False
    server.spaces = []
    state.update(offset=10, limit=1)
    changed = "adrive-group-spaces:v1:" + json.dumps(state).encode().hex()
    result = invoke(binary, root, ["--marker", changed])
    assert result.returncode != 0 and "restart listing" in result.stdout + result.stderr, result


def check_discovery(binary, root, server):
    before = len(server.requests)
    for language in ("en", "zh"):
        help_result = run(binary, root, ["--language", language, "ls", "--help"])
        assert help_result.returncode == 0 and "is_root" in help_result.stdout, help_result
        assert "--max-keys 1" in help_result.stdout and "data.spaces" in help_result.stdout
        described = run(binary, root, ["--language", language, "--describe", "ls"])
        assert described.returncode == 0 and "is_root" in described.stdout, described
        exported = root / f"skill-{language}"
        result = run(binary, root, ["skill", "export", "--name", "ve_adrive_ls",
                                    "--dir", str(exported), "--language", language])
        assert result.returncode == 0, result
        markdown = next(exported.glob("*/ve_adrive_ls/SKILL.md")).read_text()
        assert "is_root" in markdown and "data.spaces" in markdown and "--max-keys 1" in markdown
    rpc = Rpc(binary, root)
    try:
        tools = rpc.call(2, "tools/list", {})["tools"]
        listing = next(tool for tool in tools if tool["name"] == "ve_adrive_ls")
        description = listing["inputSchema"]["properties"]["owner-type"]["description"]
        assert "is_root" in description and "--max-keys 1" in description, listing
    finally:
        rpc.close()
    assert len(server.requests) == before


def check_binary(binary):
    with tempfile.TemporaryDirectory(prefix="group-space-listing-") as directory:
        root = Path(directory)
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Listing)
        server.requests, server.page_size = [], 0
        server.root, server.spaces = None, []
        server.root_markers = set()
        server.is_cyclic, server.is_overfull, server.cycle_has_rows = False, False, False
        worker = threading.Thread(target=server.serve_forever, daemon=True)
        worker.start()
        config = f"[default.adrive]\nauth_mode='oauth'\nendpoint='http://127.0.0.1:{server.server_port}'\nregion='test-region'\n"
        (root / "config").write_text(config)
        (root / "credentials").write_text("schema_version=1\n")
        try:
            check_listing(binary, root, server)
            check_late_root(binary, root, server)
            check_output(binary, root, server)
            marker = check_pagination(binary, root, server)
            check_invalid_cursors(binary, root, server, marker)
            check_discovery(binary, root, server)
            assert (root / "config").read_text() == config
            assert (root / "credentials").read_text() == "schema_version=1\n"
            print(f"PASS: {binary.name}; RootSpace rows, strict pagination, raw ownership and root display")
        finally:
            server.shutdown()
            server.server_close()
            worker.join()


if __name__ == "__main__":
    assert len(sys.argv) > 1, "pass the unified and/or standalone binary paths"
    for argument in sys.argv[1:]:
        check_binary(Path(argument).resolve())
