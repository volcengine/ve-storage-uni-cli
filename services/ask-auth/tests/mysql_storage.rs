//! Requires two explicit accounts in one isolated disposable MySQL database.
use ask_auth::crypto::{Binding, Crypto, KeyMaterial};
use ask_auth::storage::*;
use chrono::{Duration, Utc};
use std::sync::Arc;
use uuid::Uuid;

async fn fixture() -> Store {
    let migration_url =
        std::env::var("ASK_AUTH_TEST_MIGRATION_URL").expect("dedicated migration URL is required");
    let runtime_url =
        std::env::var("ASK_AUTH_TEST_DATABASE_URL").expect("dedicated runtime URL is required");
    let migration = MigrationDatabase::connect(
        DatabaseConfig::new(migration_url, Tls::LoopbackTlsTestOnly, 1).unwrap(),
    )
    .await
    .unwrap();
    migration.migrate().await.unwrap();
    migration.migrate().await.unwrap();
    migration.close().await;
    let database = RuntimeDatabase::connect(
        DatabaseConfig::new(runtime_url, Tls::LoopbackTlsTestOnly, 4).unwrap(),
    )
    .await
    .unwrap();
    Store::new(database, "app-a", crypto()).unwrap()
}
fn crypto() -> Arc<Crypto> {
    Arc::new(
        Crypto::new(
            "deployment-a",
            "app-a",
            vec![
                KeyMaterial::new(1, [3; 32], [7; 32]).unwrap(),
                KeyMaterial::new(2, [4; 32], [8; 32]).unwrap(),
            ],
        )
        .unwrap(),
    )
}

