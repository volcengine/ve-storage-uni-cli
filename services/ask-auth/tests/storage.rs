//! Storage security and phase invariants; these do not enable HTTP lifecycle.
use ask_auth::crypto::{Binding, Crypto, KeyMaterial, Purpose};
use uuid::Uuid;

#[test]
fn encrypted_results_are_bound_to_original_resource_and_generation() {
    let crypto = Crypto::new(
        "deployment-a",
        "app-a",
        vec![KeyMaterial::new(1, [3; 32], [7; 32]).unwrap()],
    )
    .unwrap();
    let auth_id = Uuid::now_v7();
    let request_id = Uuid::now_v7();
    let binding = Binding::refresh(auth_id, request_id, 1).unwrap();
    let encrypted = crypto.encrypt(1, &binding, b"token-bundle").unwrap();
    assert_eq!(
        &*crypto.decrypt(&binding, &encrypted).unwrap(),
        b"token-bundle"
    );
    assert!(crypto
        .decrypt(
            &Binding::refresh(auth_id, request_id, 2).unwrap(),
            &encrypted
        )
        .is_err());
    assert!(crypto
        .decrypt(
            &Binding::refresh(auth_id, Uuid::now_v7(), 1).unwrap(),
            &encrypted
        )
        .is_err());
    assert!(crypto
        .decrypt(&Binding::login(auth_id), &encrypted)
        .is_err());
    let digest = crypto
        .digest(1, Purpose::Broker, auth_id, &[b"broker"])
        .unwrap();
    assert!(crypto
        .verify(1, Purpose::Broker, auth_id, &[b"broker"], &digest)
        .unwrap());
    assert!(!crypto
        .verify(1, Purpose::RefreshToken, auth_id, &[b"broker"], &digest)
        .unwrap());
}

#[test]
fn pending_shape_and_complete_group_clear_are_enforced() {
    let mut row = pending();
    assert!(row.validate("app-a").is_ok());
    row.broker_hash = Some([1; 32]);
    assert!(row.validate("app-a").is_err());
    row.broker_hash = None;
    row.oauth_state_hash = None;
    assert!(row.validate("app-a").is_err());
    assert!(pending().validate("other-app").is_err());
}

fn pending() -> ask_auth::storage::Session {
    use ask_auth::storage::Session;
    Session {
        auth_id: Uuid::now_v7(),
        app_id: "app-a".into(),
        status: "pending".into(),
        request_hash: Some([1; 32]),
        login_ticket_hash: Some([2; 32]),
        claim_challenge: Some([3; 32]),
        oauth_state_hash: Some([4; 32]),
        hmac_key_version: 1,
        encrypted_oauth_context: Some(vec![1; 32]),
        claim_request_id: None,
        encrypted_result: None,
        login_expires_at: Some(chrono::Utc::now().naive_utc()),
        user_id: None,
        tenant_id: None,
        scope_json: None,
        broker_hash: None,
        refresh_token_hash: None,
        credential_generation: None,
        auth_expires_at: None,
        reason_code: None,
    }
}

#[test]
fn original_login_lease_payload_and_cleared_group_cannot_be_replaced() {
    let original = pending();
    let mut next = original.clone();
    next.status = "exchanging".into();
    assert!(original.validate_transition(&next, "app-a").is_ok());
    next.login_expires_at = next
        .login_expires_at
        .map(|deadline| deadline + chrono::Duration::seconds(1));
    assert!(original.validate_transition(&next, "app-a").is_err());
    next = original.clone();
    next.encrypted_oauth_context = Some(vec![9; 32]);
    assert!(original.validate_transition(&next, "app-a").is_err());
}

#[test]
fn tls_is_not_optional_for_remote_hosts() {
    use ask_auth::storage::*;
    assert!(DatabaseConfig::new(
        "mysql://service:secret@192.0.2.1/test".into(),
        Tls::LoopbackTlsTestOnly,
        2
    )
    .is_err());
    assert!(DatabaseConfig::new(
        "mysql://service:secret@127.0.0.1/test?socket=/tmp/mysql.sock".into(),
        Tls::LoopbackTlsTestOnly,
        2
    )
    .is_err());
}

