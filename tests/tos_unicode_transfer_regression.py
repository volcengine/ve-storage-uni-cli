"""Validate ByteTOS UTF-8 transfers against the Go SDK V1 signing contract."""
import base64
import hashlib
import hmac
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
from urllib.parse import parse_qsl, unquote, urlsplit

from directory_upload_regression import Fixture, UploadService, fixture_environment


def sdk_signature(method, path, expiration, query=(), secret="fixture-sk"):
    # Go SDK canonicalRequest signs URL.Path, not URL.EscapedPath.
    parts = [method, path]
    if query:
        parts.append("&".join(f"{key}={value}" for key, value in sorted(query)))
    parts.extend(["", str(expiration)])
    digest = hmac.new(secret.encode(), "\n".join(parts).encode(), hashlib.sha256).digest()
    return base64.urlsafe_b64encode(digest).decode().rstrip("=")[:30]


class SignedUploadService(UploadService):
    def is_authorized(self):
        parsed = urlsplit(self.path)
        signature = self.headers.get("X-Tos-Signature", "")
        match = re.fullmatch(
            r"TOS-HMAC-SHA256 expiration=(\d+),signame=,signature=([^,]+),credentials=.+", signature)
        if match:
            expected = sdk_signature(self.command, unquote(parsed.path), match[1],
                                     parse_qsl(parsed.query, keep_blank_values=True))
            if hmac.compare_digest(match[2], expected):
                self.server.authenticated.append((self.command, unquote(parsed.path)))
                return True
        self.server.auth_failures.append((self.command, parsed.path))
        self.respond({"Code": "SignatureDoesNotMatch", "Message": "raw V1 canonical path required"}, 403)
        return False

    def do_HEAD(self):
        if self.is_authorized():
            super().do_HEAD()

    def do_GET(self):
        if self.is_authorized():
            super().do_GET()

    def do_PUT(self):
        if self.is_authorized():
            super().do_PUT()


def invoke(fixture, arguments, output="json"):
    prefix = ["tos"] if fixture.binary.name == "ve-storage-uni-cli" else []
    command = [str(fixture.binary), *prefix, "--config-path", str(fixture.root / "config"),
               "--credentials-path", str(fixture.root / "credentials"), "--output", output,
               "--quiet", *arguments]
    environment = fixture_environment()
    if fixture.is_psm:
        environment["TEST_TOSAPI_ADDR"] = f"127.0.0.1:{fixture.server.server_port}"
    result = subprocess.run(command, capture_output=True, text=True, env=environment, timeout=30)
    return result


def upload_arguments(fixture, flags):
    return ["cp", str(fixture.source), fixture.destination(), "--recursive",
            "--checkpoint-dir", str(fixture.root / "checkpoints"), *flags]


def check_upload_names(fixture):
    folder = fixture.source / "中文 空+%25%2F?#"
    folder.mkdir()
    (folder / "未命名空目录").mkdir()
    for name in ["原名.txt", "a b+%25%2F?#.txt"]:
        (folder / name).write_bytes(name.encode())
    for flags in [(), ("--no-manifest",), ("--disable-dir-object",)]:
        fixture.reset()
        fixture.server.auth_failures.clear()
        result = invoke(fixture, upload_arguments(fixture, flags))
        assert result.returncode == 0, (result.stdout, result.stderr)
        assert not fixture.server.auth_failures, fixture.server.auth_failures
        expected_files = {"base/" + path.relative_to(fixture.source).as_posix(): path.read_bytes()
                          for path in fixture.source.rglob("*") if path.is_file()}
        actual_files = {key: body for key, body in fixture.server.objects.items() if not key.endswith("/")}
        assert actual_files == expected_files, (actual_files, expected_files)
        directories = [key for key in fixture.server.objects if key.endswith("/")]
        if "--disable-dir-object" in flags:
            assert not directories, directories
        else:
            assert "base/中文 空+%25%2F?#/未命名空目录/" in directories, directories
            assert all(fixture.server.objects[key] == b"" for key in directories)
            for key in directories:
                assert key.rstrip("/") not in actual_files, (key, actual_files)


def check_summary(fixture):
    for output in ["table", "csv"]:
        for is_failed in [False, True]:
            fixture.reset()
            fixture.server.fail_directory = is_failed
            result = invoke(fixture, upload_arguments(fixture, ()), output)
            assert result.returncode == int(is_failed), (result.stdout, result.stderr)
            for field in ["manifest_path", "report_path", "succeeded", "failed"]:
                assert field in result.stdout, (output, is_failed, field, result.stdout)
            assert "cp-manifest-" in result.stdout, result.stdout
    fixture.server.fail_directory = False


def check_binary(binary, is_psm):
    with tempfile.TemporaryDirectory(prefix="tos-unicode-signature-") as directory:
        fixture = Fixture(binary, "tos", Path(directory))
        fixture.is_psm = is_psm
        if is_psm:
            config = fixture.root / "config"
            config.write_text(re.sub(r"(?m)^endpoint=.*$", "psm='fixture.tos.psm'", config.read_text()))
        source = fixture.root / "猿辅导素养课截图"
        fixture.source.rename(source)
        fixture.source = source
        fixture.server.RequestHandlerClass = SignedUploadService
        fixture.server.authenticated, fixture.server.auth_failures = [], []
        try:
            check_upload_names(fixture)
            check_summary(fixture)
            assert fixture.server.authenticated
            assert (fixture.root / "credentials").read_text() == "schema_version=1\n"
        finally:
            fixture.close()
        mode = "PSM" if is_psm else "endpoint"
        print(f"PASS: {binary.name} {mode}; SDK V1 signatures, exact UTF-8 keys, empty dirs, table/CSV paths")


if __name__ == "__main__":
    assert sdk_signature("PUT", "/bucket/【田野巴士】The Fast Yellow Bus 黄色快速公交车/book.pdf",
                         1700003600, secret="sk") == "YJj75zGLcScmNbJIkFADkh89q36l2X"
    assert len(sys.argv) > 1, "pass the unified and/or tos-cli binary paths"
    for argument in sys.argv[1:]:
        for is_psm in [False, True]:
            check_binary(Path(argument).resolve(), is_psm)