#[tokio::test]
#[ignore = "requires explicit isolated MySQL URLs"]
async fn runtime_pool_close_is_idempotent_and_store_operations_fail_closed() {
    let store = fixture().await;
    assert!(store.session(Uuid::now_v7()).await.is_ok());
    tokio::join!(store.close(), store.close());
    assert_eq!(
        store.session(Uuid::now_v7()).await.err(),
        Some(StorageError::Unavailable)
    );
}
fn now() -> chrono::NaiveDateTime {
    chrono::DateTime::from_timestamp_millis(Utc::now().timestamp_millis())
        .unwrap()
        .naive_utc()
}
fn pending() -> Session {
    let auth_id = Uuid::now_v7();
    Session {
        auth_id,
        app_id: "app-a".into(),
        status: "pending".into(),
        request_hash: Some([1; 32]),
        login_ticket_hash: Some([2; 32]),
        claim_challenge: Some([3; 32]),
        oauth_state_hash: Some([4; 32]),
        hmac_key_version: 1,
        encrypted_oauth_context: Some(
            crypto()
                .encrypt(1, &Binding::context(auth_id), b"context")
                .unwrap()
                .to_vec(),
        ),
        claim_request_id: None,
        encrypted_result: None,
        login_expires_at: Some(now() + Duration::minutes(9)),
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
#[tokio::test]
#[ignore = "requires explicit isolated MySQL runtime and migration URLs"]
async fn mysql_create_validates_and_concurrent_callback_cas_has_one_winner() {
    let store = fixture().await;
    let original = store.create_pending(pending()).await.unwrap();
    assert_eq!(
        store.create_pending(original.clone()).await.err(),
        Some(StorageError::Conflict)
    );
    let loaded = store.session(original.auth_id).await.unwrap().unwrap();
    assert!(loaded == original);
    let mut next = original.clone();
    next.status = "exchanging".into();
    let fence = CallbackFence::capture(&original).unwrap();
    let first = store.apply(Mutation::Callback {
        expected: original.clone(),
        next: next.clone(),
        fence: fence.clone(),
    });
    let second = store.apply(Mutation::Callback {
        expected: original.clone(),
        next,
        fence,
    });
    let results = tokio::join!(first, second);
    assert_eq!(
        usize::from(results.0.is_ok()) + usize::from(results.1.is_ok()),
        1
    );
}

async fn active(store: &Store) -> Session {
    let original = store.create_pending(pending()).await.unwrap();
    let fence = CallbackFence::capture(&original).unwrap();
    let mut exchanging = original.clone();
    exchanging.status = "exchanging".into();
    store
        .apply(Mutation::Callback {
            expected: original,
            next: exchanging.clone(),
            fence: fence.clone(),
        })
        .await
        .unwrap();
    let mut ready = exchanging.clone();
    ready.status = "ready".into();
    ready.encrypted_oauth_context = None;
    ready.encrypted_result = Some(
        crypto()
            .encrypt(1, &Binding::login(ready.auth_id), b"login-result")
            .unwrap()
            .to_vec(),
    );
    ready.user_id = Some("trusted-user".into());
    ready.scope_json = Some(vec!["read".into()]);
    ready.broker_hash = Some([8; 32]);
    ready.refresh_token_hash = Some([9; 32]);
    ready.credential_generation = Some(1);
    ready.auth_expires_at = Some(now() + Duration::days(1));
    store
        .apply(Mutation::Callback {
            expected: exchanging,
            next: ready.clone(),
            fence,
        })
        .await
        .unwrap();
    let mut active = ready.clone();
    active.status = "active".into();
    active.claim_request_id = Some(Uuid::now_v7());
    active.login_expires_at = Some(now() + Duration::minutes(4));
    store
        .apply(Mutation::Claim {
            expected: ready,
            next: active.clone(),
        })
        .await
        .unwrap();
    active
}
fn request(session: &Session) -> RefreshRequest {
    RefreshRequest {
        auth_id: session.auth_id,
        request_id: Uuid::now_v7(),
        request_hash: [10; 32],
        hmac_key_version: 1,
        input_generation: session.credential_generation.unwrap(),
        status: "pending".into(),
        expires_at: now() + Duration::seconds(40),
        encrypted_response: None,
        error_code: None,
    }
}
#[tokio::test]
#[ignore = "requires explicit isolated MySQL runtime and migration URLs"]
async fn mysql_refresh_is_atomic_rotates_only_a_and_rejects_late_result() {
    let store = fixture().await;
    let (refreshing, request) = starting_refresh(&store).await;
    let (next, completed) = completion(&refreshing, &request);
    store
        .apply(finishing(
            &refreshing,
            &request,
            next.clone(),
            completed.clone(),
        ))
        .await
        .unwrap();
    let (stored, ledger) = store
        .refresh(next.auth_id, request.request_id)
        .await
        .unwrap()
        .unwrap();
    assert!(stored == next);
    assert!(ledger == completed);
    assert_eq!(ledger.hmac_key_version, 1);
    assert!(store
        .apply(finishing(&refreshing, &request, next, completed))
        .await
        .is_err());
}
#[tokio::test]
#[ignore = "requires explicit isolated MySQL runtime and migration URLs"]
async fn mysql_revoke_blocks_late_result_and_parent_delete_until_child_settled() {
    let store = fixture().await;
    let (refreshing, request) = starting_refresh(&store).await;
    let revoked = revoke(&store, &refreshing).await;
    let (next, completed) = completion(&refreshing, &request);
    assert!(store
        .apply(finishing(&refreshing, &request, next, completed))
        .await
        .is_err());
    assert!(store
        .apply(Mutation::DeleteSession {
            expected: revoked.clone()
        })
        .await
        .is_err());
    settle_and_delete(&store, &revoked, request).await;
    assert!(store.session(revoked.auth_id).await.unwrap().is_none());
}

#[tokio::test]
#[ignore = "requires explicit isolated MySQL runtime and migration URLs"]
async fn mysql_failed_refresh_cannot_advance_generation_or_rotate_keys() {
    let store = fixture().await;
    let original = active(&store).await;
    let request = request(&original);
    store
        .apply(Mutation::StartRefresh {
            expected: original,
            request: request.clone(),
        })
        .await
        .unwrap();
    let expected = store.session(request.auth_id).await.unwrap().unwrap();
    let fence = RefreshFence::capture(&expected, &request).unwrap();
    let mut next = expected.clone();
    next.status = "active".into();
    next.clear_login();
    next.credential_generation = Some(2);
    next.hmac_key_version = 2;
    next.broker_hash = Some([11; 32]);
    next.refresh_token_hash = Some([12; 32]);
    let mut failed = request.clone();
    failed.status = "failed".into();
    failed.error_code = Some("refresh_failed".into());
    failed.expires_at = now() + Duration::minutes(4);
    assert!(store
        .apply(Mutation::FinishRefresh {
            expected: expected.clone(),
            next: next.into(),
            request: request.clone(),
            next_request: failed,
            fence
        })
        .await
        .is_err());
    assert!(store.session(expected.auth_id).await.unwrap().unwrap() == expected);
    assert_eq!(
        store
            .refresh(expected.auth_id, request.request_id)
            .await
            .unwrap()
            .unwrap()
            .1
            .status,
        "pending"
    );
}

async fn admin() -> sqlx::MySqlConnection {
    use sqlx::{ConnectOptions, Connection};
    let url =
        std::env::var("ASK_AUTH_TEST_MIGRATION_URL").expect("dedicated migration URL is required");
    let options = url
        .parse::<sqlx::mysql::MySqlConnectOptions>()
        .map_err(|_| StorageError::Invalid)
        .unwrap()
        .ssl_mode(sqlx::mysql::MySqlSslMode::Required)
        .disable_statement_logging();
    let mut connection = sqlx::MySqlConnection::connect_with(&options)
        .await
        .map_err(StorageError::from)
        .unwrap();
    let status: (String, String) = sqlx::query_as("SHOW SESSION STATUS LIKE 'Ssl_cipher'")
        .fetch_one(&mut connection)
        .await
        .map_err(StorageError::from)
        .unwrap();
    assert!(
        !status.1.is_empty(),
        "isolated test connection must negotiate TLS"
    );
    connection
}
async fn migrator() -> MigrationDatabase {
    let url =
        std::env::var("ASK_AUTH_TEST_MIGRATION_URL").expect("dedicated migration URL is required");
    MigrationDatabase::connect(DatabaseConfig::new(url, Tls::LoopbackTlsTestOnly, 1).unwrap())
        .await
        .unwrap()
}
#[tokio::test]
#[ignore = "requires explicit isolated MySQL runtime and migration URLs"]
async fn mysql_migration_checksum_dirty_and_cancelled_lock_are_fail_closed() {
    let _store = fixture().await;
    let mut connection = admin().await;
    let migration = migrator().await;
    let checksum: Vec<u8> =
        sqlx::query_scalar("SELECT checksum FROM _sqlx_migrations WHERE version=1")
            .fetch_one(&mut connection)
            .await
            .map_err(StorageError::from)
            .unwrap();
    sqlx::query("UPDATE _sqlx_migrations SET checksum=REPEAT(0x00,48) WHERE version=1")
        .execute(&mut connection)
        .await
        .map_err(StorageError::from)
        .unwrap();
    let mismatch = migration.migrate().await;
    sqlx::query("UPDATE _sqlx_migrations SET checksum=?,success=FALSE WHERE version=1")
        .bind(checksum)
        .execute(&mut connection)
        .await
        .map_err(StorageError::from)
        .unwrap();
    let dirty = migration.migrate().await;
    sqlx::query("UPDATE _sqlx_migrations SET success=TRUE WHERE version=1")
        .execute(&mut connection)
        .await
        .map_err(StorageError::from)
        .unwrap();
    assert_eq!(mismatch.err(), Some(StorageError::Migration));
    assert_eq!(dirty.err(), Some(StorageError::Migration));
    migration.migrate().await.unwrap();
    assert_cancelled_migration_releases_lock(&migration, &mut connection).await;
    migration.close().await;
}
#[tokio::test]
#[ignore = "requires explicit isolated MySQL runtime and migration URLs"]
async fn mysql_schema_has_exact_business_shape_and_primary_key_plans() {
    use sqlx::{Column, Row};
    let store = fixture().await;
    let original = active(&store).await;
    let refresh = request(&original);
    store
        .apply(Mutation::StartRefresh {
            expected: original.clone(),
            request: refresh.clone(),
        })
        .await
        .unwrap();
    let mut connection = admin().await;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM information_schema.columns WHERE table_schema=DATABASE() AND table_name IN ('auth_sessions','auth_refresh_requests')").fetch_one(&mut connection).await.map_err(StorageError::from).unwrap();
    assert_eq!(count, 29);
    for (table, expected) in [("auth_sessions", 20_i64), ("auth_refresh_requests", 9_i64)] {
        let columns: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM information_schema.columns WHERE table_schema=DATABASE() AND table_name=?")
            .bind(table).fetch_one(&mut connection).await.map_err(StorageError::from).unwrap();
        assert_eq!(columns, expected);
    }
    let keys: Vec<(String,i64)> = sqlx::query_as("SELECT index_name, COUNT(DISTINCT table_name) FROM information_schema.statistics WHERE table_schema=DATABASE() AND table_name IN ('auth_sessions','auth_refresh_requests') GROUP BY index_name").fetch_all(&mut connection).await.map_err(StorageError::from).unwrap();
    assert_eq!(keys.len(), 4);
    assert_eq!(keys.iter().find(|entry| entry.0 == "PRIMARY").unwrap().1, 2);
    let constraints: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM information_schema.table_constraints WHERE constraint_schema=DATABASE() AND table_name IN ('auth_sessions','auth_refresh_requests') AND constraint_type IN ('UNIQUE','CHECK','FOREIGN KEY')").fetch_one(&mut connection).await.map_err(StorageError::from).unwrap();
    assert_eq!(constraints, 0);
    for query in [
        "EXPLAIN FORMAT=TRADITIONAL SELECT * FROM auth_sessions WHERE auth_id=?",
        "EXPLAIN FORMAT=TRADITIONAL SELECT * FROM auth_refresh_requests WHERE auth_id=? AND request_id=?",
    ] {
        let mut statement = sqlx::query(query).bind(original.auth_id.as_bytes().to_vec());
        if query.contains("request_id") {
            statement = statement.bind(refresh.request_id.as_bytes().to_vec());
        }
        let row = statement
            .fetch_one(&mut connection)
            .await
            .map_err(StorageError::from)
            .unwrap();
        // EXPLAIN metadata can differ from the prepared SELECT name map; resolve its returned ordinal.
        let column = row.columns().iter().position(|column| column.name() == "key").unwrap();
        let key: Option<String> = row.try_get_unchecked(column).map_err(StorageError::from).unwrap();
        assert_eq!(key.as_deref(), Some("PRIMARY"));
    }
}

#[tokio::test]
#[ignore = "requires explicit isolated MySQL URLs; migration test account needs TRIGGER"]
async fn mysql_insert_failure_rolls_back_parent_and_never_creates_an_orphan() {
    let store = fixture().await;
    let original = active(&store).await;
    let request = request(&original);
    let mut connection = admin().await;
    sqlx::raw_sql("DROP TRIGGER IF EXISTS ask_auth_test_insert_failure")
        .execute(&mut connection)
        .await
        .map_err(StorageError::from)
        .unwrap();
    let trigger = format!("CREATE TRIGGER ask_auth_test_insert_failure BEFORE INSERT ON auth_refresh_requests FOR EACH ROW BEGIN IF NEW.auth_id=UNHEX('{}') THEN SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT='injected_storage_failure'; END IF; END",original.auth_id.simple());
    // Only a typed UUID rendered as fixed hexadecimal is interpolated; no user SQL is accepted.
    sqlx::raw_sql(sqlx::AssertSqlSafe(trigger))
        .execute(&mut connection)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "test trigger DB code: {:?}",
                error.as_database_error().map(|error| error
                    .downcast_ref::<sqlx::mysql::MySqlDatabaseError>()
                    .number())
            )
        });
    let outcome = store
        .apply(Mutation::StartRefresh {
            expected: original.clone(),
            request: request.clone(),
        })
        .await;
    let cleanup = sqlx::raw_sql("DROP TRIGGER IF EXISTS ask_auth_test_insert_failure")
        .execute(&mut connection)
        .await
        .map_err(StorageError::from);
    cleanup.unwrap();
    assert!(outcome.is_err());
    assert!(store.session(original.auth_id).await.unwrap().unwrap() == original);
    assert!(store
        .refresh(original.auth_id, request.request_id)
        .await
        .unwrap()
        .is_none());
}
#[tokio::test]
#[ignore = "requires explicit isolated MySQL runtime and migration URLs"]
async fn mysql_orphan_and_bad_ciphertext_fail_closed_without_adoption_or_repair() {
    let store = fixture().await;
    let mut original = pending();
    let mut connection = admin().await;
    let request_id = Uuid::now_v7();
    sqlx::query("INSERT INTO auth_refresh_requests (auth_id,request_id,request_hash,hmac_key_version,input_generation,status,expires_at) VALUES (?,?,?,1,1,'pending',UTC_TIMESTAMP(3))")
        .bind(original.auth_id.as_bytes().as_slice()).bind(request_id.as_bytes().as_slice()).bind([1_u8;32].as_slice()).execute(&mut connection).await.map_err(StorageError::from).unwrap();
    let read = store.refresh(original.auth_id, request_id).await;
    let adoption = store.create_pending(original.clone()).await;
    sqlx::query("DELETE FROM auth_refresh_requests WHERE auth_id=? AND request_id=?")
        .bind(original.auth_id.as_bytes().as_slice())
        .bind(request_id.as_bytes().as_slice())
        .execute(&mut connection)
        .await
        .map_err(StorageError::from)
        .unwrap();
    assert_eq!(read.err(), Some(StorageError::Integrity));
    assert_eq!(adoption.err(), Some(StorageError::Integrity));
    original.encrypted_oauth_context.as_mut().unwrap()[20] ^= 1;
    assert_eq!(
        store.create_pending(original.clone()).await.err(),
        Some(StorageError::Integrity)
    );
    assert!(store.session(original.auth_id).await.unwrap().is_none());
}