fn ring(deployment: &str, app: &str) -> Crypto {
    Crypto::new(
        deployment,
        app,
        vec![
            KeyMaterial::new(1, [3; 32], [7; 32]).unwrap(),
            KeyMaterial::new(2, [4; 32], [8; 32]).unwrap(),
        ],
    )
    .unwrap()
}
#[test]
fn crypto_rejects_tampering_cross_scope_and_unknown_versions() {
    let crypto = ring("deployment-a", "app-a");
    let id = Uuid::now_v7();
    let binding = Binding::context(id);
    let encrypted = crypto.encrypt(1, &binding, b"context").unwrap();
    assert_ne!(
        encrypted.to_vec(),
        crypto.encrypt(1, &binding, b"context").unwrap().to_vec()
    );
    for position in [0, 1, 2, 3, 15, encrypted.len() - 1] {
        let mut corrupted = encrypted.to_vec();
        corrupted[position] ^= 1;
        assert!(crypto.decrypt(&binding, &corrupted).is_err());
    }
    assert!(crypto
        .decrypt(&Binding::context(Uuid::now_v7()), &encrypted)
        .is_err());
    assert!(ring("deployment-b", "app-a")
        .decrypt(&binding, &encrypted)
        .is_err());
    assert!(ring("deployment-a", "app-b")
        .decrypt(&binding, &encrypted)
        .is_err());
    let rotated = crypto.encrypt(2, &binding, b"new-context").unwrap();
    assert_eq!(
        &*crypto.decrypt(&binding, &rotated).unwrap(),
        b"new-context"
    );
    assert!(crypto.encrypt(3, &binding, b"unknown").is_err());
    assert!(crypto.digest(3, Purpose::Broker, id, &[b"proof"]).is_err());
    assert!(Binding::refresh(id, Uuid::now_v7(), u64::MAX).is_err());
}
#[test]
fn canonical_hmac_and_envelope_bounds_are_unambiguous() {
    let crypto = ring("deployment-a", "app-a");
    let id = Uuid::now_v7();
    assert_ne!(
        *crypto
            .digest(1, Purpose::LoginRequest, id, &[b"ab", b"c"])
            .unwrap(),
        *crypto
            .digest(1, Purpose::LoginRequest, id, &[b"a", b"bc"])
            .unwrap()
    );
    let binding = Binding::context(id);
    for size in [1, 32768 - 31] {
        let plaintext = vec![42; size];
        let envelope = crypto.encrypt(1, &binding, &plaintext).unwrap();
        assert_eq!(&*crypto.decrypt(&binding, &envelope).unwrap(), plaintext);
    }
    assert!(crypto.encrypt(1, &binding, &[]).is_err());
    assert!(crypto.encrypt(1, &binding, &vec![0; 32768 - 30]).is_err());
    assert!(crypto.decrypt(&binding, &vec![0; 32769]).is_err());
    assert!(crypto.decrypt(&binding, &[1; 31]).is_err());
    assert!(KeyMaterial::new(0, [1; 32], [2; 32]).is_err());
    assert!(KeyMaterial::new(1, [1; 32], [1; 32]).is_err());
}
#[test]
fn phase_matrix_rejects_partial_authorization_unknown_state_scope_and_generation() {
    let original = pending();
    for status in [
        "claimed",
        "active",
        "refreshing",
        "ready",
        "revoked",
        "reauth_required",
        "",
    ] {
        let mut malformed = original.clone();
        malformed.status = status.into();
        assert!(malformed.validate("app-a").is_err());
    }
    for status in ["denied", "error", "expired"] {
        let mut terminal = original.clone();
        terminal.status = status.into();
        terminal.encrypted_oauth_context = None;
        terminal.reason_code = Some("login_expired".into());
        assert!(terminal.validate("app-a").is_ok());
        terminal.oauth_state_hash = None;
        assert!(terminal.validate("app-a").is_err());
    }
    let mut authorized = original.clone();
    authorized.status = "ready".into();
    authorized.encrypted_oauth_context = None;
    authorized.encrypted_result = Some(vec![1; 32]);
    authorized.user_id = Some("user".into());
    authorized.scope_json = Some(vec!["read".into()]);
    authorized.broker_hash = Some([1; 32]);
    authorized.refresh_token_hash = Some([2; 32]);
    authorized.credential_generation = Some(1);
    authorized.auth_expires_at = authorized.login_expires_at;
    assert!(authorized.validate("app-a").is_ok());
    authorized.scope_json = Some(vec!["read".into(), "read".into()]);
    assert!(authorized.validate("app-a").is_err());
    authorized.scope_json = Some(vec!["z".into(), "a".into()]);
    assert!(authorized.validate("app-a").is_err());
    authorized.scope_json = Some(vec!["read".into()]);
    authorized.credential_generation = Some(0);
    assert!(authorized.validate("app-a").is_err());
    authorized.credential_generation = Some(1);
    authorized.reason_code = Some("upstream response secret".into());
    assert!(authorized.validate("app-a").is_err());
}
#[test]
fn refresh_shapes_and_overflow_fail_closed() {
    use ask_auth::storage::RefreshRequest;
    let mut request = RefreshRequest {
        auth_id: Uuid::now_v7(),
        request_id: Uuid::now_v7(),
        request_hash: [1; 32],
        hmac_key_version: 1,
        input_generation: 1,
        status: "pending".into(),
        expires_at: chrono::Utc::now().naive_utc(),
        encrypted_response: None,
        error_code: None,
    };
    assert!(request.validate().is_ok());
    let request_id = request.request_id;
    request.request_id = legacy_id();
    assert!(request.validate().is_err());
    request.request_id = request_id;
    request.encrypted_response = Some(vec![1; 32]);
    assert!(request.validate().is_err());
    request.status = "completed".into();
    assert!(request.validate().is_ok());
    request.error_code = Some("refresh_failed".into());
    assert!(request.validate().is_err());
    request.encrypted_response = None;
    for status in ["failed", "unknown"] {
        request.status = status.into();
        assert!(request.validate().is_ok());
    }
    request.input_generation = u64::MAX;
    assert!(request.validate().is_err());
    request.input_generation = 1;
    request.hmac_key_version = 0;
    assert!(request.validate().is_err());
}

