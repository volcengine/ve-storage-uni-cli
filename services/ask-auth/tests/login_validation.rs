use ask_auth::login::{AdmissionSnapshot, DeploymentIdentity, EnabledConfig, OAuthEndpoints};
use chrono::{Duration, Utc};

#[test]
fn immutable_enabled_config_rejects_non_https_or_query_callback() {
    assert!(configuration("http://auth.example/callback").is_err());
    assert!(configuration("https://auth.example/callback?secret=x").is_err());
}

#[test]
fn admission_defaults_to_deny_and_explicit_denial_has_precedence() {
    let now = chrono::DateTime::from_timestamp_millis(Utc::now().timestamp_millis())
        .unwrap()
        .naive_utc();
    let mut policy = AdmissionSnapshot::deny_all(now);
    assert!(!policy.allows("user", Some("tenant"), now));
    policy.is_enabled = true;
    policy.version = "policy-1".into();
    policy.allowed_users.insert("user".into());
    assert!(policy.allows("user", Some("tenant"), now));
    policy.denied_tenants.insert("tenant".into());
    assert!(!policy.allows("user", Some("tenant"), now));
    assert!(!policy.allows("user", None, now + Duration::minutes(6)));
}

fn configuration(callback: &str) -> Result<EnabledConfig, ask_auth::login::LoginError> {
    EnabledConfig::new(
        DeploymentIdentity {
            app_id: "app".into(),
            environment: "test".into(),
            deployment: "deploy".into(),
        },
        1,
        vec!["files.read".into()],
        OAuthEndpoints {
            callback_url: callback.into(),
            authorization_url: "https://provider.example/authorize".into(),
        },
    )
}

#[test]
fn enabled_config_accepts_fixed_callback_and_rejects_other_paths_and_pkce() {
    assert!(configuration("https://auth.example/v1/oauth/doubao/callback").is_ok());
    assert!(configuration("https://auth.example/other").is_err());
    let configured = EnabledConfig::new(
        DeploymentIdentity {
            app_id: "app".into(),
            environment: "test".into(),
            deployment: "deploy".into(),
        },
        1,
        vec!["files.read".into()],
        OAuthEndpoints {
            callback_url: "https://auth.example/v1/oauth/doubao/callback".into(),
            authorization_url: "https://provider.example/authorize?code_challenge=unproven".into(),
        },
    );
    assert!(configured.is_err());
}

#[test]
fn admission_rejects_submillisecond_snapshots_without_future_tolerance() {
    let now = chrono::DateTime::from_timestamp_millis(Utc::now().timestamp_millis())
        .unwrap()
        .naive_utc();
    let fetched_at = now - Duration::milliseconds(1) + Duration::nanoseconds(1);
    let mut policy = AdmissionSnapshot::deny_all(fetched_at);
    policy.is_enabled = true;
    policy.version = "policy-precision-1".into();
    policy.allowed_users.insert("verified-user".into());
    assert!(!policy.is_fresh(now));
    assert!(!policy.allows("verified-user", None, now));
    policy.fetched_at = now;
    assert!(policy.is_fresh(now));
    policy.fetched_at = now + Duration::milliseconds(1);
    assert!(!policy.is_fresh(now));
}
