pub(super) mod cleanup;
mod lifecycle;
use super::{mysql::bounded, rows, RefreshRequest, RuntimeDatabase, Session, StorageError};
use crate::crypto::{Binding, Crypto};
use crate::validation::is_uuid_v7;
use chrono::{Duration, NaiveDateTime};
use sqlx::{MySql, Transaction};
use std::sync::Arc;
use uuid::Uuid;

/// Original callback operation, captured before external I/O; never replace with a newer row.
#[derive(Clone)]
pub struct CallbackFence {
    auth_id: Uuid,
    state: [u8; 32],
    version: u16,
    deadline: NaiveDateTime,
}
impl CallbackFence {
    /// Capture original state/version/deadline from a preauthorization row; missing fields fail.
    pub fn capture(row: &Session) -> Result<Self, StorageError> {
        if !matches!(row.status.as_str(), "pending" | "exchanging" | "ready") {
            return Err(StorageError::Invalid);
        }
        Ok(Self {
            auth_id: row.auth_id,
            state: row.oauth_state_hash.ok_or(StorageError::Invalid)?,
            version: row.hmac_key_version,
            deadline: row.login_expires_at.ok_or(StorageError::Invalid)?,
        })
    }
    fn matches(&self, row: &Session) -> bool {
        // [Review Fix #14] Resource identity belongs to the captured operation, even if other fields collide.
        row.auth_id == self.auth_id
            && row.oauth_state_hash == Some(self.state)
            && row.hmac_key_version == self.version
            && row.login_expires_at == Some(self.deadline)
    }
}
/// Original refresh operation: A broker/key/generation and R request/key/input/lease.
#[derive(Clone)]
pub struct RefreshFence {
    auth_id: Uuid,
    request_id: Uuid,
    broker: [u8; 32],
    version: u16,
    request: [u8; 32],
    request_version: u16,
    generation: u64,
    deadline: NaiveDateTime,
}
impl RefreshFence {
    /// Capture an already-validated pending refresh pair. Invalid phase or generation fails.
    pub fn capture(session: &Session, request: &RefreshRequest) -> Result<Self, StorageError> {
        request.validate()?;
        if session.status != "refreshing"
            || request.status != "pending"
            || session.auth_id != request.auth_id
            || session.credential_generation != Some(request.input_generation)
        {
            return Err(StorageError::Invalid);
        }
        Ok(Self {
            auth_id: session.auth_id,
            request_id: request.request_id,
            broker: session.broker_hash.ok_or(StorageError::Invalid)?,
            version: session.hmac_key_version,
            request: request.request_hash,
            request_version: request.hmac_key_version,
            generation: request.input_generation,
            deadline: request.expires_at,
        })
    }
    fn matches(&self, session: &Session, request: &RefreshRequest) -> bool {
        // [Review Fix #14] An old fence can never be rebound to a new auth/request ID.
        session.auth_id == self.auth_id
            && request.auth_id == self.auth_id
            && request.request_id == self.request_id
            && session.status == "refreshing"
            && request.status == "pending"
            && session.broker_hash == Some(self.broker)
            && session.hmac_key_version == self.version
            && session.credential_generation == Some(self.generation)
            && request.request_hash == self.request
            && request.hmac_key_version == self.request_version
            && request.input_generation == self.generation
            && request.expires_at == self.deadline
    }
}
/// Closed set of guarded mutations. No raw transaction/SQL/closure API is exposed.
pub enum Mutation {
    Callback {
        expected: Session,
        next: Session,
        fence: CallbackFence,
    },
    Claim {
        expected: Session,
        next: Session,
    },
    ExpireLogin {
        expected: Session,
        fence: CallbackFence,
    },
    SettleEndedRefresh {
        expected: Session,
        request: RefreshRequest,
    },
    ClearLogin {
        expected: Session,
    },
    StartRefresh {
        expected: Session,
        request: RefreshRequest,
    },
    FinishRefresh {
        expected: Session,
        next: Box<Session>,
        request: RefreshRequest,
        next_request: RefreshRequest,
        fence: RefreshFence,
    },
    EndAuthorization {
        expected: Session,
        next: Session,
    },
    DeleteRefresh {
        expected: Session,
        request: RefreshRequest,
    },
    DeleteSession {
        expected: Session,
    },
}
impl Mutation {
    fn expected(&self) -> &Session {
        match self {
            Self::Callback { expected, .. }
            | Self::Claim { expected, .. }
            | Self::ExpireLogin { expected, .. }
            | Self::SettleEndedRefresh { expected, .. }
            | Self::ClearLogin { expected }
            | Self::StartRefresh { expected, .. }
            | Self::FinishRefresh { expected, .. }
            | Self::EndAuthorization { expected, .. }
            | Self::DeleteRefresh { expected, .. }
            | Self::DeleteSession { expected } => expected,
        }
    }
}
/// Validating writer store. Transactions live only inside bounded methods and auto-rollback on failure.
pub struct Store {
    database: RuntimeDatabase,
    app: String,
    crypto: Arc<Crypto>,
}
impl Store {
    /// Permanently close this store's exact writer pool, including external handles to this store.
    /// New operations fail after this call; await existing connections closing. Idempotent, cannot fail.
    pub fn close(&self) -> impl std::future::Future<Output = ()> + '_ {
        self.database.close()
    }
    /// Bind a runtime database to immutable app identity; empty/oversize app fails.
    pub fn new(
        database: RuntimeDatabase,
        app: &str,
        crypto: Arc<Crypto>,
    ) -> Result<Self, StorageError> {
        if app.is_empty() || app.chars().count() > 256 || crypto.app() != app {
            return Err(StorageError::Invalid);
        }
        Ok(Self {
            database,
            app: app.into(),
            crypto,
        })
    }
    /// Create pending with DB now + ten minutes; supplied login deadline is ignored.
    /// Returns the stored snapshot for fences; duplicate IDs never overwrite or renew the lease.
    pub async fn create_pending(&self, mut row: Session) -> Result<Session, StorageError> {
        bounded(async {
            if row.status != "pending" {
                return Err(StorageError::Invalid);
            }
            let mut transaction = self.database.pool.begin().await?;
            let now = db_now(&mut transaction).await?;
            // [Review Fix #12] Creation must not depend on caller wall-clock accuracy.
            row.login_expires_at = Some(now + Duration::minutes(10));
            self.validate_session(&row)?;
            let found = lock_session(&mut transaction, row.auth_id).await?;
            if found.is_some() {
                return Err(StorageError::Conflict);
            }
            // [Review Fix #9] Reusing a missing parent ID must not silently adopt orphan refresh evidence.
            if has_children(&mut transaction, row.auth_id).await? {
                return Err(StorageError::Integrity);
            }
            affected(
                rows::bind_session(sqlx::query(rows::INSERT_SESSION), &row)
                    .execute(&mut *transaction)
                    .await?
                    .rows_affected(),
            )?;
            transaction.commit().await?;
            Ok(row)
        })
        .await
    }
    /// Read A by RFC-binary primary key from writer and validate complete shape. No row returns None.
    pub async fn session(&self, auth_id: Uuid) -> Result<Option<Session>, StorageError> {
        bounded(async {
            if !is_uuid_v7(auth_id) {
                return Err(StorageError::Invalid);
            }
            let mut transaction = self.database.pool.begin().await?;
            let row = lock_session(&mut transaction, auth_id).await?;
            if let Some(ref session) = row {
                self.validate_session(session)?;
            }
            transaction.commit().await?;
            Ok(row)
        })
        .await
    }
    pub(crate) fn matches_crypto(&self, crypto: &Crypto) -> bool {
        std::ptr::eq(self.crypto.as_ref(), crypto)
    }
    pub(crate) async fn login_snapshot(
        &self,
        auth_id: Uuid,
    ) -> Result<(Option<Session>, NaiveDateTime), StorageError> {
        bounded(async {
            if !is_uuid_v7(auth_id) {
                return Err(StorageError::Invalid);
            }
            let mut transaction = self.database.pool.begin().await?;
            let row = lock_session(&mut transaction, auth_id).await?;
            if let Some(ref session) = row {
                // [Review Fix #30] Authenticate supplied proofs before inspecting encrypted business payloads.
                session.validate(&self.app)?;
                if !self.crypto.has_version(session.hmac_key_version) {
                    return Err(StorageError::Integrity);
                }
            }
            let now = db_now(&mut transaction).await?;
            transaction.commit().await?;
            Ok((row, now))
        })
        .await
    }
    // Closed credential-release confirmation: final row lock rejects concurrent refresh/revoke/cleanup.
    pub(crate) async fn confirm_login(
        &self,
        expected: &Session,
        credential_deadline: NaiveDateTime,
    ) -> Result<(), StorageError> {
        bounded(async {
            let mut transaction = self.database.pool.begin().await?;
            let current = lock_session(&mut transaction, expected.auth_id)
                .await?
                .ok_or(StorageError::NotFound)?;
            self.validate_session(&current)?;
            let now = db_now(&mut transaction).await?;
            if current != *expected
                || current.status != "active"
                || current.credential_generation != Some(1)
                || current
                    .login_expires_at
                    .is_none_or(|deadline| deadline <= now)
                || current
                    .auth_expires_at
                    .is_none_or(|deadline| deadline <= now)
                || credential_deadline <= now
            {
                return Err(StorageError::Conflict);
            }
            transaction.commit().await.map_err(Into::into)
        })
        .await
    }
    /// Read a consistent A->exact R pair and validate their relationship; orphan R fails closed.
    pub async fn refresh(
        &self,
        auth_id: Uuid,
        request_id: Uuid,
    ) -> Result<Option<(Session, RefreshRequest)>, StorageError> {
        bounded(async {
            // [Review Fix #21] Reject legacy operation IDs before any database lookup.
            if !is_uuid_v7(auth_id) || !is_uuid_v7(request_id) {
                return Err(StorageError::Invalid);
            }
            let mut transaction = self.database.pool.begin().await?;
            let parent = lock_session(&mut transaction, auth_id).await?;
            // [Review Fix #16] A missing request cannot hide corruption in its already-read parent.
            if let Some(ref parent) = parent {
                self.validate_session(parent)?;
            }
            let child = lock_refresh(&mut transaction, auth_id, request_id).await?;
            let pair = match (parent, child) {
                (Some(parent), Some(child)) => {
                    self.validate_pair(&parent, &child)?;
                    Some((parent, child))
                }
                (None, Some(_)) => return Err(StorageError::Integrity),
                _ => None,
            };
            transaction.commit().await?;
            Ok(pair)
        })
        .await
    }
    /// Apply one explicit mutation under A->R locks and a three-second total deadline.
    /// Snapshot/fence mismatch makes no writes; all errors drop/rollback the transaction.
    pub async fn apply(&self, mutation: Mutation) -> Result<(), StorageError> {
        bounded(async {
            let expected = mutation.expected();
            self.validate_session(expected)?;
            let mut transaction = self.database.pool.begin().await?;
            let current = lock_session(&mut transaction, expected.auth_id)
                .await?
                .ok_or(StorageError::NotFound)?;
            self.validate_session(&current)?;
            if &current != expected {
                return Err(StorageError::Conflict);
            }
            let now = db_now(&mut transaction).await?;
            self.mutate(&mut transaction, mutation, now).await?;
            transaction.commit().await.map_err(Into::into)
        })
        .await
    }
    async fn mutate(
        &self,
        transaction: &mut Transaction<'_, MySql>,
        mutation: Mutation,
        now: NaiveDateTime,
    ) -> Result<(), StorageError> {
        match mutation {
            Mutation::Callback {
                expected,
                next,
                fence,
            } => {
                self.callback(transaction, (expected, next), fence, now)
                    .await
            }
            Mutation::Claim { expected, next } => {
                self.claim(transaction, expected, next, now).await
            }
            Mutation::ClearLogin { expected } => self.clear_login(transaction, expected, now).await,
            Mutation::ExpireLogin { expected, fence } => {
                self.expire_login(transaction, expected, fence, now).await
            }
            Mutation::EndAuthorization { expected, mut next } => {
                if expected.credential_generation.is_none() || !ended(&next) {
                    return Err(StorageError::Conflict);
                }
                next.auth_expires_at = expected.auth_expires_at.map(|deadline| deadline.min(now));
                self.update(transaction, &expected, &next).await
            }
            other => self.mutate_refresh(transaction, other, now).await,
        }
    }
    async fn mutate_refresh(
        &self,
        transaction: &mut Transaction<'_, MySql>,
        mutation: Mutation,
        now: NaiveDateTime,
    ) -> Result<(), StorageError> {
        match mutation {
            Mutation::StartRefresh { expected, request } => {
                self.start_refresh(transaction, expected, request, now)
                    .await
            }
            Mutation::FinishRefresh {
                expected,
                next,
                request,
                next_request,
                fence,
            } => {
                self.finish_refresh(
                    transaction,
                    (expected, *next),
                    (request, next_request),
                    (fence, false),
                )
                .await
            }
            Mutation::SettleEndedRefresh { expected, request } => {
                self.settle_ended(transaction, expected, request, now).await
            }
            Mutation::DeleteRefresh { expected, request } => {
                self.delete_refresh(transaction, expected, request, now)
                    .await
            }
            Mutation::DeleteSession { expected } => {
                self.delete_session(transaction, expected, now).await
            }
            _ => Err(StorageError::Invalid),
        }
    }
    async fn callback(
        &self,
        transaction: &mut Transaction<'_, MySql>,
        rows: (Session, Session),
        fence: CallbackFence,
        now: NaiveDateTime,
    ) -> Result<(), StorageError> {
        let (expected, mut next) = rows;
        // [Review Fix #8] Every callback reserves pending -> exchanging before publishing any outcome.
        let is_legal = matches!(
            (expected.status.as_str(), next.status.as_str()),
            ("pending", "exchanging") | ("exchanging", "ready" | "denied" | "error")
        );
        if !fence.matches(&expected) || fence.deadline <= now || !is_legal {
            return Err(StorageError::Conflict);
        }
        if matches!(next.status.as_str(), "denied" | "error") {
            next.login_expires_at = expected.login_expires_at.map(|deadline| deadline.min(now));
        }
        if next.status == "ready" {
            within(
                next.auth_expires_at.ok_or(StorageError::Invalid)?,
                now,
                Duration::days(30),
            )?;
        }
        self.update(transaction, &expected, &next).await
    }
    async fn claim(
        &self,
        transaction: &mut Transaction<'_, MySql>,
        expected: Session,
        next: Session,
        now: NaiveDateTime,
    ) -> Result<(), StorageError> {
        if expected.status != "ready"
            || next.status != "active"
            || expected
                .login_expires_at
                .is_none_or(|deadline| deadline <= now)
            || expected
                .auth_expires_at
                .is_none_or(|deadline| deadline <= now)
        {
            return Err(StorageError::Conflict);
        }
        within(
            next.login_expires_at.ok_or(StorageError::Invalid)?,
            now,
            Duration::minutes(5),
        )?;
        self.update(transaction, &expected, &next).await
    }
    async fn clear_login(
        &self,
        transaction: &mut Transaction<'_, MySql>,
        expected: Session,
        now: NaiveDateTime,
    ) -> Result<(), StorageError> {
        if !matches!(expected.status.as_str(), "active" | "refreshing")
            || expected
                .login_expires_at
                .is_none_or(|deadline| deadline > now)
        {
            return Err(StorageError::Conflict);
        }
        let mut next = expected.clone();
        next.clear_login();
        self.update(transaction, &expected, &next).await
    }
    async fn update(
        &self,
        transaction: &mut Transaction<'_, MySql>,
        current: &Session,
        next: &Session,
    ) -> Result<(), StorageError> {
        current.validate_transition(next, &self.app)?;
        self.validate_session(next)?;
        affected(
            rows::bind_session(sqlx::query(rows::UPDATE_SESSION), next)
                .execute(&mut **transaction)
                .await?
                .rows_affected(),
        )
    }
    // [Review Fix #4] Length alone is not authenticated storage: reject unknown keys and invalid AEAD/AAD.
    fn validate_session(&self, session: &Session) -> Result<(), StorageError> {
        session.validate(&self.app)?;
        if !self.crypto.has_version(session.hmac_key_version) {
            return Err(StorageError::Integrity);
        }
        if let Some(ref envelope) = session.encrypted_oauth_context {
            self.crypto
                .decrypt(&Binding::context(session.auth_id), envelope)
                .map_err(|_| StorageError::Integrity)?;
        }
        if let Some(ref envelope) = session.encrypted_result {
            self.crypto
                .decrypt(&Binding::login(session.auth_id), envelope)
                .map_err(|_| StorageError::Integrity)?;
        }
        Ok(())
    }
    fn validate_pair(
        &self,
        session: &Session,
        request: &RefreshRequest,
    ) -> Result<(), StorageError> {
        self.validate_session(session)?;
        request.validate()?;
        if !self.crypto.has_version(request.hmac_key_version) {
            return Err(StorageError::Integrity);
        }
        if let Some(ref envelope) = request.encrypted_response {
            let binding = Binding::refresh(
                request.auth_id,
                request.request_id,
                request.input_generation,
            )
            .map_err(|_| StorageError::Integrity)?;
            self.crypto
                .decrypt(&binding, envelope)
                .map_err(|_| StorageError::Integrity)?;
        }
        if session.auth_id != request.auth_id
            || session.credential_generation.is_none()
            || session.status == "ready"
            || session
                .credential_generation
                .is_none_or(|generation| request.input_generation > generation)
            || (request.status == "pending"
                && !ended(session)
                && (session.status != "refreshing"
                    || session.credential_generation != Some(request.input_generation)))
            || (request.status == "unknown" && !ended(session))
            || (request.status == "completed"
                && session
                    .credential_generation
                    .is_none_or(|generation| generation <= request.input_generation))
        {
            return Err(StorageError::Integrity);
        }
        Ok(())
    }
    async fn start_refresh(
        &self,
        transaction: &mut Transaction<'_, MySql>,
        expected: Session,
        request: RefreshRequest,
        now: NaiveDateTime,
    ) -> Result<(), StorageError> {
        self.validate_session(&expected)?;
        request.validate()?;
        if expected.status != "active"
            || expected
                .auth_expires_at
                .is_none_or(|deadline| deadline <= now)
            || request.status != "pending"
            || expected.auth_id != request.auth_id
            || expected.credential_generation != Some(request.input_generation)
        {
            return Err(StorageError::Conflict);
        }
        within(request.expires_at, now, Duration::seconds(45))?;
        if lock_refresh(transaction, request.auth_id, request.request_id)
            .await?
            .is_some()
        {
            return Err(StorageError::Conflict);
        }
        let mut next = expected.clone();
        next.status = "refreshing".into();
        self.validate_pair(&next, &request)?;
        self.update(transaction, &expected, &next).await?;
        affected(
            rows::bind_refresh(sqlx::query(rows::INSERT_REFRESH), &request)
                .execute(&mut **transaction)
                .await?
                .rows_affected(),
        )
    }
    async fn finish_refresh(
        &self,
        transaction: &mut Transaction<'_, MySql>,
        sessions: (Session, Session),
        requests: (RefreshRequest, RefreshRequest),
        operation: (RefreshFence, bool),
    ) -> Result<(), StorageError> {
        let (expected, mut next) = sessions;
        let (request, mut next_request) = requests;
        let (fence, is_active_worker) = operation;
        let current = lock_refresh(transaction, expected.auth_id, request.request_id)
            .await?
            .ok_or(StorageError::NotFound)?;
        self.validate_pair(&expected, &current)?;
        // [Review Fix #34] Active workers check DB time after both locks; only cleanup may settle an expired lease.
        let now = db_now(transaction).await?;
        if (is_active_worker && fence.deadline <= now)
            || current != request
            || !fence.matches(&expected, &current)
        {
            return Err(StorageError::Conflict);
        }
        current.transition(&next_request)?;
        self.validate_outcome(&expected, &mut next, &mut next_request, (&fence, now))?;
        self.validate_pair(&next, &next_request)?;
        self.update(transaction, &expected, &next).await?;
        affected(
            rows::bind_refresh(sqlx::query(rows::UPDATE_REFRESH), &next_request)
                .execute(&mut **transaction)
                .await?
                .rows_affected(),
        )
    }
    fn validate_outcome(
        &self,
        expected: &Session,
        next: &mut Session,
        next_request: &mut RefreshRequest,
        operation: (&RefreshFence, NaiveDateTime),
    ) -> Result<(), StorageError> {
        let (fence, now) = operation;
        match next_request.status.as_str() {
            "completed" => {
                if fence.deadline <= now
                    || expected
                        .auth_expires_at
                        .is_none_or(|deadline| deadline <= now)
                    || next.status != "active"
                    || next.credential_generation != next_request.input_generation.checked_add(1)
                    || !next.login_cleared()
                    // [Review Fix #15] Never cache a releasable credential beyond its known authorization bound.
                    || Some(next_request.expires_at) > next.auth_expires_at
                    || next.auth_expires_at.is_none_or(|deadline| deadline <= now)
                {
                    return Err(StorageError::Conflict);
                }
                // [Review Fix #10] Refresh must enforce the same bounded authorization lease as initial issuance.
                within(
                    next.auth_expires_at.ok_or(StorageError::Invalid)?,
                    now,
                    Duration::days(30),
                )?;
                within(next_request.expires_at, now, Duration::minutes(5))?;
            }
            "unknown" => {
                if next.status != "reauth_required" {
                    return Err(StorageError::Conflict);
                }
                next.auth_expires_at = expected.auth_expires_at.map(|deadline| deadline.min(now));
                next_request.expires_at = now;
            }
            "failed" => self.validate_failed(expected, next, next_request, (fence, now))?,
            _ => return Err(StorageError::Conflict),
        }
        Ok(())
    }
    fn validate_failed(
        &self,
        expected: &Session,
        next: &mut Session,
        next_request: &RefreshRequest,
        operation: (&RefreshFence, NaiveDateTime),
    ) -> Result<(), StorageError> {
        let (fence, now) = operation;

        // [Review Fix #7] Proven non-consumption cannot publish a new generation or migrate credential keys.
        if next.credential_generation != expected.credential_generation
            || next.hmac_key_version != expected.hmac_key_version
            || next.broker_hash != expected.broker_hash
            || next.refresh_token_hash != expected.refresh_token_hash
        {
            return Err(StorageError::Conflict);
        }
        if fence.deadline <= now
            || !matches!(
                next.status.as_str(),
                "active" | "expired" | "reauth_required"
            )
        {
            return Err(StorageError::Conflict);
        }
        within(next_request.expires_at, now, Duration::minutes(5))?;
        if ended(next) {
            next.auth_expires_at = expected.auth_expires_at.map(|deadline| deadline.min(now));
        }
        if next.status == "active" && next.auth_expires_at.is_none_or(|deadline| deadline <= now) {
            return Err(StorageError::Conflict);
        }
        Ok(())
    }
    async fn expire_login(
        &self,
        transaction: &mut Transaction<'_, MySql>,
        expected: Session,
        fence: CallbackFence,
        now: NaiveDateTime,
    ) -> Result<(), StorageError> {
        if !fence.matches(&expected)
            || fence.deadline > now
            || !matches!(expected.status.as_str(), "pending" | "exchanging" | "ready")
        {
            return Err(StorageError::Conflict);
        }
        let mut next = expected.clone();
        next.status = "expired".into();
        next.reason_code = Some("login_expired".into());
        if expected.credential_generation.is_some() {
            next.clear_login();
            next.auth_expires_at = expected.auth_expires_at.map(|deadline| deadline.min(now));
        } else {
            next.encrypted_oauth_context = None;
            next.login_expires_at = Some(fence.deadline.min(now));
        }
        self.update(transaction, &expected, &next).await
    }
    async fn settle_ended(
        &self,
        transaction: &mut Transaction<'_, MySql>,
        expected: Session,
        request: RefreshRequest,
        now: NaiveDateTime,
    ) -> Result<(), StorageError> {
        if !ended(&expected) {
            return Err(StorageError::Conflict);
        }
        let current = lock_refresh(transaction, expected.auth_id, request.request_id)
            .await?
            .ok_or(StorageError::NotFound)?;
        self.validate_pair(&expected, &current)?;
        if current != request || current.status != "pending" {
            return Err(StorageError::Conflict);
        }
        let mut next = current.clone();
        next.status = "unknown".into();
        next.error_code = Some("refresh_outcome_unknown".into());
        next.expires_at = now;
        current.transition(&next)?;
        self.validate_pair(&expected, &next)?;
        affected(
            rows::bind_refresh(sqlx::query(rows::UPDATE_REFRESH), &next)
                .execute(&mut **transaction)
                .await?
                .rows_affected(),
        )
    }
    async fn delete_refresh(
        &self,
        transaction: &mut Transaction<'_, MySql>,
        expected: Session,
        request: RefreshRequest,
        now: NaiveDateTime,
    ) -> Result<(), StorageError> {
        let current = lock_refresh(transaction, expected.auth_id, request.request_id)
            .await?
            .ok_or(StorageError::NotFound)?;
        self.validate_pair(&expected, &current)?;
        if current != request || request.status == "pending" || request.expires_at > now {
            return Err(StorageError::Conflict);
        }
        affected(
            sqlx::query("DELETE FROM auth_refresh_requests WHERE auth_id=? AND request_id=?")
                .bind(expected.auth_id.as_bytes().as_slice())
                .bind(request.request_id.as_bytes().as_slice())
                .execute(&mut **transaction)
                .await?
                .rows_affected(),
        )
    }
    async fn delete_session(
        &self,
        transaction: &mut Transaction<'_, MySql>,
        expected: Session,
        now: NaiveDateTime,
    ) -> Result<(), StorageError> {
        let is_expired = if expected.credential_generation.is_some() {
            ended(&expected)
                && expected
                    .auth_expires_at
                    .is_some_and(|deadline| deadline <= now)
        } else {
            matches!(expected.status.as_str(), "denied" | "error" | "expired")
                && expected
                    .login_expires_at
                    .is_some_and(|deadline| deadline <= now)
        };
        if !is_expired {
            return Err(StorageError::Conflict);
        }
        let child = sqlx::query("SELECT request_id FROM auth_refresh_requests WHERE auth_id=? ORDER BY request_id LIMIT 1 FOR UPDATE")
            .bind(expected.auth_id.as_bytes().as_slice()).fetch_optional(&mut **transaction).await?;
        if child.is_some() {
            return Err(StorageError::Conflict);
        }
        affected(sqlx::query("DELETE FROM auth_sessions WHERE auth_id=? AND NOT EXISTS (SELECT 1 FROM auth_refresh_requests WHERE auth_id=?)")
            .bind(expected.auth_id.as_bytes().as_slice()).bind(expected.auth_id.as_bytes().as_slice()).execute(&mut **transaction).await?.rows_affected())
    }
}
async fn lock_session(
    transaction: &mut Transaction<'_, MySql>,
    auth_id: Uuid,
) -> Result<Option<Session>, StorageError> {
    sqlx::query("SELECT * FROM auth_sessions WHERE auth_id=? FOR UPDATE")
        .bind(auth_id.as_bytes().as_slice())
        .fetch_optional(&mut **transaction)
        .await?
        .map(rows::session)
        .transpose()
}
async fn lock_refresh(
    transaction: &mut Transaction<'_, MySql>,
    auth_id: Uuid,
    request_id: Uuid,
) -> Result<Option<RefreshRequest>, StorageError> {
    sqlx::query("SELECT * FROM auth_refresh_requests WHERE auth_id=? AND request_id=? FOR UPDATE")
        .bind(auth_id.as_bytes().as_slice())
        .bind(request_id.as_bytes().as_slice())
        .fetch_optional(&mut **transaction)
        .await?
        .map(rows::refresh)
        .transpose()
}
async fn has_children(
    transaction: &mut Transaction<'_, MySql>,
    auth_id: Uuid,
) -> Result<bool, StorageError> {
    Ok(sqlx::query("SELECT request_id FROM auth_refresh_requests WHERE auth_id=? ORDER BY request_id LIMIT 1 FOR UPDATE")
        .bind(auth_id.as_bytes().as_slice()).fetch_optional(&mut **transaction).await?.is_some())
}
async fn db_now(transaction: &mut Transaction<'_, MySql>) -> Result<NaiveDateTime, StorageError> {
    sqlx::query_scalar("SELECT UTC_TIMESTAMP(3)")
        .fetch_one(&mut **transaction)
        .await
        .map_err(Into::into)
}
fn affected(count: u64) -> Result<(), StorageError> {
    if count == 1 {
        Ok(())
    } else {
        Err(StorageError::Conflict)
    }
}
fn within(
    deadline: NaiveDateTime,
    now: NaiveDateTime,
    maximum: Duration,
) -> Result<(), StorageError> {
    if deadline > now && deadline <= now + maximum {
        Ok(())
    } else {
        Err(StorageError::Conflict)
    }
}
fn ended(row: &Session) -> bool {
    matches!(
        row.status.as_str(),
        "expired" | "revoked" | "reauth_required"
    )
}