#[test]
fn callback_must_reserve_exchanging_before_publishing_denial() {
    let original = pending();
    let mut denied = original.clone();
    denied.status = "denied".into();
    denied.encrypted_oauth_context = None;
    denied.reason_code = Some("access_denied".into());
    assert!(original.validate_transition(&denied, "app-a").is_err());
}

#[test]
fn auth_and_operation_ids_require_v7() {
    let auth_id = Uuid::parse_str("11111111-1111-7111-8111-111111111111").unwrap();
    let crypto = ring("deployment-a", "app-a");
    assert!(crypto
        .encrypt(1, &Binding::context(auth_id), b"context")
        .is_ok());
    assert!(crypto
        .encrypt(1, &Binding::context(legacy_id()), b"context")
        .is_err());
    assert!(Binding::refresh(auth_id, Uuid::now_v7(), 1).is_ok());
    assert!(Binding::refresh(auth_id, legacy_id(), 1).is_err());
    // [Review Fix #21] Version bits alone cannot admit a non-RFC operation UUID.
    let invalid_variant = Uuid::parse_str("11111111-1111-7111-7111-111111111111").unwrap();
    assert!(Binding::refresh(auth_id, invalid_variant, 1).is_err());
    let mut row = pending();
    row.auth_id = auth_id;
    assert!(row.validate("app-a").is_ok());
    row.auth_id = legacy_id();
    assert!(row.validate("app-a").is_err());
}

#[test]
fn hmac_scope_key_and_length_limits_reject_cross_binding() {
    let crypto = ring("deployment-a", "app-a");
    let auth_id = Uuid::now_v7();
    let digest = crypto
        .digest(1, Purpose::Broker, auth_id, &[b"proof"])
        .unwrap();
    for other in [ring("deployment-b", "app-a"), ring("deployment-a", "app-b")] {
        assert!(!other
            .verify(1, Purpose::Broker, auth_id, &[b"proof"], &digest)
            .unwrap());
    }
    assert!(!crypto
        .verify(2, Purpose::Broker, auth_id, &[b"proof"], &digest)
        .unwrap());
    assert!(!crypto
        .verify(1, Purpose::Broker, Uuid::now_v7(), &[b"proof"], &digest)
        .unwrap());
    assert!(crypto
        .verify(3, Purpose::Broker, auth_id, &[b"proof"], &digest)
        .is_err());
    assert!(crypto
        .digest(1, Purpose::Broker, auth_id, &[&vec![0; 262145]])
        .is_err());
    assert!(crypto
        .digest(1, Purpose::Broker, auth_id, &vec![b"".as_slice(); 101])
        .is_err());
    let invalid_variant = Uuid::parse_str("11111111-1111-7111-7111-111111111111").unwrap();
    assert!(crypto
        .encrypt(1, &Binding::login(invalid_variant), b"secret")
        .is_err());
    assert!(crypto
        .digest(1, Purpose::Broker, invalid_variant, &[b"proof"])
        .is_err());
    assert!(Crypto::new(
        "",
        "app-a",
        vec![KeyMaterial::new(1, [3; 32], [4; 32]).unwrap()]
    )
    .is_err());
    assert!(Crypto::new("deployment-a", "app-a", vec![]).is_err());
}

