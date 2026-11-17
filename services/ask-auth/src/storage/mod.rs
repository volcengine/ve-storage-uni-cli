//! Writer-only validated persistence with bounded, closed lifecycle operations.
mod model;
mod mysql;
mod rows;
mod store;
pub use model::*;
pub use mysql::{CleanupDatabase, DatabaseConfig, MigrationDatabase, RuntimeDatabase, Tls};
pub use store::cleanup::{
    CleanupCandidate, CleanupCursor, CleanupOutcome, CleanupQueue, CleanupStore,
};
pub use store::{CallbackFence, Mutation, RefreshFence, Store};

#[cfg(test)]
pub(crate) struct RuntimePoolState {
    api: sqlx::MySqlPool,
    cleanup: sqlx::MySqlPool,
}
#[cfg(test)]
impl RuntimePoolState {
    pub(crate) fn is_api_closed(&self) -> bool {
        self.api.is_closed()
    }
    pub(crate) fn is_cleanup_closed(&self) -> bool {
        self.cleanup.is_closed()
    }
}

#[cfg(test)]
pub(crate) fn lazy_runtime_stores(
    api_crypto: std::sync::Arc<crate::crypto::Crypto>,
    cleanup_crypto: std::sync::Arc<crate::crypto::Crypto>,
) -> (
    std::sync::Arc<Store>,
    std::sync::Arc<CleanupStore>,
    RuntimePoolState,
) {
    let api = RuntimeDatabase {
        pool: sqlx::mysql::MySqlPoolOptions::new()
            .connect_lazy("mysql://127.0.0.1/ask_auth_stage4_cleanup_test")
            .unwrap(),
    };
    let cleanup = CleanupDatabase {
        pool: sqlx::mysql::MySqlPoolOptions::new()
            .connect_lazy("mysql://127.0.0.1/ask_auth_stage4_cleanup_test")
            .unwrap(),
    };
    let api_app = api_crypto.app().to_owned();
    let cleanup_app = cleanup_crypto.app().to_owned();
    let pool_state = RuntimePoolState {
        api: api.pool.clone(),
        cleanup: cleanup.pool.clone(),
    };
    (
        std::sync::Arc::new(Store::new(api, &api_app, api_crypto).unwrap()),
        std::sync::Arc::new(CleanupStore::new(cleanup, &cleanup_app, cleanup_crypto).unwrap()),
        pool_state,
    )
}

/// Sanitized storage failures; SQL/server/DSN values are never retained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageError {
    Invalid,
    Integrity,
    Conflict,
    NotFound,
    Unavailable,
    Timeout,
    Migration,
    Retryable,
}
impl std::fmt::Display for StorageError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "storage_{self:?}")
    }
}
impl std::error::Error for StorageError {}
impl From<sqlx::Error> for StorageError {
    fn from(error: sqlx::Error) -> Self {
        match error {
            sqlx::Error::PoolTimedOut => Self::Timeout,
            sqlx::Error::Database(ref failure) if failure.is_unique_violation() => Self::Conflict,
            sqlx::Error::Database(ref failure)
                if matches!(failure.code().as_deref(), Some("40001" | "41000"))
                    || failure
                        .try_downcast_ref::<sqlx::mysql::MySqlDatabaseError>()
                        .is_some_and(|error| matches!(error.number(), 1205 | 1213)) =>
            {
                // [Review Fix #46] MySQL lock waits use HY000; classify typed numbers without retaining server text.
                Self::Retryable
            }
            sqlx::Error::ColumnDecode { .. }
            | sqlx::Error::Decode(_)
            | sqlx::Error::ColumnNotFound(_) => Self::Integrity,
            _ => Self::Unavailable,
        }
    }
}
