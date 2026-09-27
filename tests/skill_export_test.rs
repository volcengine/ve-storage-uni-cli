use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static EXPORT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct ExportDirectory(PathBuf);

impl ExportDirectory {
    fn new() -> Self {
        let sequence = EXPORT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        Self(std::env::temp_dir().join(format!("skill-export-{}-{sequence}", std::process::id())))
    }
}

impl Drop for ExportDirectory {
    fn drop(&mut self) {
        if self.0.exists() {
            fs::remove_dir_all(&self.0).expect("remove test export");
        }
    }
}

fn export(surface: &str, directory: &Path, language: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([surface, "skill", "export", "--name"])
        .arg(format!("{} cp", surface))
        .args(["--language", language, "--dir"])
        .arg(directory)
        .args(["--output", "json"])
        .output()
        .expect("run export")
}

fn read_export(output: &Output) -> (String, String) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).expect("JSON");
    let files = envelope["data"]["files"]
        .as_array()
        .expect("exported files");
    assert_eq!(files.len(), 2);
    (
        fs::read_to_string(files[0].as_str().unwrap()).unwrap(),
        fs::read_to_string(files[1].as_str().unwrap()).unwrap(),
    )
}

#[test]
fn unified_exports_are_installable_and_keep_the_invocation_prefix() {
    for surface in ["tos", "ve-tos", "ve-adrive"] {
        for language in ["en", "zh"] {
            let directory = ExportDirectory::new();
            let (index, skill) = read_export(&export(surface, &directory.0, language));
            for document in [&index, &skill] {
                assert!(document.starts_with("---\nname: "), "{document}");
                assert!(document.contains("\ndescription: "));
                assert!(document.contains(&format!("`ve-storage-uni-cli {surface} cp`")));
            }
            assert!(
                skill.contains("--include-parent"),
                "missing parser reference"
            );
            assert!(skill.contains("--overwrite-strategy"));
            if surface == "tos" {
                assert!(!skill.contains("| `--storage-class"));
            }
            assert!(skill.contains(&format!("Usage: ve-storage-uni-cli {surface} cp ")));
            assert!(!skill.contains("Usage: cli "));
            assert!(skill.contains(if language == "zh" {
                "传输"
            } else {
                "Transfer"
            }));
        }
    }
}

#[test]
fn nested_commands_and_pipeline_examples_keep_public_prefixes() {
    for (surface, name, expected) in [
        (
            "tos",
            "tos_put",
            "echo hello | ve-storage-uni-cli tos put tos://bucket/hello.txt",
        ),
        (
            "ve-tos",
            "ve_tos_bucket_create",
            "Usage: ve-storage-uni-cli ve-tos bucket create ",
        ),
    ] {
        let directory = ExportDirectory::new();
        let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
            .args([surface, "skill", "export", "--name", name, "--dir"])
            .arg(&directory.0)
            .args(["--output", "json"])
            .output()
            .unwrap();
        let (_, skill) = read_export(&output);
        assert!(skill.contains(expected), "{skill}");
        assert!(!skill.contains("Usage: cli "));
    }
}

#[test]
fn export_planning_and_conflicts_preserve_existing_files() {
    let directory = ExportDirectory::new();
    let (index, _) = read_export(&export("tos", &directory.0, "en"));
    let conflict = export("tos", &directory.0, "en");
    assert!(!conflict.status.success());
    assert_eq!(
        fs::read_to_string(directory.0.join("SKILL.md")).unwrap(),
        index
    );
    let planned = ExportDirectory::new();
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["tos", "skill", "export", "--dry-run", "--dir"])
        .arg(&planned.0)
        .output()
        .expect("plan export");
    assert!(output.status.success());
    assert!(!planned.0.exists());
}

#[cfg(unix)]
#[test]
fn dedicated_exports_use_only_the_dedicated_executable() {
    use std::os::unix::fs::symlink;
    for binary in ["tos-cli", "ve-tos-cli", "ve-adrive-cli"] {
        let directory = ExportDirectory::new();
        fs::create_dir_all(&directory.0).unwrap();
        let executable = directory.0.join(binary);
        symlink(env!("CARGO_BIN_EXE_ve-storage-uni-cli"), &executable).unwrap();
        let surface = binary.trim_end_matches("-cli");
        let output = Command::new(executable)
            .args([
                "skill",
                "export",
                "--name",
                &format!("{surface} cp"),
                "--dir",
            ])
            .arg(directory.0.join("export"))
            .args(["--output", "json"])
            .output()
            .unwrap();
        let (index, skill) = read_export(&output);
        assert!(index.contains(&format!("`{binary} cp`")));
        assert!(skill.contains(&format!("{binary} cp ")));
        assert!(!skill.contains("ve-storage-uni-cli"), "{skill}");
    }
}

#[test]
fn unknown_skill_does_not_create_an_export_directory() {
    let directory = ExportDirectory::new();
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "tos",
            "skill",
            "export",
            "--name",
            "unknown-command",
            "--dir",
        ])
        .arg(&directory.0)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!directory.0.exists());
}

#[test]
fn exported_tos_skills_include_zti_without_leaking_to_ve_surfaces() {
    for language in ["en", "zh"] {
        let directory = ExportDirectory::new();
        let (_, tos_skill) = read_export(&export("tos", &directory.0, language));
        assert!(tos_skill.contains("--auth-mode"));
        assert!(tos_skill.contains("aksk"));
        assert!(tos_skill.contains("zti"));
        assert!(tos_skill.contains("SEC_TOKEN_STRING"));
        assert!(tos_skill.contains("SEC_TOKEN_PATH"));

        let ve_directory = ExportDirectory::new();
        let (_, ve_skill) = read_export(&export("ve-tos", &ve_directory.0, language));
        assert!(!ve_skill.contains("SEC_TOKEN_STRING"));
        assert!(!ve_skill.contains("SEC_TOKEN_PATH"));
    }
}

#[test]
fn exported_tos_doctor_skill_matches_supported_checks() {
    let directory = ExportDirectory::new();
    let result = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["tos", "skill", "export", "--name", "tos_doctor", "--dir"])
        .arg(&directory.0)
        .args(["--output", "json"])
        .output()
        .expect("export doctor skill");
    let (_, skill) = read_export(&result);
    assert!(skill.contains("Run one check: auth, config, registry, network"));
    assert!(!skill.contains("ve-tos doctor"));
    assert!(!skill.contains("permissions"));
    assert!(!skill.contains("--bucket <BUCKET>"));
    assert!(skill.contains("--live-network"));
}