fn authorized() -> ask_auth::storage::Session {
    let mut row = pending();
    row.status = "ready".into();
    row.encrypted_oauth_context = None;
    row.encrypted_result = Some(vec![1; 32]);
    row.user_id = Some("user".into());
    row.scope_json = Some(vec!["read".into()]);
    row.broker_hash = Some([1; 32]);
    row.refresh_token_hash = Some([2; 32]);
    row.credential_generation = Some(1);
    row.auth_expires_at = row.login_expires_at;
    row
}

#[test]
fn every_authorized_phase_has_exact_shape_and_cleared_login_is_irreversible() {
    let ready = authorized();
    assert!(ready.validate("app-a").is_ok());
    let mut active = ready.clone();
    active.status = "active".into();
    active.claim_request_id = Some(Uuid::now_v7());
    assert!(ready.validate_transition(&active, "app-a").is_ok());
    let claim_id = active.claim_request_id;
    active.claim_request_id = Some(legacy_id());
    assert!(active.validate("app-a").is_err());
    active.claim_request_id = claim_id;
    for status in ["active", "refreshing"] {
        let mut row = active.clone();
        row.status = status.into();
        assert!(row.validate("app-a").is_ok());
        row.clear_login();
        assert!(row.validate("app-a").is_ok());
        assert!(row.validate_transition(&active, "app-a").is_err());
    }
    for status in ["expired", "revoked", "reauth_required"] {
        let mut ended = active.clone();
        ended.status = status.into();
        ended.clear_login();
        ended.reason_code = Some("revoked".into());
        assert!(active.validate_transition(&ended, "app-a").is_ok());
        assert!(ended.validate_transition(&active, "app-a").is_err());
    }
}

#[test]
fn immutable_authorization_identity_and_scope_cannot_be_replaced() {
    let mut current = authorized();
    current.status = "refreshing".into();
    current.claim_request_id = Some(Uuid::now_v7());
    let mut next = current.clone();
    next.status = "active".into();
    next.clear_login();
    next.credential_generation = Some(2);
    assert!(current.validate_transition(&next, "app-a").is_ok());
    next.scope_json = Some(vec!["read".into(), "write".into()]);
    assert!(current.validate_transition(&next, "app-a").is_err());
    next.scope_json = Some(vec!["read".into()]);
    next.user_id = Some("foreign-user".into());
    assert!(current.validate_transition(&next, "app-a").is_err());
    next.user_id = current.user_id.clone();
    next.app_id = "foreign-app".into();
    assert!(current.validate_transition(&next, "app-a").is_err());
}

#[test]
fn claim_replay_deadline_cannot_outlive_authorization() {
    let original = authorized();
    let mut next = original.clone();
    next.status = "active".into();
    next.claim_request_id = Some(Uuid::now_v7());
    next.login_expires_at = next
        .auth_expires_at
        .map(|deadline| deadline + chrono::Duration::seconds(1));
    assert!(original.validate_transition(&next, "app-a").is_err());
}

#[test]
fn oauth_state_hmac_matches_independent_canonical_vector() {
    // Independently generated with Python hmac/sha256 over five identity fields and full state.
    let auth_id = Uuid::parse_str("11111111-1111-7111-8111-111111111111").unwrap();
    let digest = ring("deployment-a", "app-a")
        .digest(1, Purpose::OAuthState, auth_id, &[b"full-oauth-state"])
        .unwrap();
    let expected = [
        0x02, 0x1a, 0xde, 0x2f, 0x58, 0xda, 0x8c, 0x6b, 0xe8, 0x27, 0xe4, 0xc7, 0xf3, 0xec, 0x6f,
        0x2d, 0x53, 0x7d, 0xda, 0x1e, 0x23, 0x4b, 0x55, 0x45, 0xbd, 0x9b, 0xe3, 0x3e, 0xd2, 0x5b,
        0xa5, 0x3f,
    ];
    assert_eq!(*digest, expected);
}

#[test]
fn expanded_aes_round_keys_are_zeroized_on_drop() {
    fn requires_zeroize_on_drop<T: zeroize::ZeroizeOnDrop>() {}
    requires_zeroize_on_drop::<aes_gcm::aes::Aes256>();
}

#[test]
fn authentication_hash_backend_drops_its_secret_state() {
    assert!(
        std::mem::needs_drop::<ghash::GHash>(),
        "GHASH must drop its secret state"
    );
    assert!(
        std::mem::needs_drop::<polyval::Polyval>(),
        "POLYVAL must drop its secret state"
    );
}

fn legacy_id() -> Uuid {
    Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap()
}
