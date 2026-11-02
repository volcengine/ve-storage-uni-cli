"""Exercise share files listing options against ls with local HTTP fixtures.

Run: python3 tests/share_files_options_regression.py [binary ...]
"""
import csv
import io
import json
import subprocess
from urllib.parse import parse_qs, urlsplit

import share_files_display_regression as fixture


fixture.CASES["options"] = {
    "Files": [
        {"FilePath": "docs/z.csv", "Size": 2048, "UpdatedAt": 20, "ETag": "z-tag"},
        {"FilePath": "docs/a.txt", "Size": 1024, "UpdatedAt": 30, "HashCRC64ECMA": 42},
        {"FilePath": "docs/negative.txt", "Size": -1, "UpdatedAt": 10},
    ],
    "Folders": [{"Folder": "docs/sub/", "UpdatedAt": 40}],
}
fixture.CASES["structured"] = {
    "Files": list(fixture.CASES["options"]["Files"]),
    "Folders": [{"Folder": "docs/sub/", "UpdatedAt": 40},
                {"Folder": "docs/alpha/", "UpdatedAt": 50}],
}


def options_parity(binary, root, origin):
    for arguments in [
        ["-H"],
        ["--human-readable", "--columns", "size,file_path"],
        ["--columns", " , "],
        ["--columns", "unknown,file_path"],
        ["--sort", "size"],
        ["--sort", "key"],
        ["--sort", "name"],
        ["--sort", "last-modified"],
        ["--sort", "mtime"],
    ]:
        for output in ("csv", "table"):
            shared = fixture.invoke(binary, root, [
                *fixture.share_arguments(origin, "options"), *arguments], output)
            ordinary = fixture.invoke(binary, root, [
                "ls", "adrive://inst/options/docs/", *arguments], output)
            assert shared == ordinary, (arguments, output, shared, ordinary)


def raw_metadata_survives_display_flags(binary, root, origin):
    arguments = [*fixture.share_arguments(origin, "options"), "-H",
                 "--columns", "size", "--sort", "size"]
    envelope = json.loads(fixture.invoke(binary, root, arguments, "json"))
    listing = envelope["data"]
    assert "entries" not in listing
    assert [entry["Size"] for entry in listing["Files"]] == [-1, 1024, 2048]
    selected = fixture.invoke(binary, root, arguments, "csv",
                              "data.Files[].{path:FilePath,bytes:Size}")
    assert "2048,docs/z.csv" in selected and "-1,docs/negative.txt" in selected
    exact = fixture.invoke(binary, root, [
        *fixture.share_arguments(origin, "mixed", "docs/report.txt"), "-H",
        "--columns", "size,file_path"], "csv")
    assert exact == "size,file_path\n12 B,docs/report.txt\n", exact


def manifest_is_explicit_and_private(binary, root, origin):
    before = set(root.iterdir())
    fixture.invoke(binary, root, fixture.share_arguments(origin, "options"))
    assert set(root.iterdir()) == before
    manifest = root / "listing" / "manifest.csv"
    arguments = [*fixture.share_arguments(origin, "options"), "--manifest-path", str(manifest),
                 "--human-readable", "--columns", "size"]
    arguments[2] += "?pwd=fixture-url-secret"
    raw = json.loads(fixture.invoke(binary, root, arguments, "json"))
    assert raw["data"]["manifest_path"] == str(manifest)
    encoded = manifest.read_text()
    assert "fixture-url-secret" not in encoded and "fixture-share-token" not in encoded
    rows = list(csv.DictReader(io.StringIO(encoded)))
    assert list(rows[0]) == ["command", "operation", "source", "destination", "size", "etag", "crc64"]
    assert len(rows) == 4 and [row["size"] for row in rows] == ["2048", "1024", "0", "0"]
    assert all(row["command"] == "ve-adrive share files" and row["operation"] == "list" for row in rows)
    assert rows[0]["etag"] == "z-tag" and rows[1]["crc64"] == "42"
    assert all(row["source"].startswith(f"{origin}/share/options?path=") for row in rows)


