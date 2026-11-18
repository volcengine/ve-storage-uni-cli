use super::*;
use crate::storage::CleanupDatabase;
use sqlx::{Connection, Row};
use std::time::Duration as StdDuration;

/// Fairly rotated expiry queues; refresh scans each fix exactly one status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CleanupQueue {
    Login,
    Authorization,
    Pending,
    Completed,
    Failed,
    Unknown,
}
impl CleanupQueue {
    /// Stable queue order for round-robin scheduling.
    pub const ALL: [Self; 6] = [
        Self::Login,
        Self::Authorization,
        Self::Pending,
        Self::Completed,
        Self::Failed,
        Self::Unknown,
    ];
    fn status(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
            _ => "",
        }
    }
}
/// Opaque in-process keyset hint; never authority to mutate persisted state.
#[derive(Clone, PartialEq, Eq)]
pub struct CleanupCursor {
    deadline: NaiveDateTime,
    auth_id: Uuid,
    request_id: Option<Uuid>,
}
/// Metadata-only discovery snapshot. No encrypted payload or user identity is scanned.
#[derive(Clone)]
pub struct CleanupCandidate {
    queue: CleanupQueue,
    cursor: CleanupCursor,
    parent: Option<ParentFence>,
    child: Option<ChildFence>,
}
#[derive(Clone, PartialEq, Eq)]
struct ParentFence {
    status: String,
    version: u16,
    state: Option<Vec<u8>>,
    broker: Option<Vec<u8>>,
    generation: Option<u64>,
    login: Option<NaiveDateTime>,
    auth: Option<NaiveDateTime>,
}
impl ParentFence {
    fn capture(row: &Session) -> Self {
        Self {
            status: row.status.clone(),
            version: row.hmac_key_version,
            state: row.oauth_state_hash.map(Vec::from),
            broker: row.broker_hash.map(Vec::from),
            generation: row.credential_generation,
            login: row.login_expires_at,
            auth: row.auth_expires_at,
        }
    }
}
#[derive(Clone, PartialEq, Eq)]
struct ChildFence {
    status: String,
    hash: Vec<u8>,
    version: u16,
    generation: u64,
    deadline: NaiveDateTime,
}
impl ChildFence {
    fn capture(row: &RefreshRequest) -> Self {
        Self {
            status: row.status.clone(),
            hash: row.request_hash.to_vec(),
            version: row.hmac_key_version,
            generation: row.input_generation,
            deadline: row.expires_at,
        }
    }
}
impl CleanupCandidate {
    /// Copy the stable position for a later indexed keyset scan.
    pub fn cursor(&self) -> CleanupCursor {
        self.cursor.clone()
    }
}
/// Committed business rows and contention measurements for one short transaction.
#[derive(Default, Debug)]
pub struct CleanupOutcome {
    pub affected_rows: u64,
    pub lock_skips: u64,
}
impl CleanupOutcome {
    fn changed(affected_rows: u64) -> Self {
        Self {
            affected_rows,
            lock_skips: 0,
        }
    }
    fn skipped() -> Self {
        Self {
            affected_rows: 0,
            lock_skips: 1,
        }
    }
}
/// Closed cleanup store owning its own dedicated validated writer pool.
pub struct CleanupStore {
    store: Store,
}
impl CleanupStore {
    /// Permanently close this cleanup store's dedicated pool; new operations fail immediately.
    /// Await release/closure of existing connections. Idempotent across calls; cannot fail.
    pub fn close(&self) -> impl std::future::Future<Output = ()> + '_ {
        self.store.close()
    }
    pub(crate) fn matches_crypto(&self, crypto: &Crypto) -> bool {
        self.store.matches_crypto(crypto)
    }
    /// Bind the dedicated pool to app and cryptographic validators; invalid identity fails.
    pub fn new(
        database: CleanupDatabase,
        app: &str,
        crypto: Arc<Crypto>,
    ) -> Result<Self, StorageError> {
        Ok(Self {
            store: Store::new(
                RuntimeDatabase {
                    pool: database.pool,
                },
                app,
                crypto,
            )?,
        })
    }
    /// Read the writer's millisecond UTC cutoff; never trusts process wall time.
    pub async fn cutoff(&self) -> Result<NaiveDateTime, StorageError> {
        bounded(async {
            Ok(sqlx::query_scalar("SELECT UTC_TIMESTAMP(3)")
                .fetch_one(&self.store.database.pool)
                .await?)
        })
        .await
    }
    /// Discover at most 500 due references using the existing deadline index.
    /// Invalid batch limits fail before I/O; SQL/database failures are sanitized.
    pub async fn discover(
        &self,
        queue: CleanupQueue,
        cutoff: NaiveDateTime,
        cursor: Option<&CleanupCursor>,
        limit: u16,
    ) -> Result<Vec<CleanupCandidate>, StorageError> {
        if !(1..=500).contains(&limit) {
            return Err(StorageError::Invalid);
        }
        bounded(self.scan(queue, cutoff, cursor, limit)).await
    }
    async fn scan(
        &self,
        queue: CleanupQueue,
        cutoff: NaiveDateTime,
        cursor: Option<&CleanupCursor>,
        limit: u16,
    ) -> Result<Vec<CleanupCandidate>, StorageError> {
        let mut query = discovery_sql(queue, cutoff, false);
        if let Some(cursor) = cursor {
            append_cursor(&mut query, queue, cursor)?;
        }
        query
            .push(" ORDER BY ")
            .push(order_by(queue))
            .push(" LIMIT ")
            .push_bind(limit);
        let found = query.build().fetch_all(&self.store.database.pool).await?;
        found
            .into_iter()
            .map(|row| decode_candidate(row, queue))
            .collect()
    }

    /// Measure the oldest overdue index-head age with LIMIT 1, without counting rows.
    /// Database errors are sanitized and no candidate identity is exposed in the metric.
    pub async fn oldest_overdue(
        &self,
        queue: CleanupQueue,
        cutoff: NaiveDateTime,
    ) -> Result<Option<StdDuration>, StorageError> {
        let found = self.discover(queue, cutoff, None, 1).await?;
        Ok(found.first().map(|candidate| {
            (cutoff - candidate.cursor.deadline)
                .to_std()
                .unwrap_or_default()
        }))
    }
    /// Recheck one reference under parent-first locks and a <=1 second total deadline.
    /// Returns committed affected rows; stale/locked candidates make no writes.
    pub async fn process(
        &self,
        candidate: &CleanupCandidate,
        deadline: StdDuration,
    ) -> Result<CleanupOutcome, StorageError> {
        if deadline.is_zero() || deadline > StdDuration::from_secs(1) {
            return Err(StorageError::Invalid);
        }
        tokio::time::timeout(deadline, self.process_inner(candidate))
            .await
            .map_err(|_| StorageError::Timeout)?
    }
    async fn process_inner(
        &self,
        candidate: &CleanupCandidate,
    ) -> Result<CleanupOutcome, StorageError> {
        let mut connection = CleanupConnection::acquire(&self.store.database.pool).await?;
        let mut transaction = connection.writer()?.begin().await?;
        let outcome = self.process_locked(&mut transaction, candidate).await?;
        transaction.commit().await?;
        connection.is_reusable = true;
        Ok(outcome)
    }
    async fn process_locked(
        &self,
        transaction: &mut Transaction<'_, MySql>,
        candidate: &CleanupCandidate,
    ) -> Result<CleanupOutcome, StorageError> {
        let parent =
            sqlx::query("SELECT * FROM auth_sessions WHERE auth_id=? FOR UPDATE SKIP LOCKED")
                .bind(candidate.cursor.auth_id.as_bytes().as_slice())
                .fetch_optional(&mut **transaction)
                .await?
                .map(rows::session)
                .transpose()?;
        let Some(parent) = parent else {
            return self.missing_parent(transaction, candidate).await;
        };
        self.store.validate_session(&parent)?;
        if candidate.parent.as_ref() != Some(&ParentFence::capture(&parent)) {
            return Ok(CleanupOutcome::default());
        }
        if let Some(request_id) = candidate.cursor.request_id {
            self.process_child(transaction, &parent, (request_id, candidate.child.as_ref()))
                .await
        } else {
            let now = db_now(transaction).await?;
            self.process_parent(transaction, parent, (candidate.queue, now))
                .await
        }
    }
    async fn missing_parent(
        &self,
        transaction: &mut Transaction<'_, MySql>,
        candidate: &CleanupCandidate,
    ) -> Result<CleanupOutcome, StorageError> {
        let has_parent: i64 =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM auth_sessions WHERE auth_id=?)")
                .bind(candidate.cursor.auth_id.as_bytes().as_slice())
                .fetch_one(&mut **transaction)
                .await?;
        if has_parent == 0 && candidate.cursor.request_id.is_some() {
            let has_child: i64 = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM auth_refresh_requests WHERE auth_id=? AND request_id=?)")
                .bind(candidate.cursor.auth_id.as_bytes().as_slice()).bind(candidate.cursor.request_id.map(|id| id.as_bytes().to_vec())).fetch_one(&mut **transaction).await?;
            if has_child != 0 {
                return Err(StorageError::Integrity);
            }
        }
        Ok(CleanupOutcome {
            affected_rows: 0,
            lock_skips: 1,
        })
    }
    async fn process_child(
        &self,
        transaction: &mut Transaction<'_, MySql>,
        parent: &Session,
        operation: (Uuid, Option<&ChildFence>),
    ) -> Result<CleanupOutcome, StorageError> {
        let Some(child) = skip_child(transaction, parent.auth_id, operation.0).await? else {
            return Ok(CleanupOutcome::skipped());
        };
        self.store.validate_pair(parent, &child)?;
        if operation.1 != Some(&ChildFence::capture(&child)) {
            return Ok(CleanupOutcome::default());
        }
        let now = db_now(transaction).await?;
        if child.expires_at > now {
            return Ok(CleanupOutcome::default());
        }
        if child.status != "pending" {
            self.store
                .delete_refresh(transaction, parent.clone(), child, now)
                .await?;
            return Ok(CleanupOutcome::changed(1));
        }
        if ended(parent) {
            self.store
                .settle_ended(transaction, parent.clone(), child, now)
                .await?;
            return Ok(CleanupOutcome::changed(1));
        }
        // [Review Fix #40] Cleanup explicitly requires the original expired lease after both locks.
        let fence = RefreshFence::capture(parent, &child)?;
        if fence.deadline > now || !fence.matches(parent, &child) {
            return Ok(CleanupOutcome::default());
        }
        let mut next = parent.clone();
        next.status = "reauth_required".into();
        next.reason_code = Some("refresh_outcome_unknown".into());
        next.clear_login();
        let mut unknown = child.clone();
        unknown.status = "unknown".into();
        unknown.error_code = Some("refresh_outcome_unknown".into());
        self.store
            .finish_refresh(
                transaction,
                (parent.clone(), next),
                (child, unknown),
                (fence, false),
            )
            .await?;
        Ok(CleanupOutcome::changed(2))
    }
    async fn process_parent(
        &self,
        transaction: &mut Transaction<'_, MySql>,
        parent: Session,
        operation: (CleanupQueue, NaiveDateTime),
    ) -> Result<CleanupOutcome, StorageError> {
        let (queue, now) = operation;
        let is_auth_due = parent
            .auth_expires_at
            .is_some_and(|deadline| deadline <= now);
        let is_login_due = parent
            .login_expires_at
            .is_some_and(|deadline| deadline <= now);
        if !(is_auth_due || queue == CleanupQueue::Login && is_login_due) {
            return Ok(CleanupOutcome::default());
        }
        if parent.credential_generation.is_none() || parent.status == "ready" {
            return self.expire_unclaimed(transaction, parent, now).await;
        }
        if is_auth_due {
            if !ended(&parent) {
                let mut next = parent.clone();
                next.status = "expired".into();
                next.reason_code = Some("authorization_expired".into());
                next.auth_expires_at = parent.auth_expires_at.map(|deadline| deadline.min(now));
                next.clear_login();
                self.store.update(transaction, &parent, &next).await?;
                return Ok(CleanupOutcome::changed(1));
            }
            return self.collect_ended(transaction, parent, now).await;
        }
        if is_login_due && matches!(parent.status.as_str(), "active" | "refreshing") {
            self.store.clear_login(transaction, parent, now).await?;
            return Ok(CleanupOutcome::changed(1));
        }
        Ok(CleanupOutcome::default())
    }
    async fn expire_unclaimed(
        &self,
        transaction: &mut Transaction<'_, MySql>,
        parent: Session,
        now: NaiveDateTime,
    ) -> Result<CleanupOutcome, StorageError> {
        if has_children(transaction, parent.auth_id).await? {
            return Err(StorageError::Integrity);
        }
        let mut next = parent.clone();
        let mut count = 0;
        if matches!(parent.status.as_str(), "pending" | "exchanging" | "ready") {
            next.status = "expired".into();
            next.reason_code = Some(
                if parent.status == "ready" {
                    "authorization_expired"
                } else {
                    "login_expired"
                }
                .into(),
            );
            if parent.credential_generation.is_some() {
                next.auth_expires_at = parent.auth_expires_at.map(|deadline| deadline.min(now));
                next.clear_login();
            } else {
                next.encrypted_oauth_context = None;
            }
            self.store.update(transaction, &parent, &next).await?;
            count = 1;
        }
        self.store.delete_session(transaction, next, now).await?;
        Ok(CleanupOutcome::changed(count + 1))
    }
    async fn collect_ended(
        &self,
        transaction: &mut Transaction<'_, MySql>,
        parent: Session,
        now: NaiveDateTime,
    ) -> Result<CleanupOutcome, StorageError> {
        let request_id: Option<Vec<u8>> = sqlx::query_scalar("SELECT request_id FROM auth_refresh_requests WHERE auth_id=? ORDER BY request_id LIMIT 1")
            .bind(parent.auth_id.as_bytes().as_slice()).fetch_optional(&mut **transaction).await?;
        let Some(request_id) = request_id else {
            self.store.delete_session(transaction, parent, now).await?;
            return Ok(CleanupOutcome::changed(1));
        };
        let request_id = Uuid::from_slice(&request_id).map_err(|_| StorageError::Integrity)?;
        let Some(child) = skip_child(transaction, parent.auth_id, request_id).await? else {
            return Ok(CleanupOutcome::skipped());
        };
        self.store.validate_pair(&parent, &child)?;
        if child.status == "pending" {
            self.store
                .settle_ended(transaction, parent, child, now)
                .await?;
            return Ok(CleanupOutcome::changed(1));
        }
        if child.expires_at <= now {
            self.store
                .delete_refresh(transaction, parent, child, now)
                .await?;
            return Ok(CleanupOutcome::changed(1));
        }
        Ok(CleanupOutcome::default())
    }
}
struct CleanupConnection {
    connection: Option<sqlx::pool::PoolConnection<MySql>>,
    is_reusable: bool,
}
impl CleanupConnection {
    async fn acquire(pool: &sqlx::MySqlPool) -> Result<Self, StorageError> {
        Ok(Self {
            connection: Some(pool.acquire().await?),
            is_reusable: false,
        })
    }
    fn writer(&mut self) -> Result<&mut sqlx::MySqlConnection, StorageError> {
        self.connection
            .as_deref_mut()
            .ok_or(StorageError::Unavailable)
    }
}
impl Drop for CleanupConnection {
    fn drop(&mut self) {
        // [Review Fix #43] Cancellation/unknown commit closes the socket; only acknowledged commits reuse it.
        if !self.is_reusable {
            if let Some(connection) = self.connection.take() {
                drop(connection.detach());
            }
        }
    }
}
async fn skip_child(
    transaction: &mut Transaction<'_, MySql>,
    auth_id: Uuid,
    request_id: Uuid,
) -> Result<Option<RefreshRequest>, StorageError> {
    sqlx::query("SELECT * FROM auth_refresh_requests WHERE auth_id=? AND request_id=? FOR UPDATE SKIP LOCKED")
        .bind(auth_id.as_bytes().as_slice()).bind(request_id.as_bytes().as_slice()).fetch_optional(&mut **transaction).await?.map(rows::refresh).transpose()
}
fn decode_candidate(
    row: sqlx::mysql::MySqlRow,
    queue: CleanupQueue,
) -> Result<CleanupCandidate, StorageError> {
    let auth_id = Uuid::from_slice(&row.try_get::<Vec<u8>, _>("auth_id")?)
        .map_err(|_| StorageError::Integrity)?;
    let parent = decode_parent(&row)?;
    let is_child = !queue.status().is_empty();
    let request_id = if is_child {
        Some(
            Uuid::from_slice(&row.try_get::<Vec<u8>, _>("request_id")?)
                .map_err(|_| StorageError::Integrity)?,
        )
    } else {
        None
    };
    let deadline = match queue {
        CleanupQueue::Login => row.try_get("login_expires_at")?,
        CleanupQueue::Authorization => row.try_get("auth_expires_at")?,
        _ => row.try_get("expires_at")?,
    };
    let child = if is_child {
        Some(ChildFence {
            status: row.try_get_unchecked("r_status")?,
            hash: row.try_get("request_hash")?,
            version: row.try_get("r_version")?,
            generation: row.try_get("input_generation")?,
            deadline,
        })
    } else {
        None
    };
    Ok(CleanupCandidate {
        queue,
        cursor: CleanupCursor {
            deadline,
            auth_id,
            request_id,
        },
        parent,
        child,
    })
}

