//! Cleanup acceptance tests use only the explicitly named disposable schema.
use ask_auth::crypto::{Binding, Crypto, KeyMaterial};
use ask_auth::storage::*;
use chrono::{Duration, Utc};
use sqlx::{mysql::MySqlConnectOptions, ConnectOptions, MySqlPool, Row};
use std::{str::FromStr, sync::Arc, time::Duration as StdDuration};
use uuid::Uuid;

async fn fixture() -> (CleanupStore, MySqlPool) {
    let runtime_url = std::env::var("ASK_AUTH_TEST_DATABASE_URL").unwrap();
    let migration_url = std::env::var("ASK_AUTH_TEST_MIGRATION_URL").unwrap();
    let options = guarded_options(&runtime_url, &migration_url)
        .expect("isolated cleanup database targets required")
        .ssl_mode(sqlx::mysql::MySqlSslMode::Required)
        .disable_statement_logging();
    let admin = MySqlPool::connect_with(options).await.unwrap();
    let name: String = sqlx::query_scalar("SELECT DATABASE()")
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(
        name, "ask_auth_stage4_cleanup_test",
        "unsafe cleanup test schema"
    );
    sqlx::raw_sql("DROP TRIGGER IF EXISTS cleanup_test_delay")
        .execute(&admin)
        .await
        .unwrap();
    sqlx::raw_sql("DROP TRIGGER IF EXISTS cleanup_test_retry")
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("DELETE FROM auth_refresh_requests")
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("DELETE FROM auth_sessions")
        .execute(&admin)
        .await
        .unwrap();
    let database = CleanupDatabase::connect(
        DatabaseConfig::new(runtime_url, Tls::LoopbackTlsTestOnly, 4).unwrap(),
    )
    .await
    .unwrap();
    (
        CleanupStore::new(database, "app-a", crypto()).unwrap(),
        admin,
    )
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
#[ignore = "requires explicit isolated cleanup MySQL URLs"]
async fn cleanup_pool_close_is_idempotent_and_operations_fail_closed() {
    let (store, admin) = fixture().await;
    assert!(store.cutoff().await.is_ok());
    tokio::join!(store.close(), store.close());
    assert_eq!(store.cutoff().await, Err(StorageError::Unavailable));
    admin.close().await;
}
async fn pending(admin: &MySqlPool) -> Uuid {
    let auth_id = Uuid::now_v7();
    let context = crypto()
        .encrypt(1, &Binding::context(auth_id), b"opaque fixture context")
        .unwrap();
    sqlx::query("INSERT INTO auth_sessions (auth_id,app_id,status,request_hash,login_ticket_hash,claim_challenge,oauth_state_hash,hmac_key_version,encrypted_oauth_context,login_expires_at) VALUES (?,'app-a','pending',?,?,?,?,1,?,UTC_TIMESTAMP(3)-INTERVAL 1 SECOND)")
        .bind(auth_id.as_bytes().as_slice()).bind([1u8;32].as_slice()).bind([2u8;32].as_slice()).bind([3u8;32].as_slice()).bind([4u8;32].as_slice()).bind(context.to_vec()).execute(admin).await.unwrap();
    auth_id
}
#[tokio::test]
#[ignore = "requires explicit isolated cleanup MySQL URLs"]
async fn expired_pending_is_deleted_without_provider_or_schema_changes() {
    let (store, admin) = fixture().await;
    let auth_id = pending(&admin).await;
    let cutoff = store.cutoff().await.unwrap();
    let batch = store
        .discover(CleanupQueue::Login, cutoff, None, 100)
        .await
        .unwrap();
    assert_eq!(batch.len(), 1);
    let outcome = store
        .process(&batch[0], StdDuration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(outcome.affected_rows, 2);
    let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM auth_sessions WHERE auth_id=?")
        .bind(auth_id.as_bytes().as_slice())
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(exists, 0);
}
async fn authorized(admin: &MySqlPool, status: &str, is_auth_expired: bool) -> Uuid {
    let auth_id = pending(admin).await;
    let payload = crypto()
        .encrypt(1, &Binding::login(auth_id), b"opaque fixture credentials")
        .unwrap();
    let deadline = if is_auth_expired {
        Utc::now().naive_utc() - Duration::seconds(2)
    } else {
        Utc::now().naive_utc() + Duration::hours(1)
    };
    sqlx::query("UPDATE auth_sessions SET status=?,encrypted_oauth_context=NULL,encrypted_result=?,claim_request_id=?,user_id='fixture',scope_json=JSON_ARRAY('read'),broker_hash=?,refresh_token_hash=?,credential_generation=1,auth_expires_at=? WHERE auth_id=?")
        .bind(status).bind(payload.to_vec()).bind(Uuid::now_v7().as_bytes().as_slice()).bind([8u8;32].as_slice()).bind([9u8;32].as_slice()).bind(deadline).bind(auth_id.as_bytes().as_slice()).execute(admin).await.unwrap();
    auth_id
}
async fn refresh(admin: &MySqlPool, auth_id: Uuid, status: &str) -> Uuid {
    let request_id = Uuid::now_v7();
    let error = (status != "pending").then_some("refresh_failed");
    sqlx::query("INSERT INTO auth_refresh_requests (auth_id,request_id,request_hash,hmac_key_version,input_generation,status,expires_at,error_code) VALUES (?,?,?,1,1,?,UTC_TIMESTAMP(3)-INTERVAL 1 SECOND,?)")
        .bind(auth_id.as_bytes().as_slice()).bind(request_id.as_bytes().as_slice()).bind([11u8;32].as_slice()).bind(status).bind(error).execute(admin).await.unwrap();
    request_id
}
async fn candidates(store: &CleanupStore, queue: CleanupQueue) -> Vec<CleanupCandidate> {
    store
        .discover(queue, store.cutoff().await.unwrap(), None, 100)
        .await
        .unwrap()
}
async fn row(admin: &MySqlPool, auth_id: Uuid) -> sqlx::mysql::MySqlRow {
    sqlx::query("SELECT * FROM auth_sessions WHERE auth_id=?")
        .bind(auth_id.as_bytes().as_slice())
        .fetch_one(admin)
        .await
        .unwrap()
}
#[tokio::test]
#[ignore = "requires explicit isolated cleanup MySQL URLs"]
async fn parent_lock_skips_child_without_waiting_or_mutation() {
    let (store, admin) = fixture().await;
    let auth_id = authorized(&admin, "refreshing", false).await;
    refresh(&admin, auth_id, "pending").await;
    let batch = candidates(&store, CleanupQueue::Pending).await;
    let mut transaction = admin.begin().await.unwrap();
    sqlx::query("SELECT auth_id FROM auth_sessions WHERE auth_id=? FOR UPDATE")
        .bind(auth_id.as_bytes().as_slice())
        .fetch_one(&mut *transaction)
        .await
        .unwrap();
    let result = store
        .process(&batch[0], StdDuration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(result.affected_rows, 0);
    assert_eq!(result.lock_skips, 1);
    transaction.rollback().await.unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM auth_refresh_requests")
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(status, "pending");
}
#[tokio::test]
#[ignore = "requires explicit isolated cleanup MySQL URLs"]
async fn active_login_gc_clears_exactly_eight_fields_and_preserves_authorization() {
    let (store, admin) = fixture().await;
    for status in ["active", "refreshing"] {
        let auth_id = authorized(&admin, status, false).await;
        let before = row(&admin, auth_id).await;
        let batch = candidates(&store, CleanupQueue::Login).await;
        assert_eq!(
            store
                .process(batch.last().unwrap(), StdDuration::from_secs(1))
                .await
                .unwrap()
                .affected_rows,
            1
        );
        let after = row(&admin, auth_id).await;
        assert_eq!(
            after.try_get_unchecked::<String, _>("status").unwrap(),
            status
        );
        for column in ["broker_hash", "refresh_token_hash"] {
            assert!(
                after.try_get::<Vec<u8>, _>(column).unwrap()
                    == before.try_get::<Vec<u8>, _>(column).unwrap()
            );
        }
        assert_eq!(after.try_get::<u64, _>("credential_generation").unwrap(), 1);
        for column in [
            "request_hash",
            "login_ticket_hash",
            "claim_challenge",
            "oauth_state_hash",
            "encrypted_oauth_context",
            "claim_request_id",
            "encrypted_result",
        ] {
            assert!(after
                .try_get::<Option<Vec<u8>>, _>(column)
                .unwrap()
                .is_none());
        }
        assert!(after
            .try_get::<Option<chrono::NaiveDateTime>, _>("login_expires_at")
            .unwrap()
            .is_none());
    }
}
#[tokio::test]
#[ignore = "requires explicit isolated cleanup MySQL URLs"]
async fn abandoned_refresh_becomes_atomic_unknown_without_extending_deadline() {
    let (store, admin) = fixture().await;
    let auth_id = authorized(&admin, "refreshing", false).await;
    refresh(&admin, auth_id, "pending").await;
    let batch = candidates(&store, CleanupQueue::Pending).await;
    assert_eq!(
        store
            .process(&batch[0], StdDuration::from_secs(1))
            .await
            .unwrap()
            .affected_rows,
        2
    );
    let parent = row(&admin, auth_id).await;
    assert_eq!(
        parent.try_get_unchecked::<String, _>("status").unwrap(),
        "reauth_required"
    );
    let status: String = sqlx::query_scalar("SELECT status FROM auth_refresh_requests")
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(status, "unknown");
    assert!(
        parent
            .try_get::<chrono::NaiveDateTime, _>("auth_expires_at")
            .unwrap()
            <= store.cutoff().await.unwrap()
    );
}
#[tokio::test]
#[ignore = "requires explicit isolated cleanup MySQL URLs"]
async fn old_candidate_never_expires_recreated_id_or_new_claim() {
    let (store, admin) = fixture().await;
    let auth_id = pending(&admin).await;
    let batch = candidates(&store, CleanupQueue::Login).await;
    sqlx::query("UPDATE auth_sessions SET oauth_state_hash=? WHERE auth_id=?")
        .bind([99u8; 32].as_slice())
        .bind(auth_id.as_bytes().as_slice())
        .execute(&admin)
        .await
        .unwrap();
    assert_eq!(
        store
            .process(&batch[0], StdDuration::from_secs(1))
            .await
            .unwrap()
            .affected_rows,
        0
    );
    let active_id = authorized(&admin, "active", false).await;
    let batch = candidates(&store, CleanupQueue::Login).await;
    sqlx::query("UPDATE auth_sessions SET login_expires_at=UTC_TIMESTAMP(3)+INTERVAL 5 MINUTE WHERE auth_id=?").bind(active_id.as_bytes().as_slice()).execute(&admin).await.unwrap();
    for candidate in batch {
        store
            .process(&candidate, StdDuration::from_secs(1))
            .await
            .unwrap();
    }
    assert_eq!(
        row(&admin, active_id)
            .await
            .try_get_unchecked::<String, _>("status")
            .unwrap(),
        "active"
    );
}
#[tokio::test]
#[ignore = "requires explicit isolated cleanup MySQL URLs"]
async fn orphan_and_corrupt_rows_fail_closed() {
    let (store, admin) = fixture().await;
    let missing_id = Uuid::now_v7();
    refresh(&admin, missing_id, "failed").await;
    let batch = candidates(&store, CleanupQueue::Failed).await;
    assert_eq!(
        store
            .process(&batch[0], StdDuration::from_secs(1))
            .await
            .err(),
        Some(StorageError::Integrity)
    );
    let auth_id = pending(&admin).await;
    sqlx::query("UPDATE auth_sessions SET hmac_key_version=99 WHERE auth_id=?")
        .bind(auth_id.as_bytes().as_slice())
        .execute(&admin)
        .await
        .unwrap();
    let batch = candidates(&store, CleanupQueue::Login).await;
    assert_eq!(
        store
            .process(&batch[0], StdDuration::from_secs(1))
            .await
            .err(),
        Some(StorageError::Integrity)
    );
}
#[tokio::test]
#[ignore = "requires explicit isolated cleanup MySQL URLs"]
async fn expired_failed_refresh_deletes_child_preserving_live_parent() {
    let (store, admin) = fixture().await;
    let auth_id = authorized(&admin, "active", false).await;
    refresh(&admin, auth_id, "failed").await;
    let batch = candidates(&store, CleanupQueue::Failed).await;
    assert_eq!(
        store
            .process(&batch[0], StdDuration::from_secs(1))
            .await
            .unwrap()
            .affected_rows,
        1
    );
    assert_eq!(
        row(&admin, auth_id)
            .await
            .try_get_unchecked::<String, _>("status")
            .unwrap(),
        "active"
    );
}
#[tokio::test]
#[ignore = "requires explicit isolated cleanup MySQL URLs"]
async fn concurrent_workers_one_winner_and_keyset_advances_locked_head() {
    let (store, admin) = fixture().await;
    let first_id = pending(&admin).await;
    pending(&admin).await;
    let cutoff = store.cutoff().await.unwrap();
    let first = store
        .discover(CleanupQueue::Login, cutoff, None, 1)
        .await
        .unwrap();
    let next = store
        .discover(CleanupQueue::Login, cutoff, Some(&first[0].cursor()), 1)
        .await
        .unwrap();
    assert_eq!(next.len(), 1);
    let mut transaction = admin.begin().await.unwrap();
    sqlx::query("SELECT auth_id FROM auth_sessions WHERE auth_id=? FOR UPDATE")
        .bind(first_id.as_bytes().as_slice())
        .fetch_one(&mut *transaction)
        .await
        .unwrap();
    assert_eq!(
        store
            .process(&first[0], StdDuration::from_secs(1))
            .await
            .unwrap()
            .lock_skips,
        1
    );
    assert_eq!(
        store
            .process(&next[0], StdDuration::from_secs(1))
            .await
            .unwrap()
            .affected_rows,
        2
    );
    transaction.rollback().await.unwrap();
    let (one, two) = tokio::join!(
        store.process(&first[0], StdDuration::from_secs(1)),
        store.process(&first[0], StdDuration::from_secs(1))
    );
    assert_eq!(one.unwrap().affected_rows + two.unwrap().affected_rows, 2);
}
#[tokio::test]
#[ignore = "requires explicit isolated cleanup MySQL URLs"]
async fn ready_auth_expiry_deletes_without_requiring_login_expiry() {
    let (store, admin) = fixture().await;
    let auth_id = authorized(&admin, "ready", true).await;
    sqlx::query("UPDATE auth_sessions SET claim_request_id=NULL,login_expires_at=UTC_TIMESTAMP(3)+INTERVAL 5 MINUTE WHERE auth_id=?").bind(auth_id.as_bytes().as_slice()).execute(&admin).await.unwrap();
    let batch = candidates(&store, CleanupQueue::Authorization).await;
    assert_eq!(
        store
            .process(&batch[0], StdDuration::from_secs(1))
            .await
            .unwrap()
            .affected_rows,
        2
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM auth_sessions")
            .fetch_one(&admin)
            .await
            .unwrap(),
        0
    );
}
#[tokio::test]
#[ignore = "requires explicit isolated cleanup MySQL URLs"]
async fn terminal_parent_children_are_bounded_and_parent_deleted_last() {
    let (store, admin) = fixture().await;
    let auth_id = authorized(&admin, "refreshing", true).await;
    refresh(&admin, auth_id, "pending").await;
    for _ in 0..4 {
        refresh(&admin, auth_id, "failed").await;
    }
    for iteration in 0..8 {
        let batch = candidates(&store, CleanupQueue::Authorization).await;
        if batch.is_empty() {
            break;
        }
        let outcome = store
            .process(&batch[0], StdDuration::from_secs(1))
            .await
            .unwrap();
        assert!(outcome.affected_rows <= 2);
        let children: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM auth_refresh_requests")
            .fetch_one(&admin)
            .await
            .unwrap();
        if children > 0 {
            let parent = row(&admin, auth_id).await;
            assert_eq!(
                parent.try_get_unchecked::<String, _>("status").unwrap(),
                "expired"
            );
        }
        assert!(iteration <= 7);
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM auth_sessions")
            .fetch_one(&admin)
            .await
            .unwrap(),
        0
    );
}
#[tokio::test]
#[ignore = "requires explicit isolated cleanup MySQL URLs"]
async fn refresh_completion_wins_old_lease_candidate_and_preserves_new_generation() {
    let (store, admin) = fixture().await;
    let auth_id = authorized(&admin, "refreshing", false).await;
    let request_id = refresh(&admin, auth_id, "pending").await;
    let batch = candidates(&store, CleanupQueue::Pending).await;
    let envelope = crypto()
        .encrypt(
            1,
            &Binding::refresh(auth_id, request_id, 1).unwrap(),
            b"opaque completed response",
        )
        .unwrap();
    sqlx::query("UPDATE auth_refresh_requests SET status='completed',encrypted_response=?,expires_at=UTC_TIMESTAMP(3)+INTERVAL 1 MINUTE WHERE auth_id=?").bind(envelope.to_vec()).bind(auth_id.as_bytes().as_slice()).execute(&admin).await.unwrap();
    sqlx::query("UPDATE auth_sessions SET status='active',credential_generation=2,request_hash=NULL,login_ticket_hash=NULL,claim_challenge=NULL,oauth_state_hash=NULL,encrypted_oauth_context=NULL,claim_request_id=NULL,encrypted_result=NULL,login_expires_at=NULL WHERE auth_id=?").bind(auth_id.as_bytes().as_slice()).execute(&admin).await.unwrap();
    assert_eq!(
        store
            .process(&batch[0], StdDuration::from_secs(1))
            .await
            .unwrap()
            .affected_rows,
        0
    );
    assert_eq!(
        row(&admin, auth_id)
            .await
            .try_get::<u64, _>("credential_generation")
            .unwrap(),
        2
    );
}
#[tokio::test]
#[ignore = "requires explicit isolated cleanup MySQL URLs"]
async fn bad_aead_and_unexpected_ready_child_fail_closed_without_deletion() {
    let (store, admin) = fixture().await;
    let auth_id = pending(&admin).await;
    sqlx::query("UPDATE auth_sessions SET encrypted_oauth_context=? WHERE auth_id=?")
        .bind([0u8; 64].as_slice())
        .bind(auth_id.as_bytes().as_slice())
        .execute(&admin)
        .await
        .unwrap();
    let batch = candidates(&store, CleanupQueue::Login).await;
    assert_eq!(
        store
            .process(&batch[0], StdDuration::from_secs(1))
            .await
            .err(),
        Some(StorageError::Integrity)
    );
    sqlx::query("DELETE FROM auth_sessions")
        .execute(&admin)
        .await
        .unwrap();
    let auth_id = authorized(&admin, "ready", false).await;
    sqlx::query("UPDATE auth_sessions SET claim_request_id=NULL WHERE auth_id=?")
        .bind(auth_id.as_bytes().as_slice())
        .execute(&admin)
        .await
        .unwrap();
    refresh(&admin, auth_id, "failed").await;
    let batch = candidates(&store, CleanupQueue::Login).await;
    assert_eq!(
        store
            .process(&batch[0], StdDuration::from_secs(1))
            .await
            .err(),
        Some(StorageError::Integrity)
    );
}
#[tokio::test]
#[ignore = "requires explicit isolated cleanup MySQL URLs"]
async fn transaction_timeout_rolls_back_and_eventually_releases_locks() {
    let (store, admin) = fixture().await;
    pending(&admin).await;
    sqlx::raw_sql("DROP TRIGGER IF EXISTS cleanup_test_delay")
        .execute(&admin)
        .await
        .unwrap();
    sqlx::raw_sql("CREATE TRIGGER cleanup_test_delay BEFORE DELETE ON auth_sessions FOR EACH ROW DO SLEEP(0.2)").execute(&admin).await.unwrap();
    let batch = candidates(&store, CleanupQueue::Login).await;
    let started = std::time::Instant::now();
    let result = store.process(&batch[0], StdDuration::from_millis(20)).await;
    let elapsed = started.elapsed();
    tokio::time::sleep(StdDuration::from_millis(300)).await;
    sqlx::raw_sql("DROP TRIGGER cleanup_test_delay")
        .execute(&admin)
        .await
        .unwrap();
    assert_eq!(result.err(), Some(StorageError::Timeout));
    assert!(elapsed < StdDuration::from_millis(150));
    let mut transaction = admin.begin().await.unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM auth_sessions FOR UPDATE NOWAIT")
        .fetch_one(&mut *transaction)
        .await
        .unwrap();
    assert_eq!(status, "pending");
    transaction.rollback().await.unwrap();
}
#[derive(Default)]
struct Reports(std::sync::Mutex<Vec<u64>>);
impl ask_auth::cleanup::CleanupObserver for Reports {
    fn observe(&self, report: &ask_auth::cleanup::CleanupReport) {
        self.0.lock().unwrap().push(report.affected_rows);
    }
}
#[tokio::test]
#[ignore = "requires explicit isolated cleanup MySQL URLs"]
async fn worker_pause_shutdown_and_rolling_rate() {
    use ask_auth::cleanup::{CleanupControl, CleanupSettings, CleanupWorker};
    let (store, admin) = fixture().await;
    for _ in 0..6 {
        pending(&admin).await;
    }
    let auth_id = authorized(&admin, "active", false).await;
    refresh(&admin, auth_id, "failed").await;
    let (control, source) = CleanupControl::channel();
    let (stop, mut shutdown) = tokio::sync::watch::channel(false);
    let reports = Arc::new(Reports::default());
    let mut worker = CleanupWorker::new(Arc::new(store), control, reports.clone());
    assert!(worker.run_once(&mut shutdown).await.is_paused);
    source
        .send(Some(
            CleanupSettings::new(
                StdDuration::from_secs(1),
                1,
                StdDuration::from_millis(100),
                StdDuration::from_millis(50),
                2,
            )
            .unwrap(),
        ))
        .unwrap();
    let first = worker.run_once(&mut shutdown).await;
    assert!(first.affected_rows <= 2);
    let second = worker.run_once(&mut shutdown).await;
    assert_eq!(
        second.affected_rows, 0,
        "rolling reservation survives a pass boundary"
    );
    source.send(None).unwrap();
    assert!(worker.run_once(&mut shutdown).await.is_paused);
    source.send(Some(CleanupSettings::default())).unwrap();
    stop.send(true).unwrap();
    assert!(worker.run_once(&mut shutdown).await.is_paused);
    assert!(reports.0.lock().unwrap().len() >= 5);
}
#[tokio::test]
#[ignore = "requires explicit isolated cleanup MySQL URLs"]
async fn shutdown_during_initial_delay_starts_no_pass_and_closed_control_stops_worker() {
    use ask_auth::cleanup::{CleanupControl, CleanupSettings, CleanupWorker};
    let (store, _admin) = fixture().await;
    let (control, source) = CleanupControl::channel();
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    let reports = Arc::new(Reports::default());
    let mut worker = CleanupWorker::new(Arc::new(store), control, reports.clone());
    source.send(Some(CleanupSettings::default())).unwrap();
    let running = tokio::spawn(async move {
        worker.run(shutdown).await;
        worker
    });
    tokio::time::sleep(StdDuration::from_millis(20)).await;
    stop.send(true).unwrap();
    let mut worker = tokio::time::timeout(StdDuration::from_millis(200), running)
        .await
        .unwrap()
        .unwrap();
    assert!(reports.0.lock().unwrap().is_empty());
    let (_stop, shutdown) = tokio::sync::watch::channel(false);
    drop(source);
    tokio::time::timeout(StdDuration::from_millis(100), worker.run(shutdown))
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires explicit isolated cleanup MySQL URLs"]
async fn mysql_lock_timeout_number_retries_at_most_three_with_rollback() {
    use ask_auth::cleanup::{CleanupControl, CleanupSettings, CleanupWorker};
    let (store, admin) = fixture().await;
    let auth_id = pending(&admin).await;
    sqlx::raw_sql("CREATE TRIGGER cleanup_test_retry BEFORE DELETE ON auth_sessions FOR EACH ROW SIGNAL SQLSTATE 'HY000' SET MYSQL_ERRNO=1205, MESSAGE_TEXT='SYNTHETIC_RETRY_SECRET_SENTINEL'").execute(&admin).await.unwrap();
    let (control, source) = CleanupControl::channel();
    source.send(Some(CleanupSettings::default())).unwrap();
    let (_stop, mut shutdown) = tokio::sync::watch::channel(false);
    let mut worker = CleanupWorker::new(Arc::new(store), control, Arc::new(Reports::default()));
    let report = worker.run_once(&mut shutdown).await;
    sqlx::raw_sql("DROP TRIGGER cleanup_test_retry")
        .execute(&admin)
        .await
        .unwrap();
    assert_eq!(report.retries, 2);
    assert_eq!(report.failures, 3);
    assert_eq!(report.error, Some(StorageError::Retryable));
    assert_eq!(
        row(&admin, auth_id)
            .await
            .try_get_unchecked::<String, _>("status")
            .unwrap(),
        "pending"
    );
}
#[tokio::test]
#[ignore = "requires explicit isolated cleanup MySQL URLs"]
async fn ready_claim_and_callback_phase_changes_invalidate_original_login_candidates() {
    let (store, admin) = fixture().await;
    let auth_id = authorized(&admin, "ready", false).await;
    sqlx::query("UPDATE auth_sessions SET claim_request_id=NULL WHERE auth_id=?")
        .bind(auth_id.as_bytes().as_slice())
        .execute(&admin)
        .await
        .unwrap();
    let batch = candidates(&store, CleanupQueue::Login).await;
    sqlx::query("UPDATE auth_sessions SET status='active',claim_request_id=?,login_expires_at=UTC_TIMESTAMP(3)+INTERVAL 5 MINUTE WHERE auth_id=?").bind(Uuid::now_v7().as_bytes().as_slice()).bind(auth_id.as_bytes().as_slice()).execute(&admin).await.unwrap();
    assert_eq!(
        store
            .process(&batch[0], StdDuration::from_secs(1))
            .await
            .unwrap()
            .affected_rows,
        0
    );
    assert_eq!(
        row(&admin, auth_id)
            .await
            .try_get_unchecked::<String, _>("status")
            .unwrap(),
        "active"
    );
    let pending_id = pending(&admin).await;
    let batch = candidates(&store, CleanupQueue::Login).await;
    sqlx::query("UPDATE auth_sessions SET status='exchanging' WHERE auth_id=?")
        .bind(pending_id.as_bytes().as_slice())
        .execute(&admin)
        .await
        .unwrap();
    assert_eq!(
        store
            .process(&batch[0], StdDuration::from_secs(1))
            .await
            .unwrap()
            .affected_rows,
        0
    );
    let batch = candidates(&store, CleanupQueue::Login).await;
    assert_eq!(
        store
            .process(&batch[0], StdDuration::from_secs(1))
            .await
            .unwrap()
            .affected_rows,
        2
    );
}
#[tokio::test]
#[ignore = "requires explicit isolated cleanup MySQL URLs"]
async fn db_clock_recheck_rejects_future_cutoff_and_same_id_recreation() {
    let (store, admin) = fixture().await;
    let auth_id = pending(&admin).await;
    let original = candidates(&store, CleanupQueue::Login).await;
    sqlx::query("DELETE FROM auth_sessions WHERE auth_id=?")
        .bind(auth_id.as_bytes().as_slice())
        .execute(&admin)
        .await
        .unwrap();
    let replacement_id = pending(&admin).await;
    let context = crypto()
        .encrypt(1, &Binding::context(auth_id), b"replacement fixture")
        .unwrap();
    sqlx::query("UPDATE auth_sessions SET auth_id=?,oauth_state_hash=?,encrypted_oauth_context=?,login_expires_at=UTC_TIMESTAMP(3)+INTERVAL 5 MINUTE WHERE auth_id=?")
        .bind(auth_id.as_bytes().as_slice()).bind([99u8;32].as_slice()).bind(context.to_vec()).bind(replacement_id.as_bytes().as_slice()).execute(&admin).await.unwrap();
    assert_eq!(
        store
            .process(&original[0], StdDuration::from_secs(1))
            .await
            .unwrap()
            .affected_rows,
        0
    );
    let future = store.cutoff().await.unwrap() + Duration::hours(1);
    let batch = store
        .discover(CleanupQueue::Login, future, None, 100)
        .await
        .unwrap();
    assert_eq!(batch.len(), 1);
    assert_eq!(
        store
            .process(&batch[0], StdDuration::from_secs(1))
            .await
            .unwrap()
            .affected_rows,
        0
    );
}
async fn end_parent(admin: &MySqlPool, auth_id: Uuid) {
    sqlx::query("UPDATE auth_sessions SET status='revoked',reason_code='revoked',auth_expires_at=UTC_TIMESTAMP(3),request_hash=NULL,login_ticket_hash=NULL,claim_challenge=NULL,oauth_state_hash=NULL,encrypted_oauth_context=NULL,claim_request_id=NULL,encrypted_result=NULL,login_expires_at=NULL WHERE auth_id=?")
        .bind(auth_id.as_bytes().as_slice()).execute(admin).await.unwrap();
}
#[tokio::test]
#[ignore = "requires explicit isolated cleanup MySQL URLs"]
async fn logout_reason_survives_settling_future_pending_and_future_replay_is_retained() {
    let (store, admin) = fixture().await;
    let auth_id = authorized(&admin, "refreshing", false).await;
    refresh(&admin, auth_id, "pending").await;
    sqlx::query("UPDATE auth_refresh_requests SET expires_at=UTC_TIMESTAMP(3)+INTERVAL 30 SECOND")
        .execute(&admin)
        .await
        .unwrap();
    end_parent(&admin, auth_id).await;
    let batch = candidates(&store, CleanupQueue::Authorization).await;
    assert_eq!(
        store
            .process(&batch[0], StdDuration::from_secs(1))
            .await
            .unwrap()
            .affected_rows,
        1
    );
    assert_revoked(&admin, auth_id).await;
    let status: String = sqlx::query_scalar("SELECT status FROM auth_refresh_requests")
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(status, "unknown");
    let unknown = candidates(&store, CleanupQueue::Unknown).await;
    store
        .process(&unknown[0], StdDuration::from_secs(1))
        .await
        .unwrap();
    refresh(&admin, auth_id, "failed").await;
    sqlx::query("UPDATE auth_refresh_requests SET expires_at=UTC_TIMESTAMP(3)+INTERVAL 1 MINUTE")
        .execute(&admin)
        .await
        .unwrap();
    let batch = candidates(&store, CleanupQueue::Authorization).await;
    assert_eq!(
        store
            .process(&batch[0], StdDuration::from_secs(1))
            .await
            .unwrap()
            .affected_rows,
        0
    );
    assert_revoked(&admin, auth_id).await;
}
#[tokio::test]
#[ignore = "requires explicit isolated cleanup MySQL URLs"]
async fn worker_rotation_and_bounded_cycles_revisit_released_head_under_new_arrivals() {
    use ask_auth::cleanup::{CleanupControl, CleanupSettings, CleanupWorker};
    let (store, admin) = fixture().await;
    let head = pending(&admin).await;
    let auth_id = authorized(&admin, "active", false).await;
    refresh(&admin, auth_id, "failed").await;
    let mut transaction = admin.begin().await.unwrap();
    sqlx::query("SELECT auth_id FROM auth_sessions WHERE auth_id=? FOR UPDATE")
        .bind(head.as_bytes().as_slice())
        .fetch_one(&mut *transaction)
        .await
        .unwrap();
    let (control, source) = CleanupControl::channel();
    source
        .send(Some(
            CleanupSettings::new(
                StdDuration::from_secs(1),
                1,
                StdDuration::from_millis(500),
                StdDuration::from_millis(100),
                100,
            )
            .unwrap(),
        ))
        .unwrap();
    let (_stop, mut shutdown) = tokio::sync::watch::channel(false);
    let mut worker = CleanupWorker::new(Arc::new(store), control, Arc::new(Reports::default()));
    let report = worker.run_once(&mut shutdown).await;
    assert!(report.lock_skips >= 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM auth_refresh_requests")
            .fetch_one(&admin)
            .await
            .unwrap(),
        0
    );
    transaction.rollback().await.unwrap();
    for _ in 0..4 {
        pending(&admin).await;
        worker.run_once(&mut shutdown).await;
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM auth_sessions WHERE auth_id=?")
            .bind(head.as_bytes().as_slice())
            .fetch_one(&admin)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
#[ignore = "requires explicit isolated cleanup MySQL URLs"]
async fn hmac_rotation_invalidates_old_login_gc_and_expired_completed_deletes_child_only() {
    let (store, admin) = fixture().await;
    let auth_id = authorized(&admin, "refreshing", false).await;
    let request_id = refresh(&admin, auth_id, "pending").await;
    let login = candidates(&store, CleanupQueue::Login).await;
    let response = crypto()
        .encrypt(
            1,
            &Binding::refresh(auth_id, request_id, 1).unwrap(),
            b"opaque completed fixture",
        )
        .unwrap();
    sqlx::query("UPDATE auth_refresh_requests SET status='completed',encrypted_response=?,expires_at=UTC_TIMESTAMP(3)-INTERVAL 1 SECOND WHERE auth_id=?").bind(response.to_vec()).bind(auth_id.as_bytes().as_slice()).execute(&admin).await.unwrap();
    sqlx::query("UPDATE auth_sessions SET status='active',hmac_key_version=2,credential_generation=2,broker_hash=?,refresh_token_hash=?,request_hash=NULL,login_ticket_hash=NULL,claim_challenge=NULL,oauth_state_hash=NULL,encrypted_oauth_context=NULL,claim_request_id=NULL,encrypted_result=NULL,login_expires_at=NULL WHERE auth_id=?")
        .bind([44u8;32].as_slice()).bind([45u8;32].as_slice()).bind(auth_id.as_bytes().as_slice()).execute(&admin).await.unwrap();
    assert_eq!(
        store
            .process(&login[0], StdDuration::from_secs(1))
            .await
            .unwrap()
            .affected_rows,
        0
    );
    let completed = candidates(&store, CleanupQueue::Completed).await;
    assert_eq!(
        store
            .process(&completed[0], StdDuration::from_secs(1))
            .await
            .unwrap()
            .affected_rows,
        1
    );
    let parent = row(&admin, auth_id).await;
    assert_eq!(parent.try_get::<u16, _>("hmac_key_version").unwrap(), 2);
    assert_eq!(
        parent.try_get::<u64, _>("credential_generation").unwrap(),
        2
    );
    assert_eq!(
        parent.try_get_unchecked::<String, _>("status").unwrap(),
        "active"
    );
}
#[tokio::test]
#[ignore = "requires explicit isolated cleanup MySQL URLs"]
async fn whole_pass_timeout_is_failure_with_conservative_unknown_write_reservation() {
    use ask_auth::cleanup::{CleanupControl, CleanupSettings, CleanupWorker};
    let (store, admin) = fixture().await;
    let auth_id = pending(&admin).await;
    sqlx::raw_sql("CREATE TRIGGER cleanup_test_delay BEFORE DELETE ON auth_sessions FOR EACH ROW BEGIN DO GET_LOCK('ask_auth_cleanup_test_entered',0); DO SLEEP(0.5); END").execute(&admin).await.unwrap();
    let (control, source) = CleanupControl::channel();
    let settings = CleanupSettings::new(
        StdDuration::from_secs(1),
        100,
        StdDuration::from_millis(300),
        StdDuration::from_millis(100),
        2,
    )
    .unwrap();
    source.send(Some(settings)).unwrap();
    let (_stop, mut shutdown) = tokio::sync::watch::channel(false);
    let mut worker = CleanupWorker::new(Arc::new(store), control, Arc::new(Reports::default()));
    let started = std::time::Instant::now();
    let running = tokio::spawn(async move {
        let report = worker.run_once(&mut shutdown).await;
        (worker, report, shutdown)
    });
    let has_entered = await_trigger_entry(&admin).await;
    let (mut worker, report, mut shutdown) = running.await.unwrap();
    let elapsed = started.elapsed();
    tokio::time::sleep(StdDuration::from_millis(600)).await;
    sqlx::raw_sql("DROP TRIGGER cleanup_test_delay")
        .execute(&admin)
        .await
        .unwrap();
    assert!(has_entered, "worker never entered the guarded DML trigger");
    assert_eq!(
        report.error,
        Some(StorageError::Timeout),
        "transactions={} failures={} retries={} rows={}",
        report.transactions,
        report.failures,
        report.retries,
        report.affected_rows
    );
    assert!(report.transactions >= 1 && report.failures >= 1);
    assert!(elapsed < StdDuration::from_millis(500));
    let parent = row(&admin, auth_id).await;
    assert_eq!(
        parent.try_get_unchecked::<String, _>("status").unwrap(),
        "pending"
    );
    assert_eq!(worker.run_once(&mut shutdown).await.affected_rows, 0);
}
async fn await_trigger_entry(admin: &MySqlPool) -> bool {
    tokio::time::timeout(StdDuration::from_secs(1), async {
        loop {
            let owner: Option<u64> =
                sqlx::query_scalar("SELECT IS_USED_LOCK('ask_auth_cleanup_test_entered')")
                    .fetch_one(admin)
                    .await
                    .unwrap();
            if owner.is_some() {
                return true;
            }
            tokio::time::sleep(StdDuration::from_millis(2)).await;
        }
    })
    .await
    .unwrap_or(false)
}

async fn assert_revoked(admin: &MySqlPool, auth_id: Uuid) {
    let parent = row(admin, auth_id).await;
    for column in ["status", "reason_code"] {
        assert_eq!(
            parent.try_get_unchecked::<String, _>(column).unwrap(),
            "revoked"
        );
    }
}

fn guarded_options(
    runtime_url: &str,
    migration_url: &str,
) -> Result<MySqlConnectOptions, StorageError> {
    // [Review Fix #49] Both independent targets must be disposable before any connection or destructive fixture work.
    let runtime = MySqlConnectOptions::from_str(runtime_url).map_err(|_| StorageError::Invalid)?;
    let migration =
        MySqlConnectOptions::from_str(migration_url).map_err(|_| StorageError::Invalid)?;
    for options in [&runtime, &migration] {
        let is_local = options
            .get_host()
            .parse::<std::net::IpAddr>()
            .is_ok_and(|host| host.is_loopback());
        if options.get_database() != Some("ask_auth_stage4_cleanup_test")
            || options.get_socket().is_some()
            || !is_local
        {
            return Err(StorageError::Invalid);
        }
    }
    Ok(migration)
}
#[test]
fn cleanup_fixture_rejects_wrong_runtime_target_before_network_or_deletes() {
    let isolated = "mysql://fixture:synthetic@127.0.0.1/ask_auth_stage4_cleanup_test";
    let wrong_runtime = "mysql://fixture:synthetic@127.0.0.1/another_schema";
    assert!(guarded_options(wrong_runtime, isolated).is_err());
}

#[test]
fn cleanup_fixture_requires_both_exact_schemas_numeric_loopback_and_no_socket() {
    let isolated = "mysql://fixture:synthetic@127.0.0.1/ask_auth_stage4_cleanup_test";
    assert!(guarded_options(isolated, isolated).is_ok());
    for rejected in [
        "mysql://fixture:synthetic@127.0.0.1/another_schema",
        "mysql://fixture:synthetic@192.0.2.1/ask_auth_stage4_cleanup_test",
        "mysql://fixture:synthetic@localhost/ask_auth_stage4_cleanup_test",
        "mysql://fixture:synthetic@127.0.0.1/ask_auth_stage4_cleanup_test?socket=/private/tmp/synthetic.sock",
        "invalid-url",
    ] {
        assert!(guarded_options(rejected, isolated).is_err());
        assert!(guarded_options(isolated, rejected).is_err());
    }
}
