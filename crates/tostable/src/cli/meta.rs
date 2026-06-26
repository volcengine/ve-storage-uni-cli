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

use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct CapabilitiesArgs {
    #[arg(long, default_value = "groups")]
    pub view: String,
    #[arg(long)]
    pub group: Option<String>,
    #[arg(long)]
    pub search: Option<String>,
}

#[derive(Debug, Args)]
pub struct ApiArgs {
    pub group: String,
    pub action: String,
    #[arg(long)]
    pub request: Option<String>,
    #[arg(long)]
    pub describe: bool,
}

#[derive(Debug, Args)]
pub struct ConfigCommand {
    #[command(subcommand)]
    pub action: ConfigAction,
}

#[derive(Debug, Subcommand)]
pub enum ConfigAction {
    Init {
        #[arg(long)]
        profile: Option<String>,
    },
    Show,
    Set {
        key: String,
        value: String,
    },
}

#[derive(Debug, Args)]
pub struct CompletionArgs {
    pub shell: String,
}