fn decode_parent(row: &sqlx::mysql::MySqlRow) -> Result<Option<ParentFence>, StorageError> {
    let status: Option<String> = row.try_get_unchecked("a_status")?;
    status
        .map(|status| {
            Ok::<_, StorageError>(ParentFence {
                status,
                version: row.try_get("a_version")?,
                state: row.try_get("oauth_state_hash")?,
                broker: row.try_get("broker_hash")?,
                generation: row.try_get("credential_generation")?,
                login: row.try_get("login_expires_at")?,
                auth: row.try_get("auth_expires_at")?,
            })
        })
        .transpose()
}
fn order_by(queue: CleanupQueue) -> &'static str {
    match queue {
        CleanupQueue::Login => "a.login_expires_at,a.auth_id",
        CleanupQueue::Authorization => "a.auth_expires_at,a.auth_id",
        _ => "r.expires_at,r.auth_id,r.request_id",
    }
}
fn discovery_sql(
    queue: CleanupQueue,
    cutoff: NaiveDateTime,
    is_explain: bool,
) -> sqlx::QueryBuilder<MySql> {
    let mut query = sqlx::QueryBuilder::new(if is_explain {
        "EXPLAIN FORMAT=JSON SELECT "
    } else {
        "SELECT "
    });
    if queue.status().is_empty() {
        query.push("a.auth_id,");
    } else {
        query.push("r.auth_id,r.request_id,r.status AS r_status,r.request_hash,r.hmac_key_version AS r_version,r.input_generation,r.expires_at,");
    }
    query.push("a.status AS a_status,a.hmac_key_version AS a_version,a.oauth_state_hash,a.broker_hash,a.credential_generation,a.login_expires_at,a.auth_expires_at FROM ");
    match queue {
        CleanupQueue::Login => {
            query.push(
                "auth_sessions a FORCE INDEX (ix_session_login_expiry) WHERE a.login_expires_at<=",
            );
        }
        CleanupQueue::Authorization => {
            query.push(
                "auth_sessions a FORCE INDEX (ix_session_auth_expiry) WHERE a.auth_expires_at<=",
            );
        }
        _ => {
            query.push("auth_refresh_requests r FORCE INDEX (ix_refresh_expiry) LEFT JOIN auth_sessions a ON a.auth_id=r.auth_id WHERE r.expires_at<=");
        }
    }
    query.push_bind(cutoff);
    if !queue.status().is_empty() {
        query.push(" AND r.status=").push_bind(queue.status());
    }
    query
}
fn append_cursor(
    query: &mut sqlx::QueryBuilder<MySql>,
    queue: CleanupQueue,
    cursor: &CleanupCursor,
) -> Result<(), StorageError> {
    let deadline = match queue {
        CleanupQueue::Login => "a.login_expires_at",
        CleanupQueue::Authorization => "a.auth_expires_at",
        _ => "r.expires_at",
    };
    let id = if queue.status().is_empty() {
        "a.auth_id"
    } else {
        "r.auth_id"
    };
    query
        .push(" AND (")
        .push(deadline)
        .push(">")
        .push_bind(cursor.deadline)
        .push(" OR (")
        .push(deadline)
        .push("=")
        .push_bind(cursor.deadline)
        .push(" AND (")
        .push(id)
        .push(">")
        .push_bind(cursor.auth_id.as_bytes().to_vec());
    if !queue.status().is_empty() {
        query
            .push(" OR (r.auth_id=")
            .push_bind(cursor.auth_id.as_bytes().to_vec())
            .push(" AND r.request_id>")
            .push_bind(
                cursor
                    .request_id
                    .ok_or(StorageError::Invalid)?
                    .as_bytes()
                    .to_vec(),
            )
            .push(")");
    }
    query.push(")))");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{DatabaseConfig, Tls};
    #[tokio::test]
    #[ignore = "requires explicit isolated cleanup MySQL URLs"]
    async fn actual_cleanup_queries_use_existing_indexes_and_separate_pool_settings() {
        let url = std::env::var("ASK_AUTH_TEST_DATABASE_URL").unwrap();
        let database = CleanupDatabase::connect(
            DatabaseConfig::new(url.clone(), Tls::LoopbackTlsTestOnly, 1).unwrap(),
        )
        .await
        .unwrap();
        assert_pool_settings(&database.pool, "READ-COMMITTED", 1).await;
        let cutoff: NaiveDateTime = sqlx::query_scalar("SELECT UTC_TIMESTAMP(3)")
            .fetch_one(&database.pool)
            .await
            .unwrap();
        for queue in CleanupQueue::ALL {
            let mut query = discovery_sql(queue, cutoff, true);
            query
                .push(" ORDER BY ")
                .push(order_by(queue))
                .push(" LIMIT ")
                .push_bind(100u16);
            let plan: String = query
                .build_query_scalar()
                .fetch_one(&database.pool)
                .await
                .unwrap();
            let expected = match queue {
                CleanupQueue::Login => "ix_session_login_expiry",
                CleanupQueue::Authorization => "ix_session_auth_expiry",
                _ => "ix_refresh_expiry",
            };
            assert!(
                plan.contains(expected),
                "existing deadline index absent from actual plan"
            );
            assert!(
                !plan.contains("\"access_type\": \"ALL\""),
                "full table scan in actual plan"
            );
        }
        let api = RuntimeDatabase::connect(
            DatabaseConfig::new(url, Tls::LoopbackTlsTestOnly, 1).unwrap(),
        )
        .await
        .unwrap();
        assert_pool_settings(&api.pool, "REPEATABLE-READ", 2).await;
    }
    async fn assert_pool_settings(
        pool: &sqlx::MySqlPool,
        expected_isolation: &str,
        expected_timeout: i64,
    ) {
        let name: String = sqlx::query_scalar("SELECT DATABASE()")
            .fetch_one(pool)
            .await
            .unwrap();
        assert_eq!(name, "ask_auth_stage4_cleanup_test");
        let isolation: String = sqlx::query_scalar("SELECT @@transaction_isolation")
            .fetch_one(pool)
            .await
            .unwrap();
        assert_eq!(isolation, expected_isolation);
        let timeout: i64 = sqlx::query_scalar("SELECT CAST(@@innodb_lock_wait_timeout AS SIGNED)")
            .fetch_one(pool)
            .await
            .unwrap();
        assert_eq!(timeout, expected_timeout);
        let timezone: String = sqlx::query_scalar("SELECT @@time_zone")
            .fetch_one(pool)
            .await
            .unwrap();
        assert_eq!(timezone, "+00:00");
    }
}
