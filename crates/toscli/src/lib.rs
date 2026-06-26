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

//! ByteCloud `tos-cli` command definitions.
//!
//! This crate intentionally depends on the existing in-repo `ve-tos-cli-core`
//! implementation for mature high-level transfer execution, but it does not
//! depend on the internal `tos-rust-sdk` repository.

pub use tos_core;

pub mod cli;
pub mod handler;
pub mod registry;

pub use cli::command_path;
pub use cli::print_grouped_help;
pub use cli::TosCliCommand;
