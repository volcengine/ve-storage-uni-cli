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

use tos_core::agent::error::CliError;
use tos_core::agent::global_args::GlobalArgs;
use ve_tos_cli::cli::high_level::RecursiveListMode;

use crate::cli::TosCliCommand;

const FORCE_HIERARCHICAL_ENV: &str = "VE_STORAGE_UNI_TOS_FORCE_HIERARCHICAL_LISTING";
const FNS_DELETE_ENV: &str = "VE_STORAGE_UNI_TOS_FORCE_FNS_DELETE";
const TOS_CONFIG_BINARY_ENV: &str = "VE_STORAGE_UNI_TOS_CONFIG_BINARY";

struct EnvGuard {
    saved: Vec<(&'static str, Option<String>)>,
}

impl EnvGuard {
    fn set(values: &[(&'static str, &'static str)]) -> Self {
        let saved = values
            .iter()
            .map(|(key, _)| (*key, std::env::var(key).ok()))
            .collect::<Vec<_>>();
        for (key, value) in values {
            std::env::set_var(key, value);
        }
        Self { saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.saved.drain(..) {
            if let Some(value) = value {
                std::env::set_var(key, value);
            } else {
                std::env::remove_var(key);
            }
        }
    }
}

/// Execute a ByteCloud TOS high-level command through the shared TOS transfer
/// engine while pinning `tos-cli`-specific list/delete semantics.
pub async fn handle_high_level_command(
    global: &GlobalArgs,
    command: TosCliCommand,
) -> Result<i32, CliError> {
    validate_tos_cli_high_level_options(&command)?;
    let _guard = EnvGuard::set(&[(FORCE_HIERARCHICAL_ENV, "1"), (FNS_DELETE_ENV, "1")]);
    let _config_guard = EnvGuard::set(&[(TOS_CONFIG_BINARY_ENV, "tos")]);
    let ve_command = match command {
        TosCliCommand::Cp(args) => ve_tos_cli::TosCommand::Cp(args),
        TosCliCommand::Mv(args) => ve_tos_cli::TosCommand::Mv(args),
        TosCliCommand::Sync(args) => ve_tos_cli::TosCommand::Sync(args),
        TosCliCommand::Mkdir(args) => ve_tos_cli::TosCommand::Mkdir(args),
        TosCliCommand::Rm(args) => ve_tos_cli::TosCommand::Rm(args.into_ve_tos_args()),
        TosCliCommand::Ls(args) => ve_tos_cli::TosCommand::Ls(args),
        TosCliCommand::Stat(args) => ve_tos_cli::TosCommand::Stat(args),
        TosCliCommand::Du(args) => ve_tos_cli::TosCommand::Du(args),
        TosCliCommand::Find(args) => ve_tos_cli::TosCommand::Find(args),
        TosCliCommand::Cat(args) => ve_tos_cli::TosCommand::Cat(args),
        TosCliCommand::Put(args) => ve_tos_cli::TosCommand::Put(args),
        TosCliCommand::Presign(args) => ve_tos_cli::TosCommand::Presign(args),
        other => {
            return Err(CliError::ValidationError(format!(
                "{} is not a high-level TOS command",
                crate::cli::command_path(&other)
            )))
        }
    };
    ve_tos_cli::handler::high_level::handle_high_level_command(global, &ve_command).await
}

fn validate_tos_cli_high_level_options(command: &TosCliCommand) -> Result<(), CliError> {
    validate_tos_cli_target_scope(command)?;
    validate_tos_cli_storage_class(command)?;
    validate_tos_cli_list_mode(command)?;
    Ok(())
}

fn validate_tos_cli_storage_class(command: &TosCliCommand) -> Result<(), CliError> {
    let storage_class = match command {
        TosCliCommand::Cp(args) => args.storage_class.as_deref(),
        TosCliCommand::Mv(args) => args.storage_class.as_deref(),
        TosCliCommand::Sync(args) => args.storage_class.as_deref(),
        TosCliCommand::Put(args) => args.storage_class.as_deref(),
        TosCliCommand::Find(args) => args.storage_class.as_deref(),
        _ => None,
    };
    if storage_class.is_some() {
        return Err(CliError::ValidationError(format!(
            "{} does not support --storage-class",
            crate::cli::command_path(command)
        )));
    }
    Ok(())
}

fn validate_tos_cli_target_scope(command: &TosCliCommand) -> Result<(), CliError> {
    if let TosCliCommand::Ls(args) = command {
        let has_object_listing_target = args.path.is_some() || args.bucket.is_some();
        if !has_object_listing_target {
            // [Review Fix #8] ByteCloud `tos ls` is object/prefix scoped only.
            // The old `ve-tos ls` service-level bucket listing must not leak
            // into the new `tos` surface.
            return Err(CliError::ValidationError(
                "tos-cli ls only supports object listing; provide tos://bucket/prefix or --bucket BUCKET".to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_tos_cli_list_mode(command: &TosCliCommand) -> Result<(), CliError> {
    let mode = match command {
        TosCliCommand::Cp(args) => args.recursive_list_mode,
        TosCliCommand::Mv(args) => args.recursive_list_mode,
        TosCliCommand::Sync(args) => args.recursive_list_mode,
        TosCliCommand::Rm(args) => args.recursive_list_mode,
        _ => None,
    };
    match mode {
        Some(mode @ (RecursiveListMode::Auto | RecursiveListMode::Flat)) => {
            Err(CliError::ValidationError(format!(
                "tos-cli recursive listing only supports delimiter=\"/\"; --recursive-list-mode {} is not supported",
                recursive_list_mode_name(mode)
            )))
        }
        _ => Ok(()),
    }
}

fn recursive_list_mode_name(mode: RecursiveListMode) -> &'static str {
    match mode {
        RecursiveListMode::Auto => "auto",
        RecursiveListMode::Flat => "flat",
        RecursiveListMode::Hierarchical => "hierarchical",
    }
}