def manifests_preserve_empty_folders_and_exact_selection(binary, root, origin):
    for case, path, expected in [
        ("empty", "docs/", []),
        ("folders", "docs/", ["docs/empty/"]),
        ("markers", "docs/", ["docs/report.txt", "docs/sub/", "docs/empty/"]),
        ("mixed", "docs/report.txt", ["docs/report.txt"]),
    ]:
        manifest = root / f"{case}-manifest.csv"
        fixture.invoke(binary, root, [*fixture.share_arguments(origin, case, path),
            "--manifest-path", str(manifest)], "json")
        rows = list(csv.DictReader(io.StringIO(manifest.read_text())))
        paths = [parse_qs(urlsplit(row["source"]).query)["path"][0] for row in rows]
        assert paths == expected, (case, paths)


def readonly_paths_never_write(binary, root, origin):
    for mode in ("--dry-run", "--help", "--describe"):
        manifest = root / mode.removeprefix("--") / "manifest.csv"
        before = len(fixture.Listing.requests)
        fixture.invoke(binary, root, [*fixture.share_arguments(origin, "options"), "-H",
            "--columns", "file_path,size", "--sort", "size", "--manifest-path", str(manifest), mode], "json")
        assert not manifest.parent.exists(), mode
        assert len(fixture.Listing.requests) == before, mode
    blocked = root / "blocked"
    blocked.write_text("ordinary file")
    prefix = [] if binary.stem == "ve-adrive-cli" else ["ve-adrive"]
    result = subprocess.run([str(binary), *prefix, "--output", "json", "share", "files",
        f"{origin}/share/options", "--path", "docs/", "--manifest-path", str(blocked / "manifest.csv")],
        capture_output=True, text=True, timeout=15)
    assert result.returncode != 0 and not (blocked / "manifest.csv").exists()


def sorting_uses_metadata_before_formatting(binary, root, origin):
    expected_paths = {
        "key": ["docs/sub/", "docs/a.txt", "docs/negative.txt", "docs/z.csv"],
        "size": ["docs/sub/", "docs/negative.txt", "docs/a.txt", "docs/z.csv"],
        "mtime": ["docs/sub/", "docs/negative.txt", "docs/z.csv", "docs/a.txt"],
    }
    for field, paths in expected_paths.items():
        ordinary = fixture.invoke(binary, root, [
            "ls", "adrive://inst/options/docs/", "--sort", field], "csv")
        rows = list(csv.DictReader(io.StringIO(ordinary)))
        assert [row["file_path"] for row in rows] == paths, (field, rows)
        shared = fixture.invoke(binary, root, [
            *fixture.share_arguments(origin, "options"), "--sort", field, "-H"], "csv")
        assert [row["file_path"] for row in csv.DictReader(io.StringIO(shared))] == paths


def invalid_sort_never_lists_or_writes(binary, root, origin):
    prefix = [] if binary.stem == "ve-adrive-cli" else ["ve-adrive"]
    for mode in ([], ["--dry-run"]):
        messages = []
        for arguments in (fixture.share_arguments(origin, "options"),
                          ["ls", "adrive://inst/options/docs/"]):
            manifest = root / "invalid-sort" / "manifest.csv"
            before = len(fixture.Listing.requests)
            result = subprocess.run([str(binary), *prefix, "--config-path", str(root / "config"),
                "--credentials-path", str(root / "credentials"), "--output", "json", *arguments,
                "--sort", "unknown", "--manifest-path", str(manifest), *mode],
                capture_output=True, text=True, timeout=15)
            assert result.returncode == 6, result.stdout or result.stderr
            messages.append(json.loads(result.stdout or result.stderr)["error"]["message"])
            assert len(fixture.Listing.requests) == before and not manifest.parent.exists()
        assert messages[0] == messages[1] and "unsupported ls sort field" in messages[0]


