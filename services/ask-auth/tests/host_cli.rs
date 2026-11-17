use std::process::Command;

#[test]
fn help_runs_without_workload_identity_or_application_credentials() {
    let output = Command::new(env!("CARGO_BIN_EXE_ask-auth"))
        .arg("--help")
        .env_clear()
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("ask-auth probe --environment boe|prod"));
    assert!(output.stderr.is_empty());
}
#[test]
fn accidental_secret_arguments_and_non_utf8_values_never_reach_output() {
    let output = Command::new(env!("CARGO_BIN_EXE_ask-auth"))
        .args([
            "serve",
            "--environment",
            "boe",
            "--client-secret",
            "PRIVATE_SENTINEL",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "invalid bootstrap arguments\n"
    );
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let output = Command::new(env!("CARGO_BIN_EXE_ask-auth"))
            .arg(std::ffi::OsString::from_vec(vec![255]))
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            "invalid bootstrap arguments\n"
        );
    }
}
#[test]
#[cfg(unix)]
fn deployment_builder_rejects_non_linux_before_cargo() {
    if std::env::consts::OS != "linux" {
        let output = Command::new("bash")
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/build.sh"))
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            "ask-auth deployment build requires Linux\n"
        );
    }
}

#[test]
fn same_binary_exposes_every_deployment_command() {
    let output = Command::new(env!("CARGO_BIN_EXE_ask-auth"))
        .arg("--help")
        .env_clear()
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    for command in ["ask-auth serve", "ask-auth probe", "ask-auth migrate"] {
        assert!(help.contains(command), "missing command: {command}");
    }
}

#[test]
fn one_auth_package_uses_the_root_workspace_and_public_dependencies() {
    let service = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    assert!(!service.join("../ask-auth-tce").exists());
    assert!(!service.join("Cargo.lock").exists());
    assert!(!service.join(".cargo").exists());
    let manifest = std::fs::read_to_string(service.join("Cargo.toml")).unwrap();
    assert!(!manifest.contains("[workspace]"));
    assert!(!manifest.contains("byted-"));
    let lock = std::fs::read_to_string(service.join("../../Cargo.lock")).unwrap();
    assert_eq!(lock.matches("name = \"ask-auth\"").count(), 1);
    assert!(!lock.contains("name = \"ask-auth-tce\""));
    assert!(!lock.contains("name = \"byted-"));
}

#[test]
fn cargo_metadata_has_one_auth_package_and_binary() {
    let output = Command::new(env!("CARGO"))
        .args([
            "metadata",
            "--offline",
            "--locked",
            "--no-deps",
            "--format-version",
            "1",
        ])
        .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
        .output()
        .unwrap();
    assert!(output.status.success());
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let packages = metadata["packages"].as_array().unwrap();
    let auth: Vec<_> = packages
        .iter()
        .filter(|package| package["name"].as_str().unwrap().starts_with("ask-auth"))
        .collect();
    assert_eq!(auth.len(), 1);
    let binaries: Vec<_> = auth[0]["targets"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|target| {
            target["kind"]
                .as_array()
                .unwrap()
                .iter()
                .any(|kind| kind == "bin")
        })
        .collect();
    assert_eq!(binaries.len(), 1);
    assert_eq!(binaries[0]["name"], "ask-auth");
    assert_eq!(metadata["workspace_members"].as_array().unwrap().len(), 8);
}

#[test]
fn same_binary_routes_serve_probe_and_migrate_before_identity_initialization() {
    for arguments in [
        vec!["serve"],
        vec!["probe"],
        vec!["migrate", "--config", "relative.json"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_ask-auth"))
            .args(arguments)
            .env_clear()
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr, b"invalid bootstrap arguments\n");
    }
}