#[tokio::test]
#[ignore = "requires explicit isolated MySQL runtime and migration URLs"]
async fn mysql_concurrent_create_and_refresh_have_one_winner() {
    let store = fixture().await;
    let original = pending();
    let results = tokio::join!(
        store.create_pending(original.clone()),
        store.create_pending(original)
    );
    assert_eq!(
        usize::from(results.0.is_ok()) + usize::from(results.1.is_ok()),
        1
    );
    let original = active(&store).await;
    let first = store.apply(Mutation::StartRefresh {
        expected: original.clone(),
        request: request(&original),
    });
    let second = store.apply(Mutation::StartRefresh {
        expected: original.clone(),
        request: request(&original),
    });
    let results = tokio::join!(first, second);
    assert_eq!(
        usize::from(results.0.is_ok()) + usize::from(results.1.is_ok()),
        1
    );
}

async fn starting_refresh(store: &Store) -> (Session, RefreshRequest) {
    let original = active(store).await;
    let request = request(&original);
    store
        .apply(Mutation::StartRefresh {
            expected: original.clone(),
            request: request.clone(),
        })
        .await
        .unwrap();
    (
        store.session(original.auth_id).await.unwrap().unwrap(),
        request,
    )
}
fn completion(session: &Session, request: &RefreshRequest) -> (Session, RefreshRequest) {
    let mut next = session.clone();
    next.status = "active".into();
    next.clear_login();
    next.credential_generation = Some(request.input_generation + 1);
    next.hmac_key_version = 2;
    next.broker_hash = Some([11; 32]);
    next.refresh_token_hash = Some([12; 32]);
    let mut completed = request.clone();
    completed.status = "completed".into();
    completed.expires_at = now() + Duration::minutes(4);
    completed.encrypted_response = Some(
        crypto()
            .encrypt(
                2,
                &Binding::refresh(next.auth_id, request.request_id, request.input_generation)
                    .unwrap(),
                b"refresh-result",
            )
            .unwrap()
            .to_vec(),
    );
    (next, completed)
}
fn finishing(
    session: &Session,
    request: &RefreshRequest,
    next: Session,
    completed: RefreshRequest,
) -> Mutation {
    Mutation::FinishRefresh {
        expected: session.clone(),
        next: next.into(),
        request: request.clone(),
        next_request: completed,
        fence: RefreshFence::capture(session, request).unwrap(),
    }
}

