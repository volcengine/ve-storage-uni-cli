use super::StorageError;
use sqlx::{
    mysql::{MySqlConnectOptions, MySqlPoolOptions, MySqlSslMode},
    ConnectOptions, MySqlPool, Row,
};
use std::{net::IpAddr, path::PathBuf, str::FromStr, time::Duration};
use zeroize::Zeroizing;

pub(crate) const DEADLINE: Duration = Duration::from_secs(3);
/// TLS is mandatory. Test certificates without identity checks are limited to numeric loopback.
pub enum Tls {
    VerifyIdentity { ca: PathBuf },
    LoopbackTlsTestOnly,
}
/// Program-injected writer configuration; no environment fallback and no Debug implementation.
pub struct DatabaseConfig {
    url: Zeroizing<String>,
    tls: Tls,
    maximum_connections: u32,
}
impl DatabaseConfig {
    /// Construct writer configuration. Connection validates TLS, server and pool bounds.
    /// URL is kept secret and erased on drop; zero or >32 pool bounds return Invalid.
    pub fn new(url: String, tls: Tls, maximum_connections: u32) -> Result<Self, StorageError> {
        if !(1..=32).contains(&maximum_connections) {
            return Err(StorageError::Invalid);
        }
        let configuration = Self {
            url: Zeroizing::new(url),
            tls,
            maximum_connections,
        };
        configuration.options()?;
        Ok(configuration)
    }
    fn options(&self) -> Result<MySqlConnectOptions, StorageError> {
        let options =
            MySqlConnectOptions::from_str(&self.url).map_err(|_| StorageError::Invalid)?;
        // [Review Fix #1] A URL socket must never bypass the explicit TLS/host policy.
        if options.get_socket().is_some() || options.get_database().is_none() {
            return Err(StorageError::Invalid);
        }
        let options = match &self.tls {
            Tls::VerifyIdentity { ca } if ca.is_file() => {
                options.ssl_mode(MySqlSslMode::VerifyIdentity).ssl_ca(ca)
            }
            Tls::LoopbackTlsTestOnly
                if options
                    .get_host()
                    .parse::<IpAddr>()
                    .is_ok_and(|address| address.is_loopback()) =>
            {
                // [Review Fix #18] Require TLS so caching_sha2 authentication never needs optional RSA.
                options.ssl_mode(MySqlSslMode::Required)
            }
            _ => return Err(StorageError::Invalid),
        };
        Ok(options
            .charset("utf8mb4")
            .collation("utf8mb4_bin")
            .timezone(Some("+00:00".into()))
            .disable_statement_logging())
    }
    async fn connect(&self) -> Result<MySqlPool, StorageError> {
        self.connect_pool(false).await
    }
    async fn connect_pool(&self, is_cleanup: bool) -> Result<MySqlPool, StorageError> {
        let pool = MySqlPoolOptions::new().max_connections(self.maximum_connections).acquire_timeout(DEADLINE)
            .after_connect(move |connection, _| Box::pin(async move {
                // [Review Fix #5] SQLx handshake collation enum lacks 0900_bin; set the real session collation explicitly.
                sqlx::query("SET NAMES utf8mb4 COLLATE utf8mb4_0900_bin").execute(&mut *connection).await?;
                sqlx::query("SET SESSION sql_mode = 'STRICT_ALL_TABLES,NO_ZERO_DATE,NO_ZERO_IN_DATE,ERROR_FOR_DIVISION_BY_ZERO,NO_ENGINE_SUBSTITUTION', innodb_lock_wait_timeout = 2").execute(&mut *connection).await?;
                let row = sqlx::query("SELECT @@read_only AS ro, @@super_read_only AS sro, CAST(@@innodb_page_size AS SIGNED) AS page_size, VERSION() AS version").fetch_one(&mut *connection).await?;
                let version: String = row.try_get("version")?;
                let numbers: Vec<u32> = version.split(['.', '-']).take(3).map(|part| part.parse().unwrap_or(0)).collect();
                let is_supported = numbers.len() == 3 && (numbers[0], numbers[1], numbers[2]) >= (8, 0, 21) && !version.contains("MariaDB");
                if row.try_get::<i64, _>("ro")? != 0 || row.try_get::<i64, _>("sro")? != 0 || row.try_get::<i64, _>("page_size")? != 16384 || !is_supported {
                    return Err(sqlx::Error::Protocol("unsupported writer configuration".into()));
                }
                if is_cleanup {
                    sqlx::query("SET SESSION TRANSACTION ISOLATION LEVEL READ COMMITTED").execute(&mut *connection).await?;
                    sqlx::query("SET SESSION innodb_lock_wait_timeout=1, max_execution_time=1000").execute(&mut *connection).await?;
                    let format: String = sqlx::query_scalar("SELECT @@binlog_format").fetch_one(&mut *connection).await?;
                    if format != "ROW" { return Err(sqlx::Error::Protocol("cleanup requires row replication".into())); }
                }
                Ok(())
            })).connect_with(self.options()?).await.map_err(StorageError::from)?;
        Ok(pool)
    }
}
/// DDL account capability, separate from the runtime store.
pub struct MigrationDatabase {
    pool: MySqlPool,
}
impl MigrationDatabase {
    /// Connect with explicitly injected migration credentials; timeout/error is sanitized.
    pub async fn connect(configuration: DatabaseConfig) -> Result<Self, StorageError> {
        Ok(Self {
            pool: bounded(configuration.connect()).await?,
        })
    }
    /// Apply embedded frozen DDL with SQLx advisory lock, version/checksum and dirty tracking.
    /// Partial DDL/checksum failures return Migration and require reviewed operator correction.
    pub async fn migrate(&self) -> Result<(), StorageError> {
        use sqlx::migrate::{Migration, MigrationType, Migrator};
        let migration = Migration::new(
            1,
            "initial_schema".into(),
            MigrationType::Simple,
            sqlx::SqlStr::from_static(include_str!("../../migrations/001_initial_schema.sql")),
            false,
        );
        let migrator = Migrator {
            migrations: vec![migration].into(),
            ..Migrator::DEFAULT
        };
        // [Review Fix #3] MySQL advisory locks survive rollback; errors/cancellation must close the socket.
        let mut connection = bounded(async { Ok(self.pool.acquire().await?.detach()) }).await?;
        tokio::time::timeout(Duration::from_secs(30), migrator.run(&mut connection))
            .await
            .map_err(|_| StorageError::Timeout)?
            .map_err(|_| StorageError::Migration)
    }
    /// Close the migration pool after deployment; returns when all connections are released.
    pub async fn close(&self) {
        self.pool.close().await;
    }
}
/// Runtime DML capability. It exposes no raw pool or SQL escape hatch.
pub struct RuntimeDatabase {
    pub(crate) pool: MySqlPool,
}
impl RuntimeDatabase {
    /// Connect to the writer with runtime credentials; never applies DDL or reads env settings.
    pub async fn connect(configuration: DatabaseConfig) -> Result<Self, StorageError> {
        Ok(Self {
            pool: bounded(configuration.connect()).await?,
        })
    }
    /// Permanently reject new acquisitions immediately; await release/closure of all connections.
    /// Idempotent across concurrent calls. Cancellation leaves the pool closed; this cannot fail.
    pub fn close(&self) -> impl std::future::Future<Output = ()> + '_ {
        self.pool.close()
    }
}
pub(crate) async fn bounded<T>(
    operation: impl std::future::Future<Output = Result<T, StorageError>>,
) -> Result<T, StorageError> {
    tokio::time::timeout(DEADLINE, operation)
        .await
        .map_err(|_| StorageError::Timeout)?
}

