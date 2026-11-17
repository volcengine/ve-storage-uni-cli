use crate::platform::{json, settings::SecretText, AppError};
use crate::storage::{DatabaseConfig, MigrationDatabase, Tls};
use futures::FutureExt;
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use zeroize::Zeroizing;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MigrationFields {
    schema_version: u16,
    database_url: SecretText,
    database_ca_path: PathBuf,
    maximum_connections: u32,
}
pub(crate) fn read_config(path: &Path) -> Result<DatabaseConfig, AppError> {
    let bytes = read_protected(path)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| AppError::MigrationFile)?;
    let mut fields: MigrationFields = json::parse(text).map_err(|_| AppError::MigrationFile)?;
    if fields.schema_version != 1
        || fields.maximum_connections != 1
        || !fields.database_ca_path.is_absolute()
    {
        return Err(AppError::MigrationFile);
    }
    DatabaseConfig::new(
        std::mem::take(&mut fields.database_url.0),
        Tls::VerifyIdentity {
            ca: fields.database_ca_path,
        },
        1,
    )
    .map_err(|_| AppError::MigrationFile)
}
#[cfg(unix)]
fn read_protected(path: &Path) -> Result<Zeroizing<Vec<u8>>, AppError> {
    use std::{
        io::Read,
        os::unix::fs::{MetadataExt, OpenOptionsExt},
    };
    if !path.is_absolute() {
        return Err(AppError::MigrationFile);
    }
    // [Review Fix #74] NOFOLLOW prevents leaf substitution; NONBLOCK prevents FIFO open from hanging before fstat.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| AppError::MigrationFile)?;
    let metadata = file.metadata().map_err(|_| AppError::MigrationFile)?;
    let owner = metadata.uid();
    let mode = metadata.mode() & 0o7777;
    let is_owned = owner == 0 || owner == unsafe { libc::geteuid() };
    if !metadata.is_file()
        || !is_owned
        || !matches!(mode, 0o400 | 0o600)
        || metadata.len() == 0
        || metadata.len() > 32768
    {
        return Err(AppError::MigrationFile);
    }
    // [Review Fix #80] Reserve one fixed bounded buffer so a growing file cannot leave a freed plaintext allocation.
    let mut bytes = Zeroizing::new(vec![0; 32769]);
    let mut file = file;
    let mut length = 0;
    while length < bytes.len() {
        let count = file
            .read(&mut bytes[length..])
            .map_err(|_| AppError::MigrationFile)?;
        if count == 0 {
            break;
        }
        length += count;
    }
    if length as u64 != metadata.len() {
        return Err(AppError::MigrationFile);
    }
    bytes.truncate(length);
    Ok(bytes)
}
#[cfg(not(unix))]
fn read_protected(_path: &Path) -> Result<Zeroizing<Vec<u8>>, AppError> {
    Err(AppError::MigrationFile)
}
struct MigrationGuard(MigrationDatabase);
impl Drop for MigrationGuard {
    fn drop(&mut self) {
        // [Review Fix #75] Core migration close is async; poll once to mark its SQLx pool closed even on caller cancellation.
        let _ = self.0.close().now_or_never();
    }
}
pub(crate) async fn run(path: &Path) -> Result<(), AppError> {
    let configuration = read_config(path)?;
    let database = MigrationGuard(
        MigrationDatabase::connect(configuration)
            .await
            .map_err(|_| AppError::Database)?,
    );
    let result = database.0.migrate().await.map_err(|_| AppError::Migration);
    let closed = tokio::time::timeout(Duration::from_secs(3), database.0.close())
        .await
        .map_err(|_| AppError::Database);
    result.and(closed)
}
// [Review Fix #104] Protected file fixtures require Unix open options; unsupported hosts fail closed.
#[cfg(all(test, unix))]
mod tests;