#[tokio::test]
#[ignore = "requires explicit isolated MySQL runtime and migration URLs"]
async fn mysql_refresh_deadline_cannot_exceed_authorization_budget() {
    let store = fixture().await;
    let (session, request) = starting_refresh(&store).await;
    let (mut next, completed) = completion(&session, &request);
    next.auth_expires_at = Some(now() + Duration::days(31));
    assert_eq!(
        store
            .apply(finishing(&session, &request, next, completed))
            .await
            .err(),
        Some(StorageError::Conflict)
    );
    assert!(store.session(session.auth_id).await.unwrap().unwrap() == session);
}

#[tokio::test]
#[ignore = "requires explicit isolated MySQL URLs; migration test account needs TRIGGER"]
async fn mysql_completion_failure_rolls_back_both_a_and_r() {
    let store = fixture().await;
    let (session, request) = starting_refresh(&store).await;
    let (next, completed) = completion(&session, &request);
    let mut connection = admin().await;
    sqlx::raw_sql("DROP TRIGGER IF EXISTS ask_auth_test_update_failure")
        .execute(&mut connection)
        .await
        .map_err(StorageError::from)
        .unwrap();
    let trigger = format!("CREATE TRIGGER ask_auth_test_update_failure BEFORE UPDATE ON auth_refresh_requests FOR EACH ROW BEGIN IF NEW.auth_id=UNHEX('{}') THEN SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT='injected_storage_failure'; END IF; END", session.auth_id.simple());
    // Only a typed UUID rendered as fixed hexadecimal is interpolated; no user SQL is accepted.
    sqlx::raw_sql(sqlx::AssertSqlSafe(trigger))
        .execute(&mut connection)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "test trigger DB code: {:?}",
                error.as_database_error().map(|error| error
                    .downcast_ref::<sqlx::mysql::MySqlDatabaseError>()
                    .number())
            )
        });
    let outcome = store
        .apply(finishing(&session, &request, next, completed))
        .await;
    sqlx::raw_sql("DROP TRIGGER IF EXISTS ask_auth_test_update_failure")
        .execute(&mut connection)
        .await
        .map_err(StorageError::from)
        .unwrap();
    assert!(outcome.is_err());
    let (stored_session, stored_request) = store
        .refresh(session.auth_id, request.request_id)
        .await
        .unwrap()
        .unwrap();
    assert!(stored_session == session);
    assert!(stored_request == request);
}