/// Dedicated writer-only cleanup capability; never borrows API connections.
pub struct CleanupDatabase {
    pub(super) pool: MySqlPool,
}
impl CleanupDatabase {
    /// Validate writer/TLS baseline, ROW replication, UTC and READ COMMITTED.
    /// Invalid server settings or connection failures return sanitized storage errors.
    pub async fn connect(configuration: DatabaseConfig) -> Result<Self, StorageError> {
        Ok(Self {
            pool: bounded(configuration.connect_pool(true)).await?,
        })
    }
    /// Permanently reject new acquisitions immediately; await release/closure of all connections.
    /// Idempotent across concurrent calls. Cancellation leaves the pool closed; this cannot fail.
    pub fn close(&self) -> impl std::future::Future<Output = ()> + '_ {
        self.pool.close()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_loopback_test_connections_require_tls() {
        let configuration = DatabaseConfig::new(
            "mysql://test:synthetic@127.0.0.1/isolated".into(),
            Tls::LoopbackTlsTestOnly,
            1,
        )
        .unwrap();
        assert!(matches!(
            configuration.options().unwrap().get_ssl_mode(),
            MySqlSslMode::Required
        ));
    }

    #[tokio::test]
    async fn closing_database_capabilities_without_polling_permanently_rejects_acquisitions() {
        let pool = MySqlPoolOptions::new()
            .connect_lazy("mysql://127.0.0.1/ask_auth_stage4_cleanup_test")
            .unwrap();
        let runtime = RuntimeDatabase { pool: pool.clone() };
        drop(runtime.close());
        assert!(pool.is_closed());
        assert!(matches!(pool.acquire().await, Err(sqlx::Error::PoolClosed)));
        tokio::join!(runtime.close(), runtime.close());

        let pool = MySqlPoolOptions::new()
            .connect_lazy("mysql://127.0.0.1/ask_auth_stage4_cleanup_test")
            .unwrap();
        let cleanup = CleanupDatabase { pool: pool.clone() };
        drop(cleanup.close());
        assert!(pool.is_closed());
        assert!(matches!(pool.acquire().await, Err(sqlx::Error::PoolClosed)));
        tokio::join!(cleanup.close(), cleanup.close());
    }
}
