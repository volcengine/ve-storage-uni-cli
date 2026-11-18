use super::*;
use crate::login::{AuthorizationRequest, OAuthProvider};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

const CALLBACK: &str = "https://broker.example/v1/oauth/doubao/callback";

fn credentials() -> AppCredentials {
    AppCredentials::new(
        "synthetic-app".into(),
        SecretString::new("synthetic-secret+&=".into()),
    )
    .unwrap()
}
fn adapter() -> DoubaoOAuth {
    DoubaoOAuth::new(
        credentials(),
        CALLBACK,
        vec!["offline_access".into(), "read:user".into()],
    )
    .unwrap()
}
fn state() -> String {
    let mut bytes = uuid::Uuid::now_v7().as_bytes().to_vec();
    bytes.extend_from_slice(&[9; 32]);
    URL_SAFE_NO_PAD.encode(bytes)
}

#[test]
fn accepts_trusted_configuration() {
    let oauth = adapter();
    let endpoints = oauth.endpoints();
    let state = state();
    let url = oauth
        .authorization_url(AuthorizationRequest {
            state: &state,
            callback_url: CALLBACK,
            required_scope: &["read:user".into(), "offline_access".into()],
        })
        .unwrap();
    let url = url::Url::parse(&url).unwrap();
    let fields: std::collections::BTreeMap<_, _> = url.query_pairs().collect();
    assert_eq!(
        url.origin().ascii_serialization(),
        "https://accounts.doubao.com"
    );
    assert_eq!(fields.get("client_id").unwrap(), "synthetic-app");
    assert_eq!(fields.get("state").unwrap(), &state);
    assert_eq!(fields.get("scope").unwrap(), "offline_access read:user");
    assert_eq!(fields.get("redirect_uri").unwrap(), CALLBACK);
    assert_eq!(endpoints.callback_url, CALLBACK);
    assert!(!endpoints.authorization_url.contains("synthetic-secret"));
}

#[test]
fn rejects_unsafe_configuration() {
    for callback in [
        "http://broker.example/v1/oauth/doubao/callback",
        "https://u:p@broker.example/v1/oauth/doubao/callback",
        "https://broker.example/other",
        "https://broker.example/v1/oauth/doubao/callback?x=1",
        "https://broker.example/v1/oauth/doubao/callback#fragment",
    ] {
        assert!(DoubaoOAuth::new(credentials(), callback, vec!["offline_access".into()]).is_err());
    }
    for scopes in [
        vec![],
        vec!["read:user".into()],
        vec!["offline_access".into(); 2],
        vec!["offline_access".into(), "bad scope".into()],
        vec!["offline_access".into(); 101],
    ] {
        assert!(DoubaoOAuth::new(credentials(), CALLBACK, scopes).is_err());
    }
    for secret in ["", "has space", "line\nbreak", "非ASCII"] {
        assert!(AppCredentials::new("app".into(), SecretString::new(secret.into())).is_err());
    }
    assert!(AppCredentials::new("x".repeat(257), SecretString::new("secret".into())).is_err());
    assert!(AppCredentials::new("app".into(), SecretString::new("x".repeat(4097))).is_err());
}

#[test]
fn authorization_rejects_state_and_configuration_drift() {
    let oauth = adapter();
    for state in [String::new(), "A".repeat(64), format!("{}=", state())] {
        assert!(oauth
            .authorization_url(AuthorizationRequest {
                state: &state,
                callback_url: CALLBACK,
                required_scope: &["offline_access".into(), "read:user".into()],
            })
            .is_err());
    }
    assert!(oauth
        .authorization_url(AuthorizationRequest {
            state: &state(),
            callback_url: "https://other.example/v1/oauth/doubao/callback",
            required_scope: &["offline_access".into(), "read:user".into()],
        })
        .is_err());
    assert!(oauth
        .authorization_url(AuthorizationRequest {
            state: &state(),
            callback_url: CALLBACK,
            required_scope: &["offline_access".into()],
        })
        .is_err());
}

mod parsing;
mod transport;

#[test]
fn rejects_configuration_that_cannot_fit_authorization_bound() {
    let mut scopes = vec!["offline_access".to_owned()];
    scopes.extend((0..99).map(|number| format!("{number:03}{}", "x".repeat(253))));
    assert!(DoubaoOAuth::new(credentials(), CALLBACK, scopes).is_err());
}