#[tokio::test]
#[ignore = "requires explicit isolated MySQL runtime and migration URLs"]
async fn mysql_expire_login_uses_original_fence_and_never_extends_deadline() {
    let store = fixture().await;
    let original = expiring_pending(&store).await;
    let fence = CallbackFence::capture(&original).unwrap();
    assert!(store
        .apply(Mutation::ExpireLogin {
            expected: original.clone(),
            fence: fence.clone()
        })
        .await
        .is_err());
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    store
        .apply(Mutation::ExpireLogin {
            expected: original.clone(),
            fence,
        })
        .await
        .unwrap();
    let expired = store.session(original.auth_id).await.unwrap().unwrap();
    assert_eq!(expired.status, "expired");
    assert!(expired.login_expires_at <= original.login_expires_at);
    assert!(expired.encrypted_oauth_context.is_none());
    assert_eq!(expired.oauth_state_hash, original.oauth_state_hash);
    store
        .apply(Mutation::DeleteSession { expected: expired })
        .await
        .unwrap();
    assert!(store.session(original.auth_id).await.unwrap().is_none());
}

#[tokio::test]
#[ignore = "requires explicit isolated MySQL runtime and migration URLs"]
async fn mysql_expired_refresh_lease_rejects_success_then_unknown_ends_authorization() {
    let store = fixture().await;
    let original = active(&store).await;
    let mut request = request(&original);
    request.expires_at = now() + Duration::milliseconds(200);
    store
        .apply(Mutation::StartRefresh {
            expected: original.clone(),
            request: request.clone(),
        })
        .await
        .unwrap();
    let session = store.session(original.auth_id).await.unwrap().unwrap();
    let (next, completed) = completion(&session, &request);
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    assert!(store
        .apply(finishing(&session, &request, next, completed))
        .await
        .is_err());
    let mut next = session.clone();
    next.status = "reauth_required".into();
    next.reason_code = Some("refresh_outcome_unknown".into());
    next.clear_login();
    let mut unknown = request.clone();
    unknown.status = "unknown".into();
    unknown.error_code = Some("refresh_outcome_unknown".into());
    store
        .apply(finishing(&session, &request, next, unknown))
        .await
        .unwrap();
    let (ended, request) = store
        .refresh(session.auth_id, request.request_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ended.status, "reauth_required");
    assert!(ended.auth_expires_at <= Some(now()));
    assert!(request.expires_at <= now());
    assert!(store
        .apply(Mutation::StartRefresh {
            expected: ended.clone(),
            request: self::request(&ended)
        })
        .await
        .is_err());
}

