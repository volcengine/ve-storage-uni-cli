use super::*;

impl Store {
    pub(crate) async fn refresh_snapshot(
        &self,
        auth_id: Uuid,
        request_id: Uuid,
    ) -> Result<(Option<Session>, Option<RefreshRequest>, NaiveDateTime), StorageError> {
        bounded(async {
            if !is_uuid_v7(auth_id) || !is_uuid_v7(request_id) {
                return Err(StorageError::Invalid);
            }
            let mut transaction = self.database.pool.begin().await?;
            let session = lock_session(&mut transaction, auth_id).await?;
            if let Some(ref row) = session {
                self.validate_proof_shape(row)?;
            }
            let request = lock_refresh(&mut transaction, auth_id, request_id).await?;
            if let Some(ref row) = request {
                row.validate()?;
                if !self.crypto.has_version(row.hmac_key_version) {
                    return Err(StorageError::Integrity);
                }
            }
            let now = db_now(&mut transaction).await?;
            transaction.commit().await?;
            Ok((session, request, now))
        })
        .await
    }
    pub(crate) async fn reserve_refresh(
        &self,
        expected: &Session,
        request: &RefreshRequest,
        policy_deadline: NaiveDateTime,
    ) -> Result<RefreshRequest, StorageError> {
        bounded(async {
            let mut transaction = self.database.pool.begin().await?;
            let current = lock_session(&mut transaction, expected.auth_id)
                .await?
                .ok_or(StorageError::NotFound)?;
            self.validate_session(&current)?;
            let now = db_now(&mut transaction).await?;
            // [Review Fix #36] A policy checked before reservation cannot authorize work after its own expiry.
            if current != *expected || policy_deadline <= now {
                return Err(StorageError::Conflict);
            }
            // [Review Fix #38] Start the execution lease at locked DB now and return its canonical persisted fence input.
            let mut reserved = request.clone();
            reserved.expires_at = now + Duration::seconds(45);
            self.start_refresh(&mut transaction, current, reserved.clone(), now)
                .await?;
            transaction.commit().await?;
            Ok(reserved)
        })
        .await
    }
    pub(crate) async fn finish_active_refresh(
        &self,
        sessions: (Session, Session),
        requests: (RefreshRequest, RefreshRequest),
        fence: RefreshFence,
    ) -> Result<(), StorageError> {
        bounded(async {
            let mut transaction = self.database.pool.begin().await?;
            let current = lock_session(&mut transaction, sessions.0.auth_id)
                .await?
                .ok_or(StorageError::NotFound)?;
            self.validate_session(&current)?;
            if current != sessions.0 {
                return Err(StorageError::Conflict);
            }
            self.finish_refresh(&mut transaction, sessions, requests, (fence, true))
                .await?;
            transaction.commit().await.map_err(Into::into)
        })
        .await
    }
    fn validate_proof_shape(&self, row: &Session) -> Result<(), StorageError> {
        row.validate(&self.app)?;
        if !self.crypto.has_version(row.hmac_key_version) {
            return Err(StorageError::Integrity);
        }
        Ok(())
    }
    pub(crate) fn validate_refresh_snapshot(
        &self,
        session: &Session,
        request: &RefreshRequest,
    ) -> Result<(), StorageError> {
        self.validate_pair(session, request)
    }
    pub(crate) async fn confirm_refresh(
        &self,
        session: &Session,
        request: &RefreshRequest,
        deadline: NaiveDateTime,
    ) -> Result<(), StorageError> {
        bounded(async {
            let mut transaction = self.database.pool.begin().await?;
            let current = lock_session(&mut transaction, session.auth_id)
                .await?
                .ok_or(StorageError::NotFound)?;
            let child = lock_refresh(&mut transaction, session.auth_id, request.request_id)
                .await?
                .ok_or(StorageError::NotFound)?;
            self.validate_pair(&current, &child)?;
            let now = db_now(&mut transaction).await?;
            if current != *session
                || child != *request
                || current.status != "active"
                || child.status != "completed"
                || child.input_generation.checked_add(1) != current.credential_generation
                || child.expires_at <= now
                || deadline <= now
                || current.auth_expires_at.is_none_or(|expiry| expiry <= now)
            {
                return Err(StorageError::Conflict);
            }
            transaction.commit().await.map_err(Into::into)
        })
        .await
    }
    pub(crate) async fn confirm_status(
        &self,
        expected: &Session,
        policy_deadline: NaiveDateTime,
    ) -> Result<(), StorageError> {
        bounded(async {
            let mut transaction = self.database.pool.begin().await?;
            let current = lock_session(&mut transaction, expected.auth_id)
                .await?
                .ok_or(StorageError::NotFound)?;
            self.validate_session(&current)?;
            let now = db_now(&mut transaction).await?;
            if current != *expected
                || policy_deadline <= now
                || (!ended(&current) && current.auth_expires_at.is_none_or(|expiry| expiry <= now))
            {
                return Err(StorageError::Conflict);
            }
            transaction.commit().await.map_err(Into::into)
        })
        .await
    }
    // A is already authenticated. The exact snapshot prevents a rotated broker revoking its successor.
    pub(crate) async fn terminate_authorization(
        &self,
        expected: &Session,
        is_logout: bool,
    ) -> Result<Session, StorageError> {
        bounded(async {
            let mut transaction = self.database.pool.begin().await?;
            let current = lock_session(&mut transaction, expected.auth_id)
                .await?
                .ok_or(StorageError::NotFound)?;
            self.validate_session(&current)?;
            let now = db_now(&mut transaction).await?;
            if current != *expected
                || current.credential_generation.is_none()
                || current.status == "ready"
                || (!is_logout && current.auth_expires_at.is_none_or(|expiry| expiry > now))
            {
                return Err(StorageError::Conflict);
            }
            let children = pending_children(&mut transaction, current.auth_id).await?;
            if children.len() > 1 {
                return Err(StorageError::Integrity);
            }
            let mut next = current.clone();
            if !ended(&current) {
                next.status = if is_logout { "revoked" } else { "expired" }.into();
                next.reason_code = Some(
                    if is_logout {
                        "revoked"
                    } else {
                        "authorization_expired"
                    }
                    .into(),
                );
                next.auth_expires_at = current.auth_expires_at.map(|expiry| expiry.min(now));
                next.clear_login();
                self.update(&mut transaction, &current, &next).await?;
            }
            for child in children {
                self.validate_pair(&current, &child)?;
                self.settle_ended(&mut transaction, next.clone(), child, now)
                    .await?;
            }
            transaction.commit().await?;
            Ok(next)
        })
        .await
    }
}
async fn pending_children(
    transaction: &mut Transaction<'_, MySql>,
    auth_id: Uuid,
) -> Result<Vec<RefreshRequest>, StorageError> {
    sqlx::query("SELECT * FROM auth_refresh_requests WHERE auth_id=? AND status='pending' ORDER BY request_id LIMIT 2 FOR UPDATE")
        .bind(auth_id.as_bytes().as_slice())
        .fetch_all(&mut **transaction).await?
        .into_iter().map(rows::refresh).collect()
}
