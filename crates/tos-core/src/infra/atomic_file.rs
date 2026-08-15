/*
 * Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 * http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Crash-safe persistence for local configuration and credential files.

use std::io::Write;
use std::path::Path;

use crate::agent::error::CliError;

/// Write a complete owner-only sibling file and atomically replace `path`.
pub(crate) fn write_owner_only_atomic(path: &Path, content: &[u8]) -> Result<(), CliError> {
    let temp_path = path.with_extension(format!("tmp-{}", ulid::Ulid::new()));
    let prepare_result = (|| {
        let mut temp_file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .map_err(CliError::Io)?;
        set_owner_only_permissions(&temp_path)?;
        temp_file.write_all(content).map_err(CliError::Io)?;
        temp_file.sync_all().map_err(CliError::Io)
    })();
    if let Err(error) = prepare_result {
        let _ = std::fs::remove_file(&temp_path);
        return Err(error);
    }
    replace_file(&temp_path, path).map_err(|error| {
        let _ = std::fs::remove_file(&temp_path);
        CliError::Io(error)
    })?;
    set_owner_only_permissions(path)?;
    sync_parent_directory(path)
}

#[cfg(not(windows))]
fn replace_file(temp_path: &Path, path: &Path) -> std::io::Result<()> {
    std::fs::rename(temp_path, path)
}

#[cfg(windows)]
fn replace_file(temp_path: &Path, path: &Path) -> std::io::Result<()> {
    if !path.exists() {
        return std::fs::rename(temp_path, path);
    }
    let backup_path = path.with_extension(format!("backup-{}", ulid::Ulid::new()));
    std::fs::rename(path, &backup_path)?;
    if let Err(error) = std::fs::rename(temp_path, path) {
        let _ = std::fs::rename(&backup_path, path);
        return Err(error);
    }
    std::fs::remove_file(backup_path)
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> Result<(), CliError> {
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(CliError::Io)
}

#[cfg(not(unix))]
fn sync_parent_directory(_path: &Path) -> Result<(), CliError> {
    Ok(())
}

#[cfg(unix)]
pub(crate) fn set_owner_only_permissions(path: &Path) -> Result<(), CliError> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path).map_err(CliError::Io)?.permissions();
    permissions.set_mode(0o600);
    std::fs::set_permissions(path, permissions).map_err(CliError::Io)
}

#[cfg(not(unix))]
pub(crate) fn set_owner_only_permissions(_path: &Path) -> Result<(), CliError> {
    Ok(())
}