#[tokio::test]
#[ignore = "requires explicit isolated MySQL runtime and migration URLs"]
async fn mysql_reads_reject_unknown_versions_bad_tags_and_partial_rows() {
    let store = fixture().await;
    let original = store.create_pending(pending()).await.unwrap();
    let mut connection = admin().await;
    for statement in [
        "UPDATE auth_sessions SET hmac_key_version=65000 WHERE auth_id=?",
        "UPDATE auth_sessions SET oauth_state_hash=NULL WHERE auth_id=?",
        "UPDATE auth_sessions SET encrypted_oauth_context=REPEAT(0x01,32) WHERE auth_id=?",
    ] {
        sqlx::query(statement)
            .bind(original.auth_id.as_bytes().as_slice())
            .execute(&mut connection)
            .await
            .map_err(StorageError::from)
            .unwrap();
        let outcome = store.session(original.auth_id).await;
        let absent_child = store.refresh(original.auth_id, Uuid::now_v7()).await;
        sqlx::query("UPDATE auth_sessions SET hmac_key_version=?,oauth_state_hash=?,encrypted_oauth_context=? WHERE auth_id=?")
            .bind(original.hmac_key_version).bind(original.oauth_state_hash.unwrap().to_vec())
            .bind(original.encrypted_oauth_context.as_ref().unwrap()).bind(original.auth_id.as_bytes().as_slice())
            .execute(&mut connection).await.map_err(StorageError::from).unwrap();
        assert_eq!(outcome.err(), Some(StorageError::Integrity));
        assert_eq!(absent_child.err(), Some(StorageError::Integrity));
    }
    assert!(store.session(original.auth_id).await.unwrap().unwrap() == original);
}

#[tokio::test]
#[ignore = "requires explicit isolated MySQL runtime and migration URLs"]
async fn mysql_expired_login_cache_clears_all_eight_fields_without_ending_auth() {
    let store = fixture().await;
    let original = active(&store).await;
    assert!(store
        .apply(Mutation::ClearLogin {
            expected: original.clone()
        })
        .await
        .is_err());
    let mut connection = admin().await;
    sqlx::query("UPDATE auth_sessions SET login_expires_at=UTC_TIMESTAMP(3)-INTERVAL 1 SECOND WHERE auth_id=?")
        .bind(original.auth_id.as_bytes().as_slice()).execute(&mut connection).await.map_err(StorageError::from).unwrap();
    let expired_cache = store.session(original.auth_id).await.unwrap().unwrap();
    store
        .apply(Mutation::ClearLogin {
            expected: expired_cache.clone(),
        })
        .await
        .unwrap();
    let cleared = store.session(original.auth_id).await.unwrap().unwrap();
    let mut expected = expired_cache;
    expected.clear_login();
    assert!(cleared == expected);
    assert_eq!(cleared.status, "active");
    assert_eq!(cleared.auth_expires_at, original.auth_expires_at);
    assert!(store.create_pending(original).await.is_err());
}

#[tokio::test]
#[ignore = "requires explicit isolated MySQL runtime and migration URLs"]
async fn mysql_contended_lock_error_rolls_back_without_late_mutation() {
    use sqlx::Connection;
    let store = fixture().await;
    let original = store.create_pending(pending()).await.unwrap();
    let mut connection = admin().await;
    let mut blocker = connection
        .begin()
        .await
        .map_err(StorageError::from)
        .unwrap();
    sqlx::query("SELECT auth_id FROM auth_sessions WHERE auth_id=? FOR UPDATE")
        .bind(original.auth_id.as_bytes().as_slice())
        .fetch_one(&mut *blocker)
        .await
        .map_err(StorageError::from)
        .unwrap();
    let mut exchanging = original.clone();
    exchanging.status = "exchanging".into();
    let outcome = store
        .apply(Mutation::Callback {
            expected: original.clone(),
            next: exchanging,
            fence: CallbackFence::capture(&original).unwrap(),
        })
        .await;
    blocker
        .rollback()
        .await
        .map_err(StorageError::from)
        .unwrap();
    assert!(outcome.is_err());
    assert!(store.session(original.auth_id).await.unwrap().unwrap() == original);
}

