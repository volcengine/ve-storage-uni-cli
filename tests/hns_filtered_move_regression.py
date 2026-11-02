"""Verify filtered HNS moves preserve source directories and excluded files.

Uses real CLI binaries with a loopback HNS fixture and fake credentials.
"""
import csv
import json
from pathlib import Path
import shutil
import sys
import tempfile

from batch_transfer_breadth_regression import BreadthService
from directory_upload_regression import Fixture
from report_request_id_regression import report_rows


class HnsMoveService(BreadthService):
    def do_PUT(self):
        _, key, query = self.route()
        if "name" in query and "recursive" in query:
            self.server.renames.append((key, query["name"][0]))
            destination = query["name"][0]
            objects = {destination + name[len(key):]: body
                       for name, body in self.server.objects.items() if name.startswith(key)}
            for name in list(self.server.objects):
                if name.startswith(key):
                    del self.server.objects[name]
            self.server.objects.update(objects)
            self.report_key = key
            return self.respond()
        return super().do_PUT()

    def do_DELETE(self):
        _, key, _ = self.route()
        self.server.deleted.append(key)
        if key.endswith("/") and any(name != key and name.startswith(key) for name in self.server.objects):
            self.report_key = key
            return self.respond({"Code": "DirectoryNotEmpty", "Message": "directory has children"}, 409)
        return super().do_DELETE()


def seed(fixture):
    fixture.reset()
    shutil.rmtree(fixture.root / "download", ignore_errors=True)
    fixture.server.response_ids.clear()
    fixture.server.deleted, fixture.server.renames = [], []
    fixture.server.objects.update({"src/": b"", "src/sub/": b"", "src/empty/": b"",
        "src/a.txt": b"root", "src/b.bin": b"keep", "src/sub/中文.txt": b"nested",
        "src/sub/excluded.bin": b"keep nested"})


def arguments(fixture, direction, mode, filters):
    destination = fixture.destination("dst/") if direction == "remote" else str(fixture.root / "download")
    source = fixture.destination("src/")
    return ["mv", source, destination, "--recursive", "--force", "--confirm", source,
            "--checkpoint-dir", str(fixture.root / "checkpoints"), *mode, *filters]


def assert_selected_files(fixture, direction, is_include_parent):
    for key, body in {"a.txt": b"root", "sub/中文.txt": b"nested"}.items():
        relative = ("src/" if is_include_parent else "") + key
        actual = (fixture.server.objects["dst/" + relative] if direction == "remote"
                  else (fixture.root / "download" / relative).read_bytes())
        assert actual == body, (direction, key, actual)
        assert "src/" + key not in fixture.server.objects


def check_filtered(fixture, direction, mode, filters):
    seed(fixture)
    preserved = {key: body for key, body in fixture.server.objects.items()
                 if key.endswith("/") or key.endswith(".bin")}
    result = fixture.invoke(arguments(fixture, direction, mode, filters))
    envelope = json.loads(result.stdout)
    rows = report_rows(envelope)
    assert result.returncode == 0, (direction, mode, filters, result.stdout, result.stderr, rows)
    assert_selected_files(fixture, direction, "--include-parent" in mode)
    remaining = {key: body for key, body in fixture.server.objects.items() if key.startswith("src/")}
    assert remaining == preserved, (remaining, preserved)
    assert sorted(fixture.server.deleted) == ["src/a.txt", "src/sub/中文.txt"], fixture.server.deleted
    assert not fixture.server.renames, fixture.server.renames
    assert len(rows) == 4 and all(row["status"] == "succeeded" for row in rows), rows
    for row in rows:
        is_delete = row["operation"] == "delete-source"
        method = "DELETE" if is_delete else ("PUT" if direction == "remote" else "GET")
        remote = row["source"] if is_delete or direction == "local" else row["destination"]
        key = remote.removeprefix(fixture.destination(""))
        assert row["request_id"] == fixture.server.response_ids[(method, key)], row
    if envelope["data"].get("manifest_path"):
        with Path(envelope["data"]["manifest_path"]).open(newline="") as stream:
            plan = list(csv.DictReader(stream))
        assert len(plan) == 4 and not any(row["source"].endswith("/") for row in plan), plan


