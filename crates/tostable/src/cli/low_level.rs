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

// ============ Bucket ============
#[derive(Debug, Args)]
pub struct TBucketCommand {
    #[command(subcommand)]
    pub action: TBucketAction,
}

#[derive(Debug, Subcommand)]
pub enum TBucketAction {
    Create(TBucketCreateArgs),
    Get(TBucketNameArgs),
    Delete(TBucketNameArgs),
    List,
}

#[derive(Debug, Args)]
pub struct TBucketCreateArgs {
    pub bucket: String,
    #[arg(long)]
    pub region: Option<String>,
}

#[derive(Debug, Args)]
pub struct TBucketNameArgs {
    pub bucket: String,
}

// ============ Namespace ============
#[derive(Debug, Args)]
pub struct NamespaceCommand {
    #[command(subcommand)]
    pub action: NamespaceAction,
}

#[derive(Debug, Subcommand)]
pub enum NamespaceAction {
    Create(NamespaceCreateArgs),
    Get(NamespaceGetArgs),
    Delete(NamespaceGetArgs),
    List(NamespaceListArgs),
}

#[derive(Debug, Args)]
pub struct NamespaceCreateArgs {
    #[arg(long)]
    pub bucket: String,
    #[arg(long)]
    pub namespace: String,
}

#[derive(Debug, Args)]
pub struct NamespaceGetArgs {
    #[arg(long)]
    pub bucket: String,
    #[arg(long)]
    pub namespace: String,
}

#[derive(Debug, Args)]
pub struct NamespaceListArgs {
    #[arg(long)]
    pub bucket: String,
}

// ============ Table ============
#[derive(Debug, Args)]
pub struct TableCommand {
    #[command(subcommand)]
    pub action: TableAction,
}

#[derive(Debug, Subcommand)]
pub enum TableAction {
    Create(TableCreateArgs),
    Get(TableGetArgs),
    Delete(TableGetArgs),
    List(TableListArgs),
    Rename(TableRenameArgs),
    Metadata(TableGetArgs),
}

#[derive(Debug, Args)]
pub struct TableCreateArgs {
    #[arg(long)]
    pub bucket: String,
    #[arg(long)]
    pub namespace: String,
    #[arg(long)]
    pub table: String,
    /// Table schema (JSON)
    #[arg(long)]
    pub schema: Option<String>,
}

#[derive(Debug, Args)]
pub struct TableGetArgs {
    #[arg(long)]
    pub bucket: String,
    #[arg(long)]
    pub namespace: String,
    #[arg(long)]
    pub table: String,
}

#[derive(Debug, Args)]
pub struct TableListArgs {
    #[arg(long)]
    pub bucket: String,
    #[arg(long)]
    pub namespace: String,
}

#[derive(Debug, Args)]
pub struct TableRenameArgs {
    #[arg(long)]
    pub bucket: String,
    #[arg(long)]
    pub namespace: String,
    #[arg(long)]
    pub table: String,
    #[arg(long)]
    pub new_name: String,
}

// ============ Maintenance ============
#[derive(Debug, Args)]
pub struct MaintenanceCommand {
    #[command(subcommand)]
    pub action: MaintenanceAction,
}

#[derive(Debug, Subcommand)]
pub enum MaintenanceAction {
    Get(MaintenanceArgs),
    Set(MaintenanceSetArgs),
}

#[derive(Debug, Args)]
pub struct MaintenanceArgs {
    #[arg(long)]
    pub bucket: String,
}

#[derive(Debug, Args)]
pub struct MaintenanceSetArgs {
    #[arg(long)]
    pub bucket: String,
    /// Configuration JSON
    #[arg(long)]
    pub config: String,
}