async fn revoke(store: &Store, original: &Session) -> Session {
    let mut revoked = original.clone();
    revoked.status = "revoked".into();
    revoked.clear_login();
    revoked.reason_code = Some("revoked".into());
    store
        .apply(Mutation::EndAuthorization {
            expected: original.clone(),
            next: revoked,
        })
        .await
        .unwrap();
    store.session(original.auth_id).await.unwrap().unwrap()
}
async fn settle_and_delete(store: &Store, revoked: &Session, request: RefreshRequest) {
    store
        .apply(Mutation::SettleEndedRefresh {
            expected: revoked.clone(),
            request: request.clone(),
        })
        .await
        .unwrap();
    let (_, settled) = store
        .refresh(revoked.auth_id, request.request_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(settled.status, "unknown");
    store
        .apply(Mutation::DeleteRefresh {
            expected: revoked.clone(),
            request: settled,
        })
        .await
        .unwrap();
    store
        .apply(Mutation::DeleteSession {
            expected: revoked.clone(),
        })
        .await
        .unwrap();
}
async fn assert_original_callback_cannot_adopt_replacement(store: &Store, original: &Session) {
    let mut replacement = pending();
    replacement.auth_id = original.auth_id;
    replacement.encrypted_oauth_context = Some(
        crypto()
            .encrypt(
                1,
                &Binding::context(original.auth_id),
                b"different-operation",
            )
            .unwrap()
            .to_vec(),
    );
    replacement.oauth_state_hash = Some([99; 32]);
    let replacement = store.create_pending(replacement).await.unwrap();
    let mut stale = replacement.clone();
    stale.status = "exchanging".into();
    assert!(store
        .apply(Mutation::Callback {
            expected: replacement,
            next: stale,
            fence: CallbackFence::capture(original).unwrap()
        })
        .await
        .is_err());
}

async fn assert_cancelled_migration_releases_lock(
    migration: &MigrationDatabase,
    connection: &mut sqlx::MySqlConnection,
) {
    let lock: String =
        sqlx::query_scalar("SELECT LOWER(CONV(CRC32(DATABASE()) * 1026731422,10,16))")
            .fetch_one(&mut *connection)
            .await
            .map_err(StorageError::from)
            .unwrap();
    let acquired: i64 = sqlx::query_scalar("SELECT GET_LOCK(?,1)")
        .bind(&lock)
        .fetch_one(&mut *connection)
        .await
        .map_err(StorageError::from)
        .unwrap();
    assert_eq!(acquired, 1);
    let cancelled =
        tokio::time::timeout(std::time::Duration::from_millis(100), migration.migrate()).await;
    sqlx::query("SELECT RELEASE_LOCK(?)")
        .bind(&lock)
        .execute(&mut *connection)
        .await
        .map_err(StorageError::from)
        .unwrap();
    assert!(cancelled.is_err());
    tokio::time::timeout(std::time::Duration::from_secs(3), migration.migrate())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
#[ignore = "requires explicit isolated MySQL runtime and migration URLs"]
async fn mysql_original_callback_cannot_adopt_recreated_auth_id() {
    let store = fixture().await;
    let original = expiring_pending(&store).await;
    let fence = CallbackFence::capture(&original).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    store
        .apply(Mutation::ExpireLogin {
            expected: original.clone(),
            fence,
        })
        .await
        .unwrap();
    let expired = store.session(original.auth_id).await.unwrap().unwrap();
    store
        .apply(Mutation::DeleteSession { expected: expired })
        .await
        .unwrap();
    assert_original_callback_cannot_adopt_replacement(&store, &original).await;
}

#[tokio::test]
#[ignore = "requires explicit isolated MySQL runtime and migration URLs"]
async fn mysql_creation_deadline_uses_database_clock_and_duplicate_never_renews() {
    let store = fixture().await;
    let mut original = pending();
    original.login_expires_at = Some(now() - Duration::days(1));
    assert!(store.create_pending(original.clone()).await.is_ok());
    let first = store.session(original.auth_id).await.unwrap().unwrap();
    let mut connection = admin().await;
    let remaining: i64 = sqlx::query_scalar("SELECT TIMESTAMPDIFF(SECOND,UTC_TIMESTAMP(3),login_expires_at) FROM auth_sessions WHERE auth_id=?")
        .bind(original.auth_id.as_bytes().as_slice()).fetch_one(&mut connection).await.map_err(StorageError::from).unwrap();
    assert!((598..=600).contains(&remaining));
    original.login_expires_at = Some(now() + Duration::days(1));
    assert_eq!(
        store.create_pending(original).await.err(),
        Some(StorageError::Conflict)
    );
    assert!(store.session(first.auth_id).await.unwrap().unwrap() == first);
}

async fn expiring_pending(store: &Store) -> Session {
    let original = store.create_pending(pending()).await.unwrap();
    let mut connection = admin().await;
    sqlx::query("UPDATE auth_sessions SET login_expires_at=UTC_TIMESTAMP(3)+INTERVAL 200000 MICROSECOND WHERE auth_id=?")
        .bind(original.auth_id.as_bytes().as_slice()).execute(&mut connection).await.map_err(StorageError::from).unwrap();
    store.session(original.auth_id).await.unwrap().unwrap()
}

#[tokio::test]
#[ignore = "requires explicit isolated MySQL runtime and migration URLs"]
async fn mysql_old_refresh_fence_cannot_complete_another_request_id() {
    let store = fixture().await;
    let (first_session, first_request) = starting_refresh(&store).await;
    let fence = RefreshFence::capture(&first_session, &first_request).unwrap();
    let active = fail_refresh(&store, &first_session, &first_request).await;
    let mut second_request = first_request.clone();
    second_request.request_id = Uuid::now_v7();
    store
        .apply(Mutation::StartRefresh {
            expected: active,
            request: second_request.clone(),
        })
        .await
        .unwrap();
    let second_session = store.session(first_session.auth_id).await.unwrap().unwrap();
    let (next, completed) = completion(&second_session, &second_request);
    let outcome = store
        .apply(Mutation::FinishRefresh {
            expected: second_session.clone(),
            next: next.into(),
            request: second_request.clone(),
            next_request: completed,
            fence,
        })
        .await;
    assert_eq!(outcome.err(), Some(StorageError::Conflict));
    assert!(
        store
            .session(second_session.auth_id)
            .await
            .unwrap()
            .unwrap()
            == second_session
    );
}
async fn fail_refresh(store: &Store, session: &Session, request: &RefreshRequest) -> Session {
    let mut next = session.clone();
    next.status = "active".into();
    let mut failed = request.clone();
    failed.status = "failed".into();
    failed.error_code = Some("refresh_failed".into());
    failed.expires_at = now() + Duration::minutes(4);
    store
        .apply(finishing(session, request, next, failed))
        .await
        .unwrap();
    store.session(session.auth_id).await.unwrap().unwrap()
}

#[tokio::test]
#[ignore = "requires explicit isolated MySQL runtime and migration URLs"]
async fn mysql_callback_fence_cannot_move_to_another_auth_id() {
    let store = fixture().await;
    let first = store.create_pending(pending()).await.unwrap();
    let second = store.create_pending(pending()).await.unwrap();
    let mut connection = admin().await;
    sqlx::query("UPDATE auth_sessions SET login_expires_at=? WHERE auth_id=?")
        .bind(first.login_expires_at)
        .bind(second.auth_id.as_bytes().as_slice())
        .execute(&mut connection)
        .await
        .map_err(StorageError::from)
        .unwrap();
    let second = store.session(second.auth_id).await.unwrap().unwrap();
    let mut exchanging = second.clone();
    exchanging.status = "exchanging".into();
    let outcome = store
        .apply(Mutation::Callback {
            expected: second.clone(),
            next: exchanging,
            fence: CallbackFence::capture(&first).unwrap(),
        })
        .await;
    assert_eq!(outcome.err(), Some(StorageError::Conflict));
    assert!(store.session(second.auth_id).await.unwrap().unwrap() == second);
}

#[tokio::test]
#[ignore = "requires explicit isolated MySQL runtime and migration URLs"]
async fn mysql_completed_replay_cannot_outlive_authorization_deadline() {
    let store = fixture().await;
    let (session, request) = starting_refresh(&store).await;
    let (mut next, completed) = completion(&session, &request);
    next.auth_expires_at = Some(now() + Duration::seconds(1));
    assert_eq!(
        store
            .apply(finishing(&session, &request, next, completed))
            .await
            .err(),
        Some(StorageError::Conflict)
    );
    assert!(store.session(session.auth_id).await.unwrap().unwrap() == session);
}

#[tokio::test]
#[ignore = "requires explicit isolated MySQL runtime and migration URLs"]
async fn mysql_refresh_lookup_rejects_legacy_operation_ids() {
    let store = fixture().await;
    let legacy_id = Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap();
    assert_eq!(
        store.refresh(Uuid::now_v7(), legacy_id).await.err(),
        Some(StorageError::Invalid)
    );
    assert!(store
        .refresh(Uuid::now_v7(), Uuid::now_v7())
        .await
        .unwrap()
        .is_none());
}
