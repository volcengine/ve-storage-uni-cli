//! Fake adapter tests against an explicit isolated real MySQL writer; no live OAuth calls.
use ask_auth::{
    build_enabled_router,
    crypto::{Crypto, KeyMaterial},
    login::*,
    storage::*,
};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{Duration, Utc};
use sha2::{Digest, Sha256};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration as Wait,
};
use tower::ServiceExt;
use uuid::Uuid;

struct FakeProvider {
    calls: AtomicUsize,
    outcome: &'static str,
    refresh_outcome: std::sync::Mutex<&'static str>,
    refresh_calls: AtomicUsize,
    is_paused: AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
impl OAuthProvider for FakeProvider {
    fn refresh(&self, _request: ProviderRefreshRequest) -> RefreshFuture<'_> {
        self.refresh_calls.fetch_add(1, Ordering::SeqCst);
        let outcome = *self.refresh_outcome.lock().unwrap();
        Box::pin(async move {
            let mut tokens = self
                .exchange(ExchangeRequest {
                    code: "fake-refresh-exchange".into(),
                    callback_url: "https://auth.example/v1/oauth/doubao/callback".into(),
                })
                .await
                .map_err(|_| RefreshFailure::OutcomeUnknown)?;
            match outcome {
                "timeout" => {
                    tokio::time::sleep(Wait::from_secs(11)).await;
                }
                "unknown" => return Err(RefreshFailure::OutcomeUnknown),
                "expiry_failure" => {
                    tokio::time::sleep(Wait::from_secs(2)).await;
                    return Err(RefreshFailure::NoConsumption(
                        NoConsumptionReason::Unavailable,
                    ));
                }
                "unavailable" => {
                    return Err(RefreshFailure::NoConsumption(
                        NoConsumptionReason::Unavailable,
                    ))
                }
                "contract" => {
                    return Err(RefreshFailure::NoConsumption(NoConsumptionReason::Contract))
                }
                "invalid" => {
                    return Err(RefreshFailure::NoConsumption(
                        NoConsumptionReason::CredentialInvalid,
                    ))
                }
                "identity" => tokens.identity.user_id = "other-user".into(),
                "tenant" => tokens.identity.tenant_id = None,
                "expanded" => tokens.scope.push("files.write".into()),
                "dropped" => tokens.scope = vec!["other".into()],
                "duplicate" => tokens.scope.push("files.read".into()),
                "empty_at" => tokens.access_token.clear(),
                "empty_rt" => tokens.refresh_token.clear(),
                "expired" => tokens.expires_at = Utc::now().naive_utc() - Duration::hours(1),
                "unknown_expiry" => tokens.refresh_expires_at = None,
                "narrow" => tokens.scope = vec!["files.read".into()],
                _ => {}
            }
            Ok(tokens)
        })
    }
    fn authorization_url(
        &self,
        request: AuthorizationRequest<'_>,
    ) -> Result<String, ProviderError> {
        let mut url =
            url::Url::parse("https://provider.example/authorize?client_id=app-a").unwrap();
        url.query_pairs_mut()
            .append_pair("state", request.state)
            .append_pair("redirect_uri", request.callback_url);
        Ok(url.into())
    }
    fn exchange(&self, _request: ExchangeRequest) -> ProviderFuture<'_> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Box::pin(async move {
            if self.is_paused.swap(false, Ordering::SeqCst) {
                self.entered.notify_one();
                self.release.notified().await;
            }
            if self.outcome == "timeout" {
                tokio::time::sleep(Wait::from_secs(11)).await;
            }
            if self.outcome == "failure" {
                return Err(ProviderError::OutcomeUnknown);
            }
            Ok(ProviderTokens {
                identity: Identity {
                    user_id: "verified-user".into(),
                    tenant_id: Some("verified-tenant".into()),
                },
                scope: if self.outcome == "wide" {
                    vec!["files.read".into(), "files.write".into()]
                } else {
                    vec![if self.outcome == "scope" {
                        "other"
                    } else {
                        "files.read"
                    }
                    .into()]
                },
                access_token: "fake-access".into(),
                refresh_token: "fake-refresh".into(),
                expires_at: Utc::now().naive_utc() + Duration::hours(1),
                refresh_expires_at: Some(Utc::now().naive_utc() + Duration::days(3)),
            })
        })
    }
}
struct FakeAdmission {
    is_enabled: AtomicBool,
    calls: AtomicUsize,
    pause_at_call: AtomicUsize,
    has_failure: AtomicBool,
    is_allowed: AtomicBool,
    is_paused: AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
impl AdmissionProvider for FakeAdmission {
    fn snapshot(&self) -> AdmissionFuture<'_> {
        Box::pin(async {
            let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if self.has_failure.load(Ordering::SeqCst) {
                return Err(AdmissionError);
            }
            if self.is_paused.swap(false, Ordering::SeqCst)
                || call == self.pause_at_call.load(Ordering::SeqCst)
            {
                self.entered.notify_one();
                self.release.notified().await;
            }
            let fetched_at =
                chrono::DateTime::from_timestamp_millis(Utc::now().timestamp_millis() - 1000)
                    .unwrap()
                    .naive_utc();
            let mut policy = AdmissionSnapshot::deny_all(fetched_at);
            policy.is_enabled = self.is_enabled.load(Ordering::SeqCst);
            policy.version = "fake-policy-1".into();
            if self.is_allowed.load(Ordering::SeqCst) {
                policy.allowed_users.insert("verified-user".into());
            }
            Ok(policy)
        })
    }
}
struct Fixture {
    router: Router,
    store: Arc<Store>,
    provider: Arc<FakeProvider>,
    crypto: Arc<Crypto>,
    admission: Arc<FakeAdmission>,
}
async fn fixture(outcome: &'static str, is_allowed: bool) -> Fixture {
    let database = runtime_database().await;
    let crypto = Arc::new(
        Crypto::new(
            "deployment-a",
            "app-a",
            vec![
                KeyMaterial::new(1, [3; 32], [7; 32]).unwrap(),
                KeyMaterial::new(2, [4; 32], [8; 32]).unwrap(),
            ],
        )
        .unwrap(),
    );
    let store = Arc::new(Store::new(database, "app-a", crypto.clone()).unwrap());
    let config = enabled_config();
    let provider = Arc::new(FakeProvider {
        calls: 0.into(),
        outcome,
        refresh_outcome: std::sync::Mutex::new("ok"),
        refresh_calls: 0.into(),
        is_paused: false.into(),
        entered: Default::default(),
        release: Default::default(),
    });
    let admission = Arc::new(FakeAdmission {
        is_enabled: true.into(),
        calls: 0.into(),
        pause_at_call: 0.into(),
        has_failure: false.into(),
        is_allowed: is_allowed.into(),
        is_paused: false.into(),
        entered: Default::default(),
        release: Default::default(),
    });
    let service = LoginService::new(
        store.clone(),
        crypto.clone(),
        config,
        LoginAdapters {
            provider: provider.clone(),
            admission: admission.clone(),
        },
    )
    .unwrap();
    Fixture {
        router: build_enabled_router(Arc::new(service)),
        store,
        provider,
        crypto,
        admission,
    }
}
async fn runtime_database() -> RuntimeDatabase {
    let migration = MigrationDatabase::connect(
        DatabaseConfig::new(
            std::env::var("ASK_AUTH_TEST_MIGRATION_URL")
                .expect("isolated migration account required"),
            Tls::LoopbackTlsTestOnly,
            1,
        )
        .unwrap(),
    )
    .await
    .unwrap();
    migration.migrate().await.unwrap();
    migration.close().await;
    RuntimeDatabase::connect(
        DatabaseConfig::new(
            std::env::var("ASK_AUTH_TEST_DATABASE_URL").expect("isolated runtime account required"),
            Tls::LoopbackTlsTestOnly,
            8,
        )
        .unwrap(),
    )
    .await
    .unwrap()
}
fn enabled_config() -> EnabledConfig {
    enabled_config_version(1)
}
fn enabled_config_version(version: u16) -> EnabledConfig {
    EnabledConfig::new(
        DeploymentIdentity {
            app_id: "app-a".into(),
            environment: "test".into(),
            deployment: "deployment-a".into(),
        },
        version,
        vec!["files.read".into()],
        OAuthEndpoints {
            callback_url: "https://auth.example/v1/oauth/doubao/callback".into(),
            authorization_url: "https://provider.example/authorize?client_id=app-a".into(),
        },
    )
    .unwrap()
}
fn creation(auth_id: Uuid) -> serde_json::Value {
    serde_json::json!({"auth_id":auth_id.to_string(),"login_ticket":URL_SAFE_NO_PAD.encode([19;32]),
        "claim_challenge":URL_SAFE_NO_PAD.encode(Sha256::digest(URL_SAFE_NO_PAD.encode([23;32]).as_bytes())),
        "device_name":"test-device","cli_version":"0.1"})
}
async fn json_request(
    router: &Router,
    method: &str,
    uri: &str,
    payload: serde_json::Value,
    key: Option<Uuid>,
) -> (StatusCode, serde_json::Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!("Bearer {}", URL_SAFE_NO_PAD.encode([19; 32])),
        );
    if let Some(key) = key {
        request = request.header("idempotency-key", key.to_string());
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::from(payload.to_string())).unwrap())
        .await
        .unwrap();
    assert_eq!(response.headers()["cache-control"], "no-store");
    let status = response.status();
    let trace = response.headers()["x-request-id"]
        .to_str()
        .unwrap()
        .to_owned();
    let payload: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 32768).await.unwrap()).unwrap();
    assert_eq!(payload["request_id"], trace);
    assert_eq!(Uuid::parse_str(&trace).unwrap().get_version_num(), 7);
    (status, payload)
}
async fn create(fixture: &Fixture, auth_id: Uuid) -> serde_json::Value {
    let (status, payload) = json_request(
        &fixture.router,
        "POST",
        "/v1/auth/login-attempts",
        creation(auth_id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    payload
}
async fn callback(fixture: &Fixture, created: &serde_json::Value) -> StatusCode {
    callback_parameter(fixture, created, "code=fake-code").await
}
async fn callback_parameter(
    fixture: &Fixture,
    created: &serde_json::Value,
    parameter: &str,
) -> StatusCode {
    let authorization =
        url::Url::parse(created["data"]["authorization_url"].as_str().unwrap()).unwrap();
    let state = authorization
        .query_pairs()
        .find(|(name, _)| name == "state")
        .unwrap()
        .1
        .into_owned();
    let response = fixture
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/v1/oauth/doubao/callback?state={state}&{parameter}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()["referrer-policy"], "no-referrer");
    assert_eq!(response.headers()["cache-control"], "no-store");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
    let html = String::from_utf8_lossy(&bytes);
    assert!(!html.contains("fake-"));
    if let Some(error) = parameter.strip_prefix("error=") {
        assert!(!html.contains(error));
    }
    status
}
async fn claim(fixture: &Fixture, auth_id: Uuid, key: Uuid) -> (StatusCode, serde_json::Value) {
    json_request(&fixture.router,"POST","/v1/auth/token", serde_json::json!({"auth_id":auth_id.to_string(),"claim_verifier":URL_SAFE_NO_PAD.encode([23;32])}),Some(key)).await
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_router_complete_lifecycle_exact_create_and_claim_replay() {
    let fixture = fixture("ok", true).await;
    let auth_id = Uuid::now_v7();
    let created = create(&fixture, auth_id).await;
    let retried = create(&fixture, auth_id).await;
    assert_eq!(created["data"], retried["data"]);
    let (_, pending) = json_request(
        &fixture.router,
        "GET",
        &format!("/v1/auth/login-attempts/{auth_id}"),
        serde_json::Value::Null,
        None,
    )
    .await;
    assert_eq!(pending["data"]["state"], "pending");
    assert_eq!(callback(&fixture, &created).await, StatusCode::OK);
    let (_, ready) = json_request(
        &fixture.router,
        "GET",
        &format!("/v1/auth/login-attempts/{auth_id}"),
        serde_json::Value::Null,
        None,
    )
    .await;
    assert_eq!(ready["data"]["state"], "ready");
    let key = Uuid::now_v7();
    let (status, first) = claim(&fixture, auth_id, key).await;
    assert_eq!(status, StatusCode::OK);
    let (status, replay) = claim(&fixture, auth_id, key).await;
    assert_eq!(status, StatusCode::OK);
    assert!(first["data"] == replay["data"]);
    assert_eq!(fixture.provider.calls.load(Ordering::Relaxed), 1);
    let (_, conflict) = claim(&fixture, auth_id, Uuid::now_v7()).await;
    assert_eq!(conflict["error"]["code"], "idempotency_conflict");
    assert!(
        fixture
            .store
            .session(auth_id)
            .await
            .unwrap()
            .unwrap()
            .status
            == "active"
    );
    validate_contracts(&[
        (&created, "LoginCreateResponse"),
        (&ready, "LoginStatusResponse"),
        (&first, "TokenResponse"),
        (&conflict, "ErrorResponse"),
    ]);
}
fn validate_contracts(payloads: &[(&serde_json::Value, &str)]) {
    use std::io::Write;
    let mut child = std::process::Command::new(std::env::var("ASK_AUTH_TEST_PYTHON").unwrap_or_else(|_| "python3".into())).arg("-c").arg(
        "import json,sys,jsonschema; spec=json.load(open(sys.argv[1])); pairs=json.load(sys.stdin); [jsonschema.Draft202012Validator(dict(spec['components']['schemas'][name], components=spec['components']),format_checker=jsonschema.FormatChecker()).validate(payload) for payload,name in pairs]")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"),"/contracts/ask-auth-v1.openapi.json"))
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::piped()).spawn().expect("python3 and test jsonschema required");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(payloads).unwrap())
        .unwrap();
    assert!(child.wait().unwrap().success());
}

