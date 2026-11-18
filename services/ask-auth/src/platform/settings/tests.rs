use super::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

pub(crate) fn runtime() -> String {
    r#"{"schema_version":1,"is_enabled":true,"app_id":"synthetic_app","environment":"boe","deployment":"synthetic-deployment","callback_url":"https://auth.example.com/v1/oauth/doubao/callback","required_scopes":["offline_access"],"current_key_version":1,"database_ca_path":"/synthetic/ca.pem","api_pool_connections":1,"cleanup_pool_connections":1}"#.into()
}
pub(crate) fn secrets() -> String {
    format!(
        r#"{{"schema_version":1,"client_secret":"synthetic-only","database_url":"mysql://synthetic:synthetic@127.0.0.1/isolated","keys":[{{"version":1,"hmac_key":"{}","aead_key":"{}"}}]}}"#,
        URL_SAFE_NO_PAD.encode([1; 32]),
        URL_SAFE_NO_PAD.encode([2; 32])
    )
}
#[test]
fn disabled_minimal_requires_no_app_secrets_or_database() {
    assert!(validate(
        r#"{"schema_version":1,"is_enabled":false}"#,
        None,
        ExpectedEnvironment::Boe
    )
    .is_ok());
}
#[test]
fn active_parsing_checks_environment_scope_and_current_key() {
    assert!(validate(&runtime(), Some(&secrets()), ExpectedEnvironment::Boe).is_ok());
    assert!(validate(
        &runtime(),
        Some(&secrets()),
        ExpectedEnvironment::Production
    )
    .is_err());
    assert!(validate(
        &runtime().replace("offline_access", "profile"),
        Some(&secrets()),
        ExpectedEnvironment::Boe
    )
    .is_err());
    assert!(validate(
        &runtime().replace("current_key_version\":1", "current_key_version\":2"),
        Some(&secrets()),
        ExpectedEnvironment::Boe
    )
    .is_err());
}
#[test]
fn configuration_rejects_unknown_duplicate_array_and_deep_values() {
    for text in [
        r#"{"schema_version":1,"is_enabled":false,"extra":0}"#,
        r#"{"schema_version":1,"schema_version":1,"is_enabled":false}"#,
        "[1,false]",
        r#"{"schema_version":2,"is_enabled":false}"#,
    ] {
        assert!(validate(text, None, ExpectedEnvironment::Boe).is_err());
    }
    let deep = format!(
        r#"{{"schema_version":1,"is_enabled":false,"app_id":{}0{}}}"#,
        "[".repeat(65),
        "]".repeat(65)
    );
    assert!(validate(&deep, None, ExpectedEnvironment::Boe).is_err());
}
#[test]
fn secret_keys_require_canonical_independent_unique_material() {
    let secret = secrets();
    let same = secret.replace(
        &URL_SAFE_NO_PAD.encode([2; 32]),
        &URL_SAFE_NO_PAD.encode([1; 32]),
    );
    assert!(validate(&runtime(), Some(&same), ExpectedEnvironment::Boe).is_err());
    assert!(validate(
        &runtime(),
        Some(&secret.replace("\"keys\":[{", "\"keys\":[[")),
        ExpectedEnvironment::Boe
    )
    .is_err());
    assert!(validate(
        &runtime(),
        Some(&secret.replace(&URL_SAFE_NO_PAD.encode([1; 32]), "AAAA=")),
        ExpectedEnvironment::Boe
    )
    .is_err());
}