def check_unfiltered_rename(fixture):
    seed(fixture)
    before = dict(fixture.server.objects)
    result = fixture.invoke(arguments(fixture, "remote", (), ()))
    assert result.returncode == 0, (result.stdout, result.stderr)
    assert fixture.server.renames == [("src/", "dst/")], fixture.server.renames
    assert fixture.server.objects == {"dst/" + key[4:]: body for key, body in before.items()}
    assert not fixture.server.deleted, fixture.server.deleted


def check_zero_match(fixture, mode):
    seed(fixture)
    before = dict(fixture.server.objects)
    result = fixture.invoke(arguments(fixture, "remote", mode, ("--include", "*.absent")))
    assert result.returncode == 0, (result.stdout, result.stderr)
    assert fixture.server.objects == before
    assert not fixture.server.deleted and not fixture.server.renames
    assert report_rows(json.loads(result.stdout)) == []


def check_all_match(fixture, mode):
    seed(fixture)
    before = dict(fixture.server.objects)
    result = fixture.invoke(arguments(fixture, "remote", mode, ("--include", "*")))
    assert result.returncode == 0, (result.stdout, result.stderr)
    remaining = {key: body for key, body in fixture.server.objects.items() if key.startswith("src/")}
    assert remaining == {key: body for key, body in before.items() if key.endswith("/")}
    for key, body in before.items():
        if not key.endswith("/"):
            assert fixture.server.objects["dst/" + key[4:]] == body
    assert not any(key.endswith("/") for key in fixture.server.deleted), fixture.server.deleted
    assert not fixture.server.renames


def check_unfiltered_download(fixture, mode):
    seed(fixture)
    before = dict(fixture.server.objects)
    result = fixture.invoke(arguments(fixture, "local", mode, ()))
    assert result.returncode == 0, (result.stdout, result.stderr)
    assert not fixture.server.objects, fixture.server.objects
    for key, body in before.items():
        destination = fixture.root / "download" / key[4:]
        if key.endswith("/"):
            assert destination.is_dir(), destination
        else:
            assert destination.read_bytes() == body, key


def check_discovery(fixture):
    for language, expected in (("en", "filtered recursive moves select files only"), ("zh", "以 HNS 为源时")):
        result = fixture.invoke(["--language", language, "mv", "--help"])
        assert result.returncode == 0 and expected in " ".join(result.stdout.split()), result.stdout
        result = fixture.invoke(["--language", language, "mv", fixture.destination("src/"),
                                 fixture.destination("dst/"), "--recursive", "--describe"])
        assert result.returncode == 0, result.stderr
        assert expected in json.dumps(json.loads(result.stdout)["data"], ensure_ascii=False), result.stdout
        output_directory = fixture.root / ("skill-" + language)
        result = fixture.invoke(["skill", "export", "--language", language, "--name", "ve_tos_mv",
                                 "--dir", str(output_directory)])
        assert result.returncode == 0, (result.stdout, result.stderr)
        markdown = "\n".join(path.read_text() for path in output_directory.rglob("SKILL.md"))
        assert expected in markdown, markdown
    shipped_skill = Path(__file__).resolve().parents[1] / "skills/ve-tos-cli/SKILL.md"
    assert "filtered recursive moves select files only" in shipped_skill.read_text()


def check_binary(binary):
    with tempfile.TemporaryDirectory(prefix="hns-filtered-move-") as directory:
        fixture = Fixture(binary, "ve-tos", Path(directory))
        fixture.server.RequestHandlerClass = HnsMoveService
        fixture.server.response_ids = {}
        fixture.server.bucket_type = "hns"
        try:
            check_discovery(fixture)
            for direction in ("remote", "local"):
                for mode in ((), ("--no-manifest",)):
                    for filters in (("--include", "*.txt"), ("--exclude", "*.bin")):
                        check_filtered(fixture, direction, mode, filters)
            check_unfiltered_rename(fixture)
            for mode in ((), ("--no-manifest",)):
                check_zero_match(fixture, mode)
                check_all_match(fixture, mode)
                check_unfiltered_download(fixture, mode)
                check_filtered(fixture, "remote", (*mode, "--include-parent"), ("--include", "*.txt"))
            print(f"PASS: {binary.name}; HNS filtered moves preserve excluded files/directories; unfiltered rename")
        finally:
            fixture.close()


if __name__ == "__main__":
    assert len(sys.argv) > 1, "pass unified and/or ve-tos-cli binary paths"
    for argument in sys.argv[1:]:
        check_binary(Path(argument).resolve())