#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_callback_race_exchanges_exactly_once_and_claims_serialize() {
    let fixture = fixture("ok", true).await;
    let auth_id = Uuid::now_v7();
    let created = create(&fixture, auth_id).await;
    let (first, second) = tokio::join!(callback(&fixture, &created), callback(&fixture, &created));
    assert!(first == StatusCode::OK || second == StatusCode::OK);
    assert_eq!(fixture.provider.calls.load(Ordering::SeqCst), 1);
    let key = Uuid::now_v7();
    let (first, second) =
        tokio::join!(claim(&fixture, auth_id, key), claim(&fixture, auth_id, key));
    assert_eq!(first.0, StatusCode::OK);
    assert_eq!(second.0, StatusCode::OK);
    assert!(first.1["data"] == second.1["data"]);
    let (first, second) = tokio::join!(
        claim(&fixture, auth_id, Uuid::now_v7()),
        claim(&fixture, auth_id, Uuid::now_v7())
    );
    assert_eq!(first.1["error"]["code"], "idempotency_conflict");
    assert_eq!(second.1["error"]["code"], "idempotency_conflict");
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_denied_scope_and_unknown_provider_outcomes_are_sanitized() {
    for (outcome, is_allowed, reason, code) in [
        ("ok", false, "admission_denied", "login_denied"),
        ("scope", true, "invalid_scope", "login_denied"),
        (
            "failure",
            true,
            "login_outcome_unknown",
            "login_outcome_unknown",
        ),
    ] {
        let fixture = fixture(outcome, is_allowed).await;
        let auth_id = Uuid::now_v7();
        let created = create(&fixture, auth_id).await;
        assert_eq!(callback(&fixture, &created).await, StatusCode::OK);
        let row = fixture.store.session(auth_id).await.unwrap().unwrap();
        assert_eq!(row.reason_code.as_deref(), Some(reason));
        assert!(row.encrypted_oauth_context.is_none());
        assert!(row.encrypted_result.is_none());
        let (_, error) = claim(&fixture, auth_id, Uuid::now_v7()).await;
        assert_eq!(error["error"]["code"], code);
        validate_contracts(&[(&error, "ErrorResponse")]);
    }
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_callback_timeout_exceeds_bootstrap_timeout_and_commits_unknown() {
    let fixture = fixture("timeout", true).await;
    let auth_id = Uuid::now_v7();
    let created = create(&fixture, auth_id).await;
    assert_eq!(callback(&fixture, &created).await, StatusCode::OK);
    assert_eq!(fixture.provider.calls.load(Ordering::SeqCst), 1);
    let row = fixture.store.session(auth_id).await.unwrap().unwrap();
    assert_eq!(row.status, "error");
    assert_eq!(row.reason_code.as_deref(), Some("login_outcome_unknown"));
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_wrong_ticket_and_wrong_proof_hide_pending_denied_and_expired() {
    let fixture = fixture("ok", true).await;
    let auth_id = Uuid::now_v7();
    create(&fixture, auth_id).await;
    let (status, pending) = claim(&fixture, auth_id, Uuid::now_v7()).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(pending["error"]["code"], "login_pending");
    let (status,wrong) = json_request(&fixture.router,"POST","/v1/auth/token",serde_json::json!({"auth_id":auth_id.to_string(),"claim_verifier":URL_SAFE_NO_PAD.encode([99;32])}),Some(Uuid::now_v7())).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(wrong["error"]["code"], "authentication_required");
    set_expired(auth_id).await;
    let (status, expired) = claim(&fixture, auth_id, Uuid::now_v7()).await;
    assert_eq!(status, StatusCode::GONE);
    assert_eq!(expired["error"]["code"], "login_expired");
    let request = Request::builder()
        .uri(format!("/v1/auth/login-attempts/{auth_id}"))
        .header(
            "authorization",
            format!("Bearer {}", URL_SAFE_NO_PAD.encode([99; 32])),
        )
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        fixture.router.oneshot(request).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
}
async fn sql_connection() -> sqlx::MySqlConnection {
    use sqlx::ConnectOptions;
    let options: sqlx::mysql::MySqlConnectOptions = std::env::var("ASK_AUTH_TEST_MIGRATION_URL")
        .unwrap()
        .parse()
        .unwrap();
    options
        .ssl_mode(sqlx::mysql::MySqlSslMode::Required)
        .disable_statement_logging()
        .connect()
        .await
        .unwrap()
}
async fn set_expired(auth_id: Uuid) {
    sqlx::query("UPDATE auth_sessions SET login_expires_at=UTC_TIMESTAMP(3)-INTERVAL 1 SECOND WHERE auth_id=?")
        .bind(auth_id.as_bytes().as_slice()).execute(&mut sql_connection().await).await.unwrap();
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_changed_create_and_cleared_group_conflict_and_expired_retry_is_gone() {
    let fixture = fixture("ok", true).await;
    let auth_id = Uuid::now_v7();
    let created = create(&fixture, auth_id).await;
    let mut changed = creation(auth_id);
    changed["device_name"] = serde_json::json!("changed");
    let (status, _) = json_request(
        &fixture.router,
        "POST",
        "/v1/auth/login-attempts",
        changed,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    set_expired(auth_id).await;
    let (status, _) = json_request(
        &fixture.router,
        "POST",
        "/v1/auth/login-attempts",
        creation(auth_id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::GONE);
    assert_eq!(callback(&fixture, &created).await, StatusCode::GONE);
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_current_admission_blocks_claim_and_replay_without_extending_deadline() {
    let fixture = fixture("ok", true).await;
    let auth_id = Uuid::now_v7();
    let created = create(&fixture, auth_id).await;
    callback(&fixture, &created).await;
    fixture.admission.is_allowed.store(false, Ordering::SeqCst);
    assert_eq!(
        claim(&fixture, auth_id, Uuid::now_v7()).await.0,
        StatusCode::FORBIDDEN
    );
    fixture.admission.is_allowed.store(true, Ordering::SeqCst);
    let key = Uuid::now_v7();
    assert_eq!(claim(&fixture, auth_id, key).await.0, StatusCode::OK);
    let original = fixture.store.session(auth_id).await.unwrap().unwrap();
    fixture.admission.is_allowed.store(false, Ordering::SeqCst);
    assert_eq!(claim(&fixture, auth_id, key).await.0, StatusCode::FORBIDDEN);
    assert!(fixture.store.session(auth_id).await.unwrap().unwrap() == original);
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_authenticated_bundle_with_missing_nullable_field_fails_closed() {
    let fixture = fixture("ok", true).await;
    let auth_id = Uuid::now_v7();
    let created = create(&fixture, auth_id).await;
    callback(&fixture, &created).await;
    replace_bundle(&fixture, auth_id, |bundle| {
        bundle.as_object_mut().unwrap().remove("refresh_expires_at");
    })
    .await;
    let (status, _) = claim(&fixture, auth_id, Uuid::now_v7()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}
async fn replace_bundle(
    fixture: &Fixture,
    auth_id: Uuid,
    change: impl FnOnce(&mut serde_json::Value),
) {
    use ask_auth::crypto::Binding;
    let row = fixture.store.session(auth_id).await.unwrap().unwrap();
    let bytes = fixture
        .crypto
        .decrypt(
            &Binding::login(auth_id),
            row.encrypted_result.as_ref().unwrap(),
        )
        .unwrap();
    let mut bundle: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    change(&mut bundle);
    let encrypted = fixture
        .crypto
        .encrypt(
            1,
            &Binding::login(auth_id),
            &serde_json::to_vec(&bundle).unwrap(),
        )
        .unwrap();
    sqlx::query("UPDATE auth_sessions SET encrypted_result=? WHERE auth_id=?")
        .bind(encrypted.to_vec())
        .bind(auth_id.as_bytes().as_slice())
        .execute(&mut sql_connection().await)
        .await
        .unwrap();
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_active_expired_cache_poll_preserves_authorization_and_refresh_priority() {
    let fixture = fixture("ok", true).await;
    let auth_id = Uuid::now_v7();
    let created = create(&fixture, auth_id).await;
    callback(&fixture, &created).await;
    let key = Uuid::now_v7();
    assert_eq!(claim(&fixture, auth_id, key).await.0, StatusCode::OK);
    set_expired(auth_id).await;
    let (_, poll) = json_request(
        &fixture.router,
        "GET",
        &format!("/v1/auth/login-attempts/{auth_id}"),
        serde_json::Value::Null,
        None,
    )
    .await;
    assert_eq!(poll["data"]["state"], "expired");
    assert_eq!(poll["data"]["reason_code"], "claim_recovery_expired");
    let row = fixture.store.session(auth_id).await.unwrap().unwrap();
    assert_eq!(row.status, "active");
    let (_, error) = claim(&fixture, auth_id, key).await;
    assert_eq!(error["error"]["code"], "claim_recovery_expired");
    let request = RefreshRequest {
        auth_id,
        request_id: Uuid::now_v7(),
        request_hash: [1; 32],
        hmac_key_version: 1,
        input_generation: 1,
        status: "pending".into(),
        expires_at: Utc::now().naive_utc() + Duration::seconds(30),
        encrypted_response: None,
        error_code: None,
    };
    fixture
        .store
        .apply(Mutation::StartRefresh {
            expected: row,
            request,
        })
        .await
        .unwrap();
    let (_, error) = claim(&fixture, auth_id, key).await;
    assert_eq!(error["error"]["code"], "claim_recovery_expired");
}

#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_claim_waiting_on_policy_cannot_release_after_revoke() {
    let fixture = fixture("ok", true).await;
    let auth_id = Uuid::now_v7();
    let created = create(&fixture, auth_id).await;
    callback(&fixture, &created).await;
    fixture.admission.is_paused.store(true, Ordering::SeqCst);
    let operation = claim(&fixture, auth_id, Uuid::now_v7());
    tokio::pin!(operation);
    tokio::select! {
        _ = fixture.admission.entered.notified() => {},
        _ = &mut operation => panic!("claim must pause at policy retrieval"),
    }
    let row = fixture.store.session(auth_id).await.unwrap().unwrap();
    let mut revoked = row.clone();
    revoked.status = "revoked".into();
    revoked.clear_login();
    revoked.reason_code = Some("revoked".into());
    fixture
        .store
        .apply(Mutation::EndAuthorization {
            expected: row,
            next: revoked,
        })
        .await
        .unwrap();
    fixture.admission.release.notify_one();
    let (status, error) = operation.await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(error["error"]["code"], "authentication_required");
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_replay_waiting_on_policy_reclassifies_refresh_and_cleanup() {
    for is_cleanup in [false, true] {
        replay_policy_race(is_cleanup).await;
    }
}
async fn replay_policy_race(is_cleanup: bool) {
    let fixture = fixture("ok", true).await;
    let auth_id = Uuid::now_v7();
    let created = create(&fixture, auth_id).await;
    callback(&fixture, &created).await;
    let key = Uuid::now_v7();
    assert_eq!(claim(&fixture, auth_id, key).await.0, StatusCode::OK);
    fixture.admission.is_paused.store(true, Ordering::SeqCst);
    let operation = claim(&fixture, auth_id, key);
    tokio::pin!(operation);
    tokio::select! {
        _ = fixture.admission.entered.notified() => {},
        _ = &mut operation => panic!("replay must pause at policy retrieval"),
    }
    mutate_replay_race(&fixture, auth_id, is_cleanup).await;
    fixture.admission.release.notify_one();
    let (status, error) = operation.await;
    assert_eq!(
        status,
        if is_cleanup {
            StatusCode::UNAUTHORIZED
        } else {
            StatusCode::CONFLICT
        }
    );
    assert_eq!(
        error["error"]["code"],
        if is_cleanup {
            "authentication_required"
        } else {
            "refresh_in_progress"
        }
    );
}

async fn mutate_replay_race(fixture: &Fixture, auth_id: Uuid, is_cleanup: bool) {
    if is_cleanup {
        set_expired(auth_id).await;
    }
    let row = fixture.store.session(auth_id).await.unwrap().unwrap();
    if is_cleanup {
        fixture
            .store
            .apply(Mutation::ClearLogin { expected: row })
            .await
            .unwrap();
    } else {
        fixture
            .store
            .apply(Mutation::StartRefresh {
                request: refresh_request(auth_id),
                expected: row,
            })
            .await
            .unwrap();
    }
}

fn refresh_request(auth_id: Uuid) -> RefreshRequest {
    RefreshRequest {
        auth_id,
        request_id: Uuid::now_v7(),
        request_hash: [1; 32],
        hmac_key_version: 1,
        input_generation: 1,
        status: "pending".into(),
        expires_at: Utc::now().naive_utc() + Duration::seconds(30),
        encrypted_response: None,
        error_code: None,
    }
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_late_callback_cannot_mutate_recreated_authorization_id() {
    let fixture = fixture("ok", true).await;
    let auth_id = Uuid::now_v7();
    let created = create(&fixture, auth_id).await;
    fixture.provider.is_paused.store(true, Ordering::SeqCst);
    let operation = callback(&fixture, &created);
    tokio::pin!(operation);
    tokio::select! {
        _ = fixture.provider.entered.notified() => {},
        _ = &mut operation => panic!("callback must pause after reservation"),
    }
    let row = fixture.store.session(auth_id).await.unwrap().unwrap();
    assert_eq!(row.status, "exchanging");
    sqlx::query("DELETE FROM auth_sessions WHERE auth_id=?")
        .bind(auth_id.as_bytes().as_slice())
        .execute(&mut sql_connection().await)
        .await
        .unwrap();
    let recreated = create(&fixture, auth_id).await;
    assert!(created["data"]["authorization_url"] != recreated["data"]["authorization_url"]);
    fixture.provider.release.notify_one();
    assert_eq!(operation.await, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        fixture
            .store
            .session(auth_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "pending"
    );
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_cancelled_callback_stays_exchanging_and_duplicate_cannot_take_over() {
    let fixture = fixture("ok", true).await;
    let auth_id = Uuid::now_v7();
    let created = create(&fixture, auth_id).await;
    fixture.provider.is_paused.store(true, Ordering::SeqCst);
    {
        let operation = callback(&fixture, &created);
        tokio::pin!(operation);
        tokio::select! {
            _ = fixture.provider.entered.notified() => {},
            _ = &mut operation => panic!("callback must pause after reservation"),
        }
    }
    assert_eq!(
        fixture
            .store
            .session(auth_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "exchanging"
    );
    assert_eq!(callback(&fixture, &created).await, StatusCode::BAD_REQUEST);
    assert_eq!(fixture.provider.calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_tampered_ciphertext_and_mismatched_bundle_metadata_fail_closed() {
    for variation in [
        "identity",
        "generation",
        "broker",
        "scope",
        "tenant_missing",
        "tamper",
    ] {
        let fixture = fixture("ok", true).await;
        let auth_id = Uuid::now_v7();
        let created = create(&fixture, auth_id).await;
        callback(&fixture, &created).await;
        if variation == "tamper" {
            let row = fixture.store.session(auth_id).await.unwrap().unwrap();
            let mut envelope = row.encrypted_result.unwrap();
            envelope[18] ^= 1;
            sqlx::query("UPDATE auth_sessions SET encrypted_result=? WHERE auth_id=?")
                .bind(envelope)
                .bind(auth_id.as_bytes().as_slice())
                .execute(&mut sql_connection().await)
                .await
                .unwrap();
        } else {
            replace_bundle(&fixture, auth_id, |bundle| match variation {
                "identity" => bundle["identity"]["user_id"] = serde_json::json!("other-user"),
                "generation" => bundle["credential_generation"] = serde_json::json!(2),
                "broker" => {
                    bundle["broker_token"] = serde_json::json!(URL_SAFE_NO_PAD.encode([9; 32]))
                }
                "scope" => bundle["scope"] = serde_json::json!(["other.scope"]),
                "tenant_missing" => {
                    bundle["identity"]
                        .as_object_mut()
                        .unwrap()
                        .remove("tenant_id");
                }
                _ => unreachable!(),
            })
            .await;
        }
        assert_eq!(
            claim(&fixture, auth_id, Uuid::now_v7()).await.0,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}

#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_wrong_ticket_is_generic_before_ciphertext_authentication() {
    let fixture = fixture("ok", true).await;
    let auth_id = Uuid::now_v7();
    let created = create(&fixture, auth_id).await;
    callback(&fixture, &created).await;
    let row = fixture.store.session(auth_id).await.unwrap().unwrap();
    let mut envelope = row.encrypted_result.unwrap();
    envelope[18] ^= 1;
    sqlx::query("UPDATE auth_sessions SET encrypted_result=? WHERE auth_id=?")
        .bind(envelope)
        .bind(auth_id.as_bytes().as_slice())
        .execute(&mut sql_connection().await)
        .await
        .unwrap();
    let request = Request::builder()
        .uri(format!("/v1/auth/login-attempts/{auth_id}"))
        .header(
            "authorization",
            format!("Bearer {}", URL_SAFE_NO_PAD.encode([99; 32])),
        )
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request)
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        claim(&fixture, auth_id, Uuid::now_v7()).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_callback_denial_reserves_without_provider_and_cleared_create_stays_closed() {
    let fixture = fixture("ok", true).await;
    let auth_id = Uuid::now_v7();
    let created = create(&fixture, auth_id).await;
    assert_eq!(
        callback_parameter(&fixture, &created, "error=access_denied").await,
        StatusCode::OK
    );
    assert_eq!(fixture.provider.calls.load(Ordering::SeqCst), 0);
    let row = fixture.store.session(auth_id).await.unwrap().unwrap();
    assert_eq!(row.status, "denied");
    assert_eq!(row.reason_code.as_deref(), Some("access_denied"));
    assert!(row.encrypted_oauth_context.is_none());
    assert_eq!(
        claim(&fixture, auth_id, Uuid::now_v7()).await.0,
        StatusCode::FORBIDDEN
    );
    let next_id = Uuid::now_v7();
    let created = create(&fixture, next_id).await;
    callback(&fixture, &created).await;
    claim(&fixture, next_id, Uuid::now_v7()).await;
    set_expired(next_id).await;
    let row = fixture.store.session(next_id).await.unwrap().unwrap();
    fixture
        .store
        .apply(Mutation::ClearLogin { expected: row })
        .await
        .unwrap();
    assert_eq!(
        json_request(
            &fixture.router,
            "POST",
            "/v1/auth/login-attempts",
            creation(next_id),
            None
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        claim(&fixture, next_id, Uuid::now_v7()).await.0,
        StatusCode::UNAUTHORIZED
    );
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_new_aead_envelope_keeps_original_hmac_version_and_replays() {
    use ask_auth::crypto::Binding;
    let fixture = fixture("ok", true).await;
    let auth_id = Uuid::now_v7();
    let created = create(&fixture, auth_id).await;
    callback(&fixture, &created).await;
    let row = fixture.store.session(auth_id).await.unwrap().unwrap();
    let plaintext = fixture
        .crypto
        .decrypt(
            &Binding::login(auth_id),
            row.encrypted_result.as_ref().unwrap(),
        )
        .unwrap();
    let envelope = fixture
        .crypto
        .encrypt(2, &Binding::login(auth_id), &plaintext)
        .unwrap();
    sqlx::query("UPDATE auth_sessions SET encrypted_result=? WHERE auth_id=?")
        .bind(envelope.to_vec())
        .bind(auth_id.as_bytes().as_slice())
        .execute(&mut sql_connection().await)
        .await
        .unwrap();
    let key = Uuid::now_v7();
    assert_eq!(claim(&fixture, auth_id, key).await.0, StatusCode::OK);
    assert_eq!(claim(&fixture, auth_id, key).await.0, StatusCode::OK);
    assert_eq!(
        fixture
            .store
            .session(auth_id)
            .await
            .unwrap()
            .unwrap()
            .hmac_key_version,
        1
    );
}

#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_callback_provider_errors_are_terminal_errors_without_exchange() {
    for error in [
        "server_error",
        "temporarily_unavailable",
        "unknown-provider-error-SENTINEL",
    ] {
        let fixture = fixture("ok", true).await;
        let auth_id = Uuid::now_v7();
        let created = create(&fixture, auth_id).await;
        let original = fixture.store.session(auth_id).await.unwrap().unwrap();
        let callback_started =
            sqlx::query_scalar::<_, chrono::NaiveDateTime>("SELECT UTC_TIMESTAMP(3)")
                .fetch_one(&mut sql_connection().await)
                .await
                .unwrap();
        assert_eq!(
            callback_parameter(&fixture, &created, &format!("error={error}")).await,
            StatusCode::OK
        );
        assert_eq!(fixture.provider.calls.load(Ordering::SeqCst), 0);
        let ended = fixture.store.session(auth_id).await.unwrap().unwrap();
        assert_eq!(ended.status, "error");
        assert_eq!(ended.reason_code.as_deref(), Some("provider_error"));
        let callback_ended =
            sqlx::query_scalar::<_, chrono::NaiveDateTime>("SELECT UTC_TIMESTAMP(3)")
                .fetch_one(&mut sql_connection().await)
                .await
                .unwrap();
        assert!(ended.login_expires_at >= Some(callback_started));
        assert!(ended.login_expires_at <= Some(callback_ended));
        assert!(ended.login_expires_at < original.login_expires_at);
        let mut expected = original;
        expected.status = "error".into();
        expected.reason_code = Some("provider_error".into());
        expected.encrypted_oauth_context = None;
        expected.login_expires_at = ended.login_expires_at;
        assert!(ended == expected);
        let (status, response) = claim(&fixture, auth_id, Uuid::now_v7()).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response["error"]["code"], "upstream_unavailable");
        assert!(!response.to_string().contains(error));
        validate_contracts(&[(&response, "ErrorResponse")]);
    }
}

#[derive(Clone, Copy)]
enum LoadedPolicy {
    Fresh,
    Default,
    Disabled,
    Stale,
    Future,
    SourceFailure,
}
struct LoadedAdmission {
    policy: LoadedPolicy,
}
impl AdmissionProvider for LoadedAdmission {
    fn snapshot(&self) -> AdmissionFuture<'_> {
        Box::pin(async {
            tokio::time::sleep(Wait::from_millis(30)).await;
            if matches!(self.policy, LoadedPolicy::SourceFailure) {
                return Err(AdmissionError);
            }
            let fetched_at =
                sqlx::query_scalar::<_, chrono::NaiveDateTime>("SELECT UTC_TIMESTAMP(3)")
                    .fetch_one(&mut sql_connection().await)
                    .await
                    .unwrap();
            let mut snapshot = AdmissionSnapshot::deny_all(fetched_at);
            if matches!(self.policy, LoadedPolicy::Default) {
                return Ok(snapshot);
            }
            snapshot.is_enabled = !matches!(self.policy, LoadedPolicy::Disabled);
            snapshot.version = "loaded-policy-1".into();
            match self.policy {
                LoadedPolicy::Stale => snapshot.fetched_at -= Duration::minutes(6),
                LoadedPolicy::Future => snapshot.fetched_at += Duration::minutes(1),
                _ => {}
            }
            Ok(snapshot)
        })
    }
}
async fn loaded_readiness_router(policy: LoadedPolicy) -> Router {
    let fixture = fixture("ok", true).await;
    let service = LoginService::new(
        fixture.store,
        fixture.crypto,
        enabled_config(),
        LoginAdapters {
            provider: fixture.provider,
            admission: Arc::new(LoadedAdmission { policy }),
        },
    )
    .unwrap();
    build_enabled_router(Arc::new(service))
}
async fn readiness_response(router: Router) -> (StatusCode, serde_json::Value) {
    let response = router
        .oneshot(
            Request::builder()
                .uri("/readyz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(
        Uuid::parse_str(response.headers()["x-request-id"].to_str().unwrap())
            .unwrap()
            .get_version_num(),
        7
    );
    let status = response.status();
    let payload =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    (status, payload)
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_readiness_uses_database_time_after_delayed_policy_load() {
    let (status, payload) =
        readiness_response(loaded_readiness_router(LoadedPolicy::Fresh).await).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload, serde_json::json!({"status":"ok"}));
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_readiness_rejects_default_disabled_stale_future_and_failed_policy() {
    for policy in [
        LoadedPolicy::Default,
        LoadedPolicy::Disabled,
        LoadedPolicy::Stale,
        LoadedPolicy::Future,
        LoadedPolicy::SourceFailure,
    ] {
        let (status, payload) = readiness_response(loaded_readiness_router(policy).await).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(payload, serde_json::json!({"status":"unavailable"}));
    }
}

async fn broker_request(
    fixture: &Fixture,
    method: &str,
    uri: &str,
    payload: serde_json::Value,
    proof: (&str, Option<Uuid>),
) -> (StatusCode, serde_json::Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {}", proof.0));
    if let Some(key) = proof.1 {
        request = request.header("idempotency-key", key.to_string());
    }
    let response = fixture
        .router
        .clone()
        .oneshot(request.body(Body::from(payload.to_string())).unwrap())
        .await
        .unwrap();
    assert_eq!(response.headers()["cache-control"], "no-store");
    let trace = response.headers()["x-request-id"]
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(Uuid::parse_str(&trace).unwrap().get_version_num(), 7);
    let status = response.status();
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 262144).await.unwrap()).unwrap();
    assert_eq!(body["request_id"], trace);
    (status, body)
}
async fn claimed_bundle(fixture: &Fixture) -> serde_json::Value {
    let auth_id = Uuid::now_v7();
    let created = create(fixture, auth_id).await;
    assert_eq!(callback(fixture, &created).await, StatusCode::OK);
    let (status, claimed) = claim(fixture, auth_id, Uuid::now_v7()).await;
    assert_eq!(status, StatusCode::OK);
    claimed["data"].clone()
}
fn refresh_payload(bundle: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({"auth_id":bundle["auth_id"],"refresh_token":bundle["refresh_token"],
        "credential_generation":bundle["credential_generation"]})
}
async fn refresh_bundle(
    fixture: &Fixture,
    bundle: &serde_json::Value,
    key: Uuid,
) -> (StatusCode, serde_json::Value) {
    broker_request(
        fixture,
        "POST",
        "/v1/auth/refresh",
        refresh_payload(bundle),
        (bundle["broker_token"].as_str().unwrap(), Some(key)),
    )
    .await
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_lifecycle_login_status_refresh_replay_logout() {
    let fixture = fixture("ok", true).await;
    let bundle = claimed_bundle(&fixture).await;
    let auth_id = bundle["auth_id"].as_str().unwrap();
    let broker = bundle["broker_token"].as_str().unwrap();
    let (status, online) = broker_request(
        &fixture,
        "GET",
        &format!("/v1/auth/status?auth_id={auth_id}"),
        serde_json::Value::Null,
        (broker, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(online["data"]["status"], "active");
    let key = Uuid::now_v7();
    let (status, refreshed) = refresh_bundle(&fixture, &bundle, key).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(refreshed["data"]["credential_generation"], 2);
    let (status, replay) = refresh_bundle(&fixture, &bundle, key).await;
    assert_eq!(status, StatusCode::OK);
    // [Review Fix #37] Test failures must never format complete credential bundles.
    assert!(replay["data"] == refreshed["data"]);
    let (status, logged_out) = broker_request(
        &fixture,
        "POST",
        "/v1/auth/logout",
        serde_json::json!({"auth_id":auth_id}),
        (refreshed["data"]["broker_token"].as_str().unwrap(), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    validate_contracts(&[
        (&online, "StatusResponse"),
        (&refreshed, "TokenResponse"),
        (&replay, "TokenResponse"),
        (&logged_out, "LogoutResponse"),
    ]);
}

fn set_refresh_outcome(fixture: &Fixture, outcome: &'static str) {
    *fixture.provider.refresh_outcome.lock().unwrap() = outcome;
}
async fn online(fixture: &Fixture, bundle: &serde_json::Value) -> (StatusCode, serde_json::Value) {
    broker_request(
        fixture,
        "GET",
        &format!(
            "/v1/auth/status?auth_id={}",
            bundle["auth_id"].as_str().unwrap()
        ),
        serde_json::Value::Null,
        (bundle["broker_token"].as_str().unwrap(), None),
    )
    .await
}
async fn logout_bundle(
    fixture: &Fixture,
    bundle: &serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    broker_request(
        fixture,
        "POST",
        "/v1/auth/logout",
        serde_json::json!({"auth_id":bundle["auth_id"]}),
        (bundle["broker_token"].as_str().unwrap(), None),
    )
    .await
}
async fn session_for(fixture: &Fixture, bundle: &serde_json::Value) -> Session {
    fixture
        .store
        .session(Uuid::parse_str(bundle["auth_id"].as_str().unwrap()).unwrap())
        .await
        .unwrap()
        .unwrap()
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_refresh_same_key_and_other_key_never_exchange_while_busy() {
    let fixture = fixture("ok", true).await;
    let bundle = claimed_bundle(&fixture).await;
    let key = Uuid::now_v7();
    fixture.provider.is_paused.store(true, Ordering::SeqCst);
    let operation = refresh_bundle(&fixture, &bundle, key);
    tokio::pin!(operation);
    tokio::select! { _ = fixture.provider.entered.notified() => {}, _ = &mut operation => panic!("must reserve before pausing") }
    for request_key in [key, Uuid::now_v7()] {
        let (status, response) = refresh_bundle(&fixture, &bundle, request_key).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(response["error"]["code"], "refresh_in_progress");
    }
    assert_eq!(
        online(&fixture, &bundle).await.1["data"]["status"],
        "refreshing"
    );
    assert_eq!(fixture.provider.refresh_calls.load(Ordering::SeqCst), 1);
    fixture.provider.release.notify_one();
    assert_eq!(operation.await.0, StatusCode::OK);
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_refresh_proof_precedes_retained_state_and_output_generation_is_current() {
    let fixture = fixture("ok", true).await;
    let bundle = claimed_bundle(&fixture).await;
    let key = Uuid::now_v7();
    let (_, refreshed) = refresh_bundle(&fixture, &bundle, key).await;
    assert_eq!(online(&fixture, &bundle).await.0, StatusCode::UNAUTHORIZED);
    for field in ["refresh_token", "broker_token", "credential_generation"] {
        let mut changed = bundle.clone();
        changed[field] = if field == "credential_generation" {
            serde_json::json!(99)
        } else if field == "broker_token" {
            serde_json::json!(URL_SAFE_NO_PAD.encode([88; 32]))
        } else {
            serde_json::json!("wrong-refresh")
        };
        assert_eq!(
            refresh_bundle(&fixture, &changed, key).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        refresh_bundle(&fixture, &bundle, key).await.0,
        StatusCode::OK
    );
    assert_eq!(
        refresh_bundle(&fixture, &refreshed["data"], Uuid::now_v7())
            .await
            .0,
        StatusCode::OK
    );
    let (status, old) = refresh_bundle(&fixture, &bundle, key).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(old["error"]["code"], "credential_generation_mismatch");
    assert_eq!(fixture.provider.refresh_calls.load(Ordering::SeqCst), 2);
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_refresh_success_clears_login_claim_cache() {
    let fixture = fixture("ok", true).await;
    let auth_id = Uuid::now_v7();
    let created = create(&fixture, auth_id).await;
    callback(&fixture, &created).await;
    let claim_key = Uuid::now_v7();
    let (_, claimed) = claim(&fixture, auth_id, claim_key).await;
    assert_eq!(
        refresh_bundle(&fixture, &claimed["data"], Uuid::now_v7())
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        claim(&fixture, auth_id, claim_key).await.0,
        StatusCode::UNAUTHORIZED
    );
    let row = session_for(&fixture, &claimed["data"]).await;
    assert!(
        row.login_ticket_hash.is_none()
            && row.login_expires_at.is_none()
            && row.encrypted_result.is_none()
    );
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_refresh_uncertain_or_invalid_contract_never_restores_active() {
    for outcome in [
        "unknown",
        "identity",
        "tenant",
        "expanded",
        "dropped",
        "duplicate",
        "empty_at",
        "empty_rt",
        "expired",
    ] {
        let fixture = fixture("ok", true).await;
        let bundle = claimed_bundle(&fixture).await;
        set_refresh_outcome(&fixture, outcome);
        let key = Uuid::now_v7();
        let (status, error) = refresh_bundle(&fixture, &bundle, key).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(error["error"]["code"], "refresh_outcome_unknown");
        assert_eq!(
            session_for(&fixture, &bundle).await.status,
            "reauth_required"
        );
        assert_eq!(
            refresh_bundle(&fixture, &bundle, key).await.1["error"]["code"],
            "refresh_outcome_unknown"
        );
        assert_eq!(fixture.provider.refresh_calls.load(Ordering::SeqCst), 1);
        validate_contracts(&[(&error, "ErrorResponse")]);
    }
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_refresh_proven_nonconsumption_is_failed_and_replayed_once() {
    for (outcome, expected, status) in [
        ("unavailable", "active", StatusCode::SERVICE_UNAVAILABLE),
        ("contract", "active", StatusCode::BAD_GATEWAY),
        ("invalid", "reauth_required", StatusCode::UNAUTHORIZED),
    ] {
        let fixture = fixture("ok", true).await;
        let bundle = claimed_bundle(&fixture).await;
        let original = session_for(&fixture, &bundle).await;
        set_refresh_outcome(&fixture, outcome);
        let key = Uuid::now_v7();
        assert_eq!(refresh_bundle(&fixture, &bundle, key).await.0, status);
        let row = session_for(&fixture, &bundle).await;
        assert_eq!(row.status, expected);
        assert!(
            row.broker_hash == original.broker_hash
                && row.refresh_token_hash == original.refresh_token_hash
        );
        assert_eq!(row.credential_generation, original.credential_generation);
        assert_eq!(row.hmac_key_version, original.hmac_key_version);
        assert_eq!(refresh_bundle(&fixture, &bundle, key).await.0, status);
        assert_eq!(fixture.provider.refresh_calls.load(Ordering::SeqCst), 1);
    }
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_refresh_unknown_expiry_cannot_extend_previous_authorization_bound() {
    let fixture = fixture("ok", true).await;
    let bundle = claimed_bundle(&fixture).await;
    set_refresh_outcome(&fixture, "unknown_expiry");
    let (status, next) = refresh_bundle(&fixture, &bundle, Uuid::now_v7()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(next["data"]["refresh_expires_at"].is_null());
    assert_eq!(
        next["data"]["broker_expires_at"],
        bundle["broker_expires_at"]
    );
    validate_contracts(&[(&next, "TokenResponse")]);
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_refresh_can_narrow_scope_but_keep_required_scope() {
    let fixture = fixture("wide", true).await;
    let bundle = claimed_bundle(&fixture).await;
    set_refresh_outcome(&fixture, "narrow");
    let (status, next) = refresh_bundle(&fixture, &bundle, Uuid::now_v7()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(next["data"]["scope"], serde_json::json!(["files.read"]));
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_logout_during_refresh_settles_pending_and_rejects_late_success() {
    let fixture = fixture("ok", true).await;
    let bundle = claimed_bundle(&fixture).await;
    let key = Uuid::now_v7();
    fixture.provider.is_paused.store(true, Ordering::SeqCst);
    let operation = refresh_bundle(&fixture, &bundle, key);
    tokio::pin!(operation);
    tokio::select! { _ = fixture.provider.entered.notified() => {}, _ = &mut operation => panic!("must reserve") }
    fixture.admission.is_allowed.store(false, Ordering::SeqCst);
    assert_eq!(logout_bundle(&fixture, &bundle).await.0, StatusCode::OK);
    let auth_id = Uuid::parse_str(bundle["auth_id"].as_str().unwrap()).unwrap();
    let (ended, child) = fixture.store.refresh(auth_id, key).await.unwrap().unwrap();
    assert_eq!(ended.status, "revoked");
    assert_eq!(child.status, "unknown");
    fixture.provider.release.notify_one();
    assert_ne!(operation.await.0, StatusCode::OK);
    assert_eq!(logout_bundle(&fixture, &bundle).await.0, StatusCode::OK);
    let (_, status) = online(&fixture, &bundle).await;
    assert_eq!(status["data"]["status"], "revoked");
    assert_eq!(status["data"]["is_allowed"], false);
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_cancelled_refresh_stays_pending_without_takeover() {
    let fixture = fixture("ok", true).await;
    let bundle = claimed_bundle(&fixture).await;
    let key = Uuid::now_v7();
    fixture.provider.is_paused.store(true, Ordering::SeqCst);
    {
        let operation = refresh_bundle(&fixture, &bundle, key);
        tokio::pin!(operation);
        tokio::select! { _ = fixture.provider.entered.notified() => {}, _ = &mut operation => panic!("must reserve") }
    }
    assert_eq!(session_for(&fixture, &bundle).await.status, "refreshing");
    assert_eq!(
        refresh_bundle(&fixture, &bundle, key).await.1["error"]["code"],
        "refresh_in_progress"
    );
    assert_eq!(fixture.provider.refresh_calls.load(Ordering::SeqCst), 1);
}

async fn rewrite_sql(auth_id: Uuid, statement: &'static str) {
    sqlx::query(statement)
        .bind(auth_id.as_bytes().as_slice())
        .execute(&mut sql_connection().await)
        .await
        .unwrap();
}
fn bundle_id(bundle: &serde_json::Value) -> Uuid {
    Uuid::parse_str(bundle["auth_id"].as_str().unwrap()).unwrap()
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_status_wrong_id_or_proof_and_unclaimed_do_not_disclose_identity() {
    let fixture = fixture("ok", true).await;
    let bundle = claimed_bundle(&fixture).await;
    let auth_id = Uuid::now_v7();
    let created = create(&fixture, auth_id).await;
    callback(&fixture, &created).await;
    let row = fixture.store.session(auth_id).await.unwrap().unwrap();
    let plaintext = fixture
        .crypto
        .decrypt(
            &ask_auth::crypto::Binding::login(auth_id),
            row.encrypted_result.as_ref().unwrap(),
        )
        .unwrap();
    let ready: serde_json::Value = serde_json::from_slice(&plaintext).unwrap();
    let mut wrong_id = bundle.clone();
    wrong_id["auth_id"] = serde_json::json!(Uuid::now_v7().to_string());
    let mut wrong_broker = bundle.clone();
    wrong_broker["broker_token"] = serde_json::json!(URL_SAFE_NO_PAD.encode([99; 32]));
    for input in [&wrong_id, &wrong_broker, &ready] {
        for response in [
            online(&fixture, input).await,
            logout_bundle(&fixture, input).await,
            refresh_bundle(&fixture, input, Uuid::now_v7()).await,
        ] {
            assert_eq!(response.0, StatusCode::UNAUTHORIZED);
            assert_eq!(response.1["error"]["code"], "authentication_required");
            assert!(!response.1.to_string().contains("verified-user"));
            validate_contracts(&[(&response.1, "ErrorResponse")]);
        }
    }
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_status_natural_expiry_and_logout_keep_terminal_reason() {
    let fixture = fixture("ok", true).await;
    let bundle = claimed_bundle(&fixture).await;
    rewrite_sql(bundle_id(&bundle),"UPDATE auth_sessions SET auth_expires_at=UTC_TIMESTAMP(3)-INTERVAL 1 SECOND WHERE auth_id=?").await;
    let (status, expired) = online(&fixture, &bundle).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(expired["data"]["status"], "expired");
    assert_eq!(expired["data"]["is_allowed"], false);
    let before = session_for(&fixture, &bundle).await;
    fixture.admission.has_failure.store(true, Ordering::SeqCst);
    assert_eq!(logout_bundle(&fixture, &bundle).await.0, StatusCode::OK);
    assert!(session_for(&fixture, &bundle).await == before);
    assert_eq!(
        online(&fixture, &bundle).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    validate_contracts(&[(&expired, "StatusResponse")]);
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_refresh_overflow_and_authenticated_generation_mismatch_never_exchange() {
    let fixture = fixture("ok", true).await;
    let mut bundle = claimed_bundle(&fixture).await;
    let mut wrong = bundle.clone();
    wrong["credential_generation"] = serde_json::json!(2);
    assert_eq!(
        refresh_bundle(&fixture, &wrong, Uuid::now_v7()).await.1["error"]["code"],
        "credential_generation_mismatch"
    );
    rewrite_sql(bundle_id(&bundle),"UPDATE auth_sessions SET credential_generation=18446744073709551615,request_hash=NULL,login_ticket_hash=NULL,claim_challenge=NULL,oauth_state_hash=NULL,encrypted_oauth_context=NULL,claim_request_id=NULL,encrypted_result=NULL,login_expires_at=NULL WHERE auth_id=?").await;
    bundle["credential_generation"] = serde_json::json!(u64::MAX);
    let (status, error) = refresh_bundle(&fixture, &bundle, Uuid::now_v7()).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["error"]["code"], "credential_generation_mismatch");
    assert_eq!(fixture.provider.refresh_calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_refresh_retained_expired_request_does_not_exchange_again() {
    let fixture = fixture("ok", true).await;
    let bundle = claimed_bundle(&fixture).await;
    let key = Uuid::now_v7();
    assert_eq!(
        refresh_bundle(&fixture, &bundle, key).await.0,
        StatusCode::OK
    );
    rewrite_sql(bundle_id(&bundle),"UPDATE auth_refresh_requests SET expires_at=UTC_TIMESTAMP(3)-INTERVAL 1 SECOND WHERE auth_id=?").await;
    let (status, error) = refresh_bundle(&fixture, &bundle, key).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["error"]["code"], "refresh_recovery_expired");
    assert_eq!(fixture.provider.refresh_calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_policy_removal_blocks_new_refresh_and_cached_release_but_not_logout() {
    let fixture = fixture("ok", true).await;
    let bundle = claimed_bundle(&fixture).await;
    fixture.admission.is_allowed.store(false, Ordering::SeqCst);
    assert_eq!(
        refresh_bundle(&fixture, &bundle, Uuid::now_v7()).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        online(&fixture, &bundle).await.1["data"]["is_allowed"],
        false
    );
    fixture.admission.is_allowed.store(true, Ordering::SeqCst);
    let key = Uuid::now_v7();
    let (_, next) = refresh_bundle(&fixture, &bundle, key).await;
    fixture.admission.is_allowed.store(false, Ordering::SeqCst);
    assert_eq!(
        refresh_bundle(&fixture, &bundle, key).await.0,
        StatusCode::FORBIDDEN
    );
    fixture.admission.has_failure.store(true, Ordering::SeqCst);
    assert_eq!(
        logout_bundle(&fixture, &next["data"]).await.0,
        StatusCode::OK
    );
    assert_eq!(fixture.provider.refresh_calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_status_final_snapshot_hides_identity_from_rotated_broker() {
    let fixture = fixture("ok", true).await;
    let bundle = claimed_bundle(&fixture).await;
    fixture.admission.is_paused.store(true, Ordering::SeqCst);
    let operation = online(&fixture, &bundle);
    tokio::pin!(operation);
    tokio::select! { _ = fixture.admission.entered.notified() => {}, _ = &mut operation => panic!("must pause") }
    assert_eq!(
        refresh_bundle(&fixture, &bundle, Uuid::now_v7()).await.0,
        StatusCode::OK
    );
    fixture.admission.release.notify_one();
    let (status, response) = operation.await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(!response.to_string().contains("verified-user"));
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_refresh_final_release_rechecks_revoke_and_second_refresh() {
    for is_second_refresh in [false, true] {
        let fixture = fixture("ok", true).await;
        let bundle = claimed_bundle(&fixture).await;
        fixture.admission.pause_at_call.store(
            fixture.admission.calls.load(Ordering::SeqCst) + 3,
            Ordering::SeqCst,
        );
        let key = Uuid::now_v7();
        let operation = refresh_bundle(&fixture, &bundle, key);
        tokio::pin!(operation);
        tokio::select! { _ = fixture.admission.entered.notified() => {}, _ = &mut operation => panic!("must pause final release") }
        let pair = fixture
            .store
            .refresh(bundle_id(&bundle), key)
            .await
            .unwrap()
            .unwrap();
        let binding = ask_auth::crypto::Binding::refresh(bundle_id(&bundle), key, 1).unwrap();
        let plaintext = fixture
            .crypto
            .decrypt(&binding, pair.1.encrypted_response.as_ref().unwrap())
            .unwrap();
        let next: serde_json::Value = serde_json::from_slice(&plaintext).unwrap();
        if is_second_refresh {
            assert_eq!(
                refresh_bundle(&fixture, &next, Uuid::now_v7()).await.0,
                StatusCode::OK
            );
        } else {
            assert_eq!(logout_bundle(&fixture, &next).await.0, StatusCode::OK);
        }
        fixture.admission.release.notify_one();
        let (status, response) = operation.await;
        if is_second_refresh {
            assert_eq!(status, StatusCode::CONFLICT);
            assert_eq!(response["error"]["code"], "credential_generation_mismatch");
        } else {
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert_eq!(response["error"]["code"], "broker_expired");
        }
    }
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_refresh_timeout_becomes_unknown_without_retry() {
    let fixture = fixture("ok", true).await;
    let bundle = claimed_bundle(&fixture).await;
    set_refresh_outcome(&fixture, "timeout");
    let key = Uuid::now_v7();
    let started = std::time::Instant::now();
    assert_eq!(
        refresh_bundle(&fixture, &bundle, key).await.1["error"]["code"],
        "refresh_outcome_unknown"
    );
    assert!(started.elapsed() >= Wait::from_secs(10) && started.elapsed() < Wait::from_secs(15));
    assert_eq!(
        session_for(&fixture, &bundle).await.status,
        "reauth_required"
    );
    assert_eq!(
        refresh_bundle(&fixture, &bundle, key).await.1["error"]["code"],
        "refresh_outcome_unknown"
    );
    assert_eq!(fixture.provider.refresh_calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_refresh_late_changed_lease_cannot_publish_any_outcome() {
    for outcome in ["ok", "unavailable", "unknown"] {
        let fixture = fixture("ok", true).await;
        let bundle = claimed_bundle(&fixture).await;
        set_refresh_outcome(&fixture, outcome);
        fixture.provider.is_paused.store(true, Ordering::SeqCst);
        let key = Uuid::now_v7();
        let operation = refresh_bundle(&fixture, &bundle, key);
        tokio::pin!(operation);
        tokio::select! { _ = fixture.provider.entered.notified() => {}, _ = &mut operation => panic!("must reserve") }
        rewrite_sql(bundle_id(&bundle),"UPDATE auth_refresh_requests SET expires_at=UTC_TIMESTAMP(3)-INTERVAL 1 SECOND WHERE auth_id=?").await;
        fixture.provider.release.notify_one();
        assert_ne!(operation.await.0, StatusCode::OK);
        let pair = fixture
            .store
            .refresh(bundle_id(&bundle), key)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(pair.0.status, "refreshing");
        assert_eq!(pair.1.status, "pending");
    }
}

#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_status_disabled_current_policy_reports_not_allowed_without_backend_error() {
    let fixture = fixture("ok", true).await;
    let bundle = claimed_bundle(&fixture).await;
    fixture.admission.is_enabled.store(false, Ordering::SeqCst);
    let (status, response) = online(&fixture, &bundle).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["data"]["is_allowed"], false);
    assert_eq!(
        refresh_bundle(&fixture, &bundle, Uuid::now_v7()).await.0,
        StatusCode::FORBIDDEN
    );
}

async fn replace_refresh_cache(
    fixture: &Fixture,
    bundle: &serde_json::Value,
    key: Uuid,
    variation: &str,
) {
    let auth_id = bundle_id(bundle);
    let (_, request) = fixture.store.refresh(auth_id, key).await.unwrap().unwrap();
    let binding =
        ask_auth::crypto::Binding::refresh(auth_id, key, request.input_generation).unwrap();
    let plaintext = fixture
        .crypto
        .decrypt(&binding, request.encrypted_response.as_ref().unwrap())
        .unwrap();
    let mut cached: serde_json::Value = serde_json::from_slice(&plaintext).unwrap();
    match variation {
        "missing" => {
            cached.as_object_mut().unwrap().remove("refresh_expires_at");
        }
        "identity" => cached["identity"]["user_id"] = serde_json::json!("another-user"),
        "generation" => cached["credential_generation"] = serde_json::json!(3),
        "broker" => cached["broker_token"] = serde_json::json!(URL_SAFE_NO_PAD.encode([22; 32])),
        "rt" => cached["refresh_token"] = serde_json::json!("different-refresh"),
        "scope" => cached["scope"] = serde_json::json!(["other"]),
        "extra" => cached["unexpected"] = serde_json::json!(true),
        _ => {}
    }
    let envelope = fixture
        .crypto
        .encrypt(2, &binding, &serde_json::to_vec(&cached).unwrap())
        .unwrap();
    sqlx::query(
        "UPDATE auth_refresh_requests SET encrypted_response=? WHERE auth_id=? AND request_id=?",
    )
    .bind(envelope.to_vec())
    .bind(auth_id.as_bytes().as_slice())
    .bind(key.as_bytes().as_slice())
    .execute(&mut sql_connection().await)
    .await
    .unwrap();
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_refresh_cached_bundle_is_fully_validated_after_request_authentication() {
    for variation in [
        "missing",
        "identity",
        "generation",
        "broker",
        "rt",
        "scope",
        "extra",
    ] {
        let fixture = fixture("ok", true).await;
        let bundle = claimed_bundle(&fixture).await;
        let key = Uuid::now_v7();
        assert_eq!(
            refresh_bundle(&fixture, &bundle, key).await.0,
            StatusCode::OK
        );
        replace_refresh_cache(&fixture, &bundle, key, variation).await;
        let mut wrong = bundle.clone();
        wrong["refresh_token"] = serde_json::json!("wrong-proof");
        assert_eq!(
            refresh_bundle(&fixture, &wrong, key).await.0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            refresh_bundle(&fixture, &bundle, key).await.0,
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(fixture.provider.refresh_calls.load(Ordering::SeqCst), 1);
    }
}
fn rotate_service_config(fixture: &mut Fixture, version: u16) {
    let service = LoginService::new(
        fixture.store.clone(),
        fixture.crypto.clone(),
        enabled_config_version(version),
        LoginAdapters {
            provider: fixture.provider.clone(),
            admission: fixture.admission.clone(),
        },
    )
    .unwrap();
    fixture.router = build_enabled_router(Arc::new(service));
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_refresh_hmac_rotation_preserves_old_request_version_and_independent_aead() {
    let mut fixture = fixture("ok", true).await;
    let bundle = claimed_bundle(&fixture).await;
    let key = Uuid::now_v7();
    let (_, next) = refresh_bundle(&fixture, &bundle, key).await;
    replace_refresh_cache(&fixture, &bundle, key, "reencrypt_only").await;
    assert_eq!(
        refresh_bundle(&fixture, &bundle, key).await.0,
        StatusCode::OK
    );
    rotate_service_config(&mut fixture, 2);
    assert_eq!(
        refresh_bundle(&fixture, &next["data"], Uuid::now_v7())
            .await
            .0,
        StatusCode::OK
    );
    let (row, old_request) = fixture
        .store
        .refresh(bundle_id(&bundle), key)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.hmac_key_version, 2);
    assert_eq!(old_request.hmac_key_version, 1);
    assert_eq!(
        refresh_bundle(&fixture, &bundle, key).await.1["error"]["code"],
        "credential_generation_mismatch"
    );
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_refresh_recreated_authorization_id_rejects_original_worker_fence() {
    for outcome in ["ok", "unavailable", "unknown"] {
        let fixture = fixture("ok", true).await;
        let bundle = claimed_bundle(&fixture).await;
        let auth_id = bundle_id(&bundle);
        set_refresh_outcome(&fixture, outcome);
        fixture.provider.is_paused.store(true, Ordering::SeqCst);
        let operation = refresh_bundle(&fixture, &bundle, Uuid::now_v7());
        tokio::pin!(operation);
        tokio::select! { _ = fixture.provider.entered.notified() => {}, _ = &mut operation => panic!("must reserve") }
        rewrite_sql(auth_id, "DELETE FROM auth_refresh_requests WHERE auth_id=?").await;
        rewrite_sql(auth_id, "DELETE FROM auth_sessions WHERE auth_id=?").await;
        let recreated = create(&fixture, auth_id).await;
        callback(&fixture, &recreated).await;
        assert_eq!(
            claim(&fixture, auth_id, Uuid::now_v7()).await.0,
            StatusCode::OK
        );
        let before = fixture.store.session(auth_id).await.unwrap().unwrap();
        fixture.provider.release.notify_one();
        assert_ne!(operation.await.0, StatusCode::OK);
        assert!(fixture.store.session(auth_id).await.unwrap().unwrap() == before);
    }
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_logout_multiple_pending_children_detects_corruption_without_partial_revocation() {
    let fixture = fixture("ok", true).await;
    let bundle = claimed_bundle(&fixture).await;
    fixture.provider.is_paused.store(true, Ordering::SeqCst);
    {
        let operation = refresh_bundle(&fixture, &bundle, Uuid::now_v7());
        tokio::pin!(operation);
        tokio::select! { _ = fixture.provider.entered.notified() => {}, _ = &mut operation => panic!("must reserve") }
    }
    sqlx::query("INSERT INTO auth_refresh_requests SELECT auth_id,?,request_hash,hmac_key_version,input_generation,status,expires_at,encrypted_response,error_code FROM auth_refresh_requests WHERE auth_id=? LIMIT 1")
        .bind(Uuid::now_v7().as_bytes().as_slice()).bind(bundle_id(&bundle).as_bytes().as_slice())
        .execute(&mut sql_connection().await).await.unwrap();
    assert_eq!(
        logout_bundle(&fixture, &bundle).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(session_for(&fixture, &bundle).await.status, "refreshing");
}

#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_no_consumption_after_natural_expiry_ends_authorization() {
    let fixture = fixture("ok", true).await;
    let bundle = claimed_bundle(&fixture).await;
    rewrite_sql(bundle_id(&bundle),"UPDATE auth_sessions SET auth_expires_at=UTC_TIMESTAMP(3)+INTERVAL 1 SECOND WHERE auth_id=?").await;
    set_refresh_outcome(&fixture, "expiry_failure");
    let key = Uuid::now_v7();
    assert_eq!(
        refresh_bundle(&fixture, &bundle, key).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let row = session_for(&fixture, &bundle).await;
    assert_eq!(row.status, "expired");
    assert_eq!(row.reason_code.as_deref(), Some("authorization_expired"));
    assert!(row.encrypted_result.is_none() && row.login_expires_at.is_none());
    assert_eq!(
        refresh_bundle(&fixture, &bundle, key).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(fixture.provider.refresh_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_refresh_execution_lease_starts_at_committed_reservation_clock() {
    let fixture = fixture("ok", true).await;
    let bundle = claimed_bundle(&fixture).await;
    fixture.admission.is_paused.store(true, Ordering::SeqCst);
    fixture.provider.is_paused.store(true, Ordering::SeqCst);
    let key = Uuid::now_v7();
    let operation = refresh_bundle(&fixture, &bundle, key);
    tokio::pin!(operation);
    tokio::select! { _ = fixture.admission.entered.notified() => {}, _ = &mut operation => panic!("must pause policy") }
    tokio::time::sleep(Wait::from_millis(1200)).await;
    fixture.admission.release.notify_one();
    tokio::select! { _ = fixture.provider.entered.notified() => {}, _ = &mut operation => panic!("must reserve") }
    let (_, request) = fixture
        .store
        .refresh(bundle_id(&bundle), key)
        .await
        .unwrap()
        .unwrap();
    let now = sqlx::query_scalar::<_, chrono::NaiveDateTime>("SELECT UTC_TIMESTAMP(3)")
        .fetch_one(&mut sql_connection().await)
        .await
        .unwrap();
    assert!(request.expires_at - now > Duration::milliseconds(44500));
    assert!(request.expires_at - now <= Duration::seconds(45));
    fixture.provider.release.notify_one();
    assert_eq!(operation.await.0, StatusCode::OK);
}

struct ExtremeTimestampAdmission;
impl AdmissionProvider for ExtremeTimestampAdmission {
    fn snapshot(&self) -> AdmissionFuture<'_> {
        Box::pin(async {
            let fetched_at = chrono::DateTime::from_timestamp_millis(
                chrono::NaiveDateTime::MAX.and_utc().timestamp_millis(),
            )
            .unwrap()
            .naive_utc();
            let mut policy = AdmissionSnapshot::deny_all(fetched_at);
            policy.is_enabled = true;
            policy.version = "extreme-timestamp-policy".into();
            Ok(policy)
        })
    }
}
#[tokio::test]
#[ignore = "requires isolated real MySQL runtime and migration accounts"]
async fn mysql_status_extreme_policy_timestamp_returns_unavailable_without_panicking() {
    let mut fixture = fixture("ok", true).await;
    let bundle = claimed_bundle(&fixture).await;
    let service = LoginService::new(
        fixture.store.clone(),
        fixture.crypto.clone(),
        enabled_config(),
        LoginAdapters {
            provider: fixture.provider.clone(),
            admission: Arc::new(ExtremeTimestampAdmission),
        },
    )
    .unwrap();
    fixture.router = build_enabled_router(Arc::new(service));
    let (status, response) = online(&fixture, &bundle).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response["error"]["code"], "upstream_unavailable");
    validate_contracts(&[(&response, "ErrorResponse")]);
}