def explicit_sort_reorders_structured_metadata(binary, root, origin):
    file_order = {"key": ["a.txt", "negative.txt", "z.csv"],
                  "size": ["negative.txt", "a.txt", "z.csv"],
                  "mtime": ["negative.txt", "z.csv", "a.txt"]}
    for arguments, files_key, folders_key in [
        (fixture.share_arguments(origin, "structured"), "Files", "Folders"),
        (["ls", "adrive://inst/structured/docs/"], "files", "folders"),
    ]:
        baseline = json.loads(fixture.invoke(binary, root, [*arguments, "-H"], "json"))["data"]
        assert [row["FilePath"] for row in baseline[files_key]] == ["docs/z.csv", "docs/a.txt", "docs/negative.txt"]
        assert [row["Folder"] for row in baseline[folders_key]] == ["docs/sub/", "docs/alpha/"]
        for field, names in file_order.items():
            selected = [*arguments, "--sort", field, "-H", "--columns", "size"]
            expected = [f"docs/{name}" for name in names]
            folder_order = ["docs/sub/", "docs/alpha/"] if field == "mtime" else ["docs/alpha/", "docs/sub/"]
            raw = json.loads(fixture.invoke(binary, root, selected, "json"))["data"]
            assert [row["FilePath"] for row in raw[files_key]] == expected, (field, raw[files_key])
            assert [row["Folder"] for row in raw[folders_key]] == folder_order
            assert {row["FilePath"]: row for row in raw[files_key]} == {row["FilePath"]: row for row in baseline[files_key]}
            assert {row["Folder"]: row for row in raw[folders_key]} == {row["Folder"]: row for row in baseline[folders_key]}
            assert {row["FilePath"]: row["Size"] for row in raw[files_key]} == {"docs/z.csv": 2048, "docs/a.txt": 1024, "docs/negative.txt": -1}
            projected = json.loads(fixture.invoke(binary, root, selected, "json", f"data.{files_key}[].FilePath"))
            assert projected == expected
            yaml = fixture.invoke(binary, root, selected, "yaml")
            assert [yaml.index(f"FilePath: {path}") for path in expected] == sorted(yaml.index(f"FilePath: {path}") for path in expected)
            assert [yaml.index(f"Folder: {path}") for path in folder_order] == sorted(yaml.index(f"Folder: {path}") for path in folder_order)
            assert "Size: -1" in yaml and "Size: 2048" in yaml and "KiB" not in yaml


def explicit_sort_preserves_bounded_page_markers(binary, root, origin):
    for arguments, files_key, marker_key, truncated_key in [
        (fixture.share_arguments(origin, "paged"), "Files", "NextMarker", "IsTruncated"),
        (["ls", "adrive://inst/paged/docs/"], "files", "next_marker", "is_truncated"),
    ]:
        raw = json.loads(fixture.invoke(binary, root, [*arguments, "--sort", "mtime",
            "--max-keys", "1", "--marker", "previous"], "json"))
        assert [row["FilePath"] for row in raw["data"][files_key]] == ["docs/page.txt"]
        assert raw["data"][marker_key] == "next" and raw["data"][truncated_key] is True
        assert raw["pagination"]["next_marker"] == "next" and raw["pagination"]["total_returned"] == 1


def check_binary(binary, root, origin):
    (root / "config").write_text(f'[default.adrive]\nauth_mode="aksk"\nendpoint="{origin}"\nregion="cn-beijing"\n')
    (root / "credentials").write_text('schema_version=1\n[default.adrive]\naccess_key_id="fixture-ak"\nsecret_access_key="fixture-sk"\n')
    options_parity(binary, root, origin)
    sorting_uses_metadata_before_formatting(binary, root, origin)
    explicit_sort_reorders_structured_metadata(binary, root, origin)
    explicit_sort_preserves_bounded_page_markers(binary, root, origin)
    raw_metadata_survives_display_flags(binary, root, origin)
    manifest_is_explicit_and_private(binary, root, origin)
    manifests_preserve_empty_folders_and_exact_selection(binary, root, origin)
    readonly_paths_never_write(binary, root, origin)
    invalid_sort_never_lists_or_writes(binary, root, origin)


if __name__ == "__main__":
    fixture.check_binary = check_binary
    fixture.main()
    print("PASS: share files sizes, columns, sort aliases, raw metadata, explicit private manifests and read-only paths")
