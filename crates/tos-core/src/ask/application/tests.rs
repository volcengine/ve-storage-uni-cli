use super::*;
use crate::ask::{Config, Status, Store};

fn bundled() -> Option<ApplicationValues<'static>> {
    parse_bundle(Some("bundled_fixture"), Some("bundled-fixture-secret")).unwrap()
}
fn environment() -> ApplicationEnvironment {
    ApplicationEnvironment {
        app_id: None,
        scopes: None,
        secret_file: None,
        callback: None,
        home: Some(
            std::env::temp_dir()
                .canonicalize()
                .unwrap()
                .join(format!("ask-bundle-{}", uuid::Uuid::now_v7()))
                .into_os_string(),
        ),
    }
}
fn internal_override() -> ApplicationEnvironment {
    let mut environment = environment();
    environment.app_id = Some("override_fixture".into());
    environment.scopes = Some("override:scope".into());
    environment
}

#[test]
fn bundled_application_loads_owned_secret_without_a_file() {
    let config = configure(environment(), bundled()).unwrap();
    assert_eq!(config.app_id(), "bundled_fixture");
    assert!(config
        .required_scopes
        .iter()
        .any(|scope| scope == "doubao:session:write"));
    assert!(config
        .required_scopes
        .iter()
        .any(|scope| scope == "offline_access"));
    assert!(config.load_secret().unwrap().expose() == "bundled-fixture-secret");
    assert!(config.secret_file.is_none());
}

#[test]
fn explicit_configs_never_inherit_a_compiled_secret() {
    let config = Config::new(
        "explicit_fixture".into(),
        "http://localhost:8080/callback",
        vec!["explicit:scope".into()],
        std::env::temp_dir(),
    )
    .unwrap();
    assert!(config.load_secret().is_err());
}

#[test]
fn callback_and_home_only_overrides_keep_the_bundled_application() {
    let mut environment = environment();
    let home = environment.home.clone().unwrap();
    environment.callback = Some("http://127.0.0.1:18080/callback".into());
    let config = configure(environment, bundled()).unwrap();
    assert_eq!(config.home, PathBuf::from(home));
    assert_eq!(config.redirect_uri(), "http://127.0.0.1:18080/callback");
    assert!(config.load_secret().unwrap().expose() == "bundled-fixture-secret");
}

#[test]
fn every_partial_override_disables_bundle_fallback() {
    for mask in [1, 2, 4, 5, 6] {
        let mut environment = environment();
        environment.app_id = (mask & 1 != 0).then(|| "override_fixture".into());
        environment.scopes = (mask & 2 != 0).then(|| "override:scope".into());
        environment.secret_file = (mask & 4 != 0).then(|| "/nonexistent/secret".into());
        assert!(configure(environment, bundled()).is_err());
    }
}

#[test]
fn internal_override_never_mixes_bundle_fields_or_secret() {
    let config = configure(internal_override(), bundled()).unwrap();
    assert_eq!(config.app_id(), "override_fixture");
    assert!(config
        .required_scopes
        .iter()
        .any(|scope| scope == "override:scope"));
    assert!(!config
        .required_scopes
        .iter()
        .any(|scope| scope == "doubao:session:write"));
    assert!(config.load_secret().is_err());
}

#[test]
fn unconfigured_build_requires_a_complete_internal_override() {
    assert!(configure(environment(), None).is_err());
    assert!(configure(internal_override(), None).is_ok());
}

#[test]
fn empty_runtime_application_values_are_rejected_without_fallback() {
    for field in ["app_id", "scopes", "secret_file", "callback"] {
        let mut environment = internal_override();
        match field {
            "app_id" => environment.app_id = Some("".into()),
            "scopes" => environment.scopes = Some("".into()),
            "secret_file" => environment.secret_file = Some("".into()),
            _ => environment.callback = Some("".into()),
        }
        assert!(configure(environment, bundled()).is_err());
    }
}

#[cfg(unix)]
#[test]
fn nonunicode_runtime_values_are_redacted_and_never_fall_back() {
    use std::os::unix::ffi::OsStringExt;
    for field in ["app_id", "scopes", "secret_file", "callback"] {
        let mut environment = internal_override();
        let invalid = Some(std::ffi::OsString::from_vec(vec![0xff]));
        match field {
            "app_id" => environment.app_id = invalid,
            "scopes" => environment.scopes = invalid,
            "secret_file" => environment.secret_file = invalid,
            _ => environment.callback = invalid,
        }
        let error = configure(environment, bundled()).err().unwrap();
        assert_eq!(error.to_string(), "invalid Doubao configuration");
    }
}

#[cfg(unix)]
#[test]
fn status_and_logout_do_not_need_a_secret_file_in_either_mode() {
    for environment in [environment(), internal_override()] {
        let config = configure(environment, bundled()).unwrap();
        let store = Store::open(&config).unwrap();
        assert_eq!(store.status().unwrap(), Status::LoggedOut);
        store.logout().unwrap();
        assert!(config.secret_file.is_none());
        std::fs::remove_dir_all(&config.home).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn unsafe_or_missing_override_files_never_fall_back_to_bundled_secret() {
    use std::os::unix::fs::PermissionsExt;
    let mut environment = internal_override();
    let home = PathBuf::from(environment.home.clone().unwrap());
    let secret_path = home.join("override-secret");
    environment.secret_file = Some(secret_path.clone().into_os_string());
    let config = configure(environment, bundled()).unwrap();
    let _store = Store::open(&config).unwrap();
    assert!(config.load_secret().is_err());
    std::fs::write(&secret_path, "override-fixture-secret").unwrap();
    std::fs::set_permissions(&secret_path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(config.load_secret().is_err());
    std::fs::set_permissions(&secret_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(config.load_secret().unwrap().expose() == "override-fixture-secret");
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn publisher_application_requests_exactly_the_ask_permissions() {
    let config = configure(environment(), bundled()).unwrap();
    assert_eq!(
        config.required_scopes,
        vec![
            "doubao:session.event:read",
            "doubao:session:write",
            "offline_access"
        ]
    );
}
