"""Exercise the real build script and Cargo environment tracking with fake inputs only."""

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


REPOSITORY = Path(__file__).resolve().parents[1]
APP_ID = "STORAGE_CLI_BUILD_DOUBAO_APP_ID"
APP_SECRET = "STORAGE_CLI_BUILD_DOUBAO_APP_SECRET"


class DoubaoBuildRegression(unittest.TestCase):
    def setUp(self):
        self.sandbox = tempfile.TemporaryDirectory(prefix="doubao-build-regression-")
        self.addCleanup(self.sandbox.cleanup)
        self.root = Path(self.sandbox.name)
        (self.root / "src/ask").mkdir(parents=True)
        shutil.copyfile(REPOSITORY / "crates/tos-core/build.rs", self.root / "build.rs")
        shutil.copyfile(REPOSITORY / "crates/tos-core/src/ask/application_values.rs",
                        self.root / "src/ask/application_values.rs")
        (self.root / "Cargo.toml").write_text(
            '[package]\nname="doubao-build-regression"\nversion="0.0.0"\n'
            'edition="2021"\n[workspace]\n', encoding="utf-8")
        (self.root / "src/main.rs").write_text(
            '#[path="ask/application_values.rs"] mod application_values;\n'
            'fn main() { let bundle = application_values::parse_bundle(\n'
            'option_env!("STORAGE_CLI_BUILD_DOUBAO_APP_ID"),\n'
            'option_env!("STORAGE_CLI_BUILD_DOUBAO_APP_SECRET")).unwrap();\n'
            'println!("application_bundled={}", bundle.is_some()); }\n', encoding="utf-8")
        self.environment = os.environ.copy()
        self.environment.pop(APP_ID, None)
        self.environment.pop(APP_SECRET, None)
        self.environment["CARGO_TARGET_DIR"] = str(self.root / "target")

    def invoke(self, app_id=None, secret=None):
        environment = self.environment.copy()
        for name, value in ((APP_ID, app_id), (APP_SECRET, secret)):
            if value is not None:
                environment[name] = value
        return subprocess.run(["cargo", "run", "--offline"], cwd=self.root,
                              env=environment, capture_output=True, text=True, timeout=60)

    def test_absent_inputs_build_with_an_explicit_unconfigured_notice(self):
        result = self.invoke()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "application_bundled=false")
        self.assertIn("Doubao ask application: not bundled", result.stderr)

    def test_exported_inputs_reach_the_compiler_without_logging_values(self):
        result = self.invoke("synthetic_build_app", "synthetic_build_secret")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "application_bundled=true")
        self.assertIn("Doubao ask application: bundled", result.stderr)
        self.assertNotIn("synthetic_build_app", result.stderr)
        self.assertNotIn("synthetic_build_secret", result.stderr)

    def test_cached_build_tracks_adding_and_removing_inputs(self):
        for app_id, secret, expected in (
            (None, None, "false"),
            ("synthetic_build_app", "synthetic_build_secret", "true"),
            (None, None, "false"),
        ):
            result = self.invoke(app_id, secret)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout.strip(), f"application_bundled={expected}")

    def test_partial_inputs_fail_without_logging_the_value(self):
        result = self.invoke("synthetic_partial_app", None)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("invalid bundled Doubao application configuration", result.stderr)
        self.assertNotIn("synthetic_partial_app", result.stderr)

    def test_unexported_shell_assignments_do_not_reach_cargo(self):
        result = subprocess.run(
            ["bash", "--noprofile", "--norc", "-c",
             f"{APP_ID}=synthetic_shell_app; {APP_SECRET}=synthetic_shell_secret; cargo run --offline"],
            cwd=self.root, env=self.environment, capture_output=True, text=True, timeout=60)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "application_bundled=false")
        self.assertIn("Doubao ask application: not bundled", result.stderr)
        self.assertNotIn("synthetic_shell_secret", result.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=2)
