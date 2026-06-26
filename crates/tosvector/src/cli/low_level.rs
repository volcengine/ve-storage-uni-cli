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
pub struct VBucketCommand {
    #[command(subcommand)]
    pub action: VBucketAction,
}

#[derive(Debug, Subcommand)]
pub enum VBucketAction {
    /// Create a vector bucket
    Create(VBucketCreateArgs),
    /// Get vector bucket info
    Get(VBucketNameArgs),
    /// Delete a vector bucket
    Delete(VBucketNameArgs),
    /// List vector buckets
    List,
}

#[derive(Debug, Args)]
pub struct VBucketCreateArgs {
    pub bucket: String,
    #[arg(long)]
    pub region: Option<String>,
}

#[derive(Debug, Args)]
pub struct VBucketNameArgs {
    pub bucket: String,
}

// ============ Policy ============
#[derive(Debug, Args)]
pub struct VPolicyCommand {
    #[command(subcommand)]
    pub action: VPolicyAction,
}

#[derive(Debug, Subcommand)]
pub enum VPolicyAction {
    Get(VPolicyBucketArgs),
    Set(VPolicySetArgs),
    Delete(VPolicyBucketArgs),
}

#[derive(Debug, Args)]
pub struct VPolicyBucketArgs {
    #[arg(long)]
    pub bucket: String,
}

#[derive(Debug, Args)]
pub struct VPolicySetArgs {
    #[arg(long)]
    pub bucket: String,
    #[arg(long)]
    pub policy: String,
}

// ============ Index ============
#[derive(Debug, Args)]
pub struct IndexCommand {
    #[command(subcommand)]
    pub action: IndexAction,
}

#[derive(Debug, Subcommand)]
pub enum IndexAction {
    /// Create a vector index
    Create(IndexCreateArgs),
    /// Get index information
    Get(IndexGetArgs),
    /// Delete an index
    Delete(IndexGetArgs),
    /// List all indexes in a bucket
    List(IndexListArgs),
}

#[derive(Debug, Args)]
pub struct IndexCreateArgs {
    #[arg(long)]
    pub bucket: String,
    #[arg(long)]
    pub index_name: String,
    /// Dimension of vectors
    #[arg(long)]
    pub dimension: u32,
    /// Distance metric (cosine, euclidean, dot_product)
    #[arg(long, default_value = "cosine")]
    pub metric: String,
}

#[derive(Debug, Args)]
pub struct IndexGetArgs {
    #[arg(long)]
    pub bucket: String,
    #[arg(long)]
    pub index_name: String,
}

#[derive(Debug, Args)]
pub struct IndexListArgs {
    #[arg(long)]
    pub bucket: String,
}

// ============ Data ============
#[derive(Debug, Args)]
pub struct DataCommand {
    #[command(subcommand)]
    pub action: DataAction,
}

#[derive(Debug, Subcommand)]
pub enum DataAction {
    /// Upsert vector data
    Upsert(DataUpsertArgs),
    /// Get vector by ID
    Get(DataGetArgs),
    /// Delete vector by ID
    Delete(DataDeleteArgs),
    /// Search similar vectors
    Search(DataSearchArgs),
    /// List vectors
    List(DataListArgs),
}

#[derive(Debug, Args)]
pub struct DataUpsertArgs {
    #[arg(long)]
    pub bucket: String,
    #[arg(long)]
    pub index_name: String,
    /// Data JSON (inline or file://path)
    #[arg(long)]
    pub data: String,
}

#[derive(Debug, Args)]
pub struct DataGetArgs {
    #[arg(long)]
    pub bucket: String,
    #[arg(long)]
    pub index_name: String,
    #[arg(long)]
    pub id: String,
}

#[derive(Debug, Args)]
pub struct DataDeleteArgs {
    #[arg(long)]
    pub bucket: String,
    #[arg(long)]
    pub index_name: String,
    #[arg(long)]
    pub id: String,
}

#[derive(Debug, Args)]
pub struct DataSearchArgs {
    #[arg(long)]
    pub bucket: String,
    #[arg(long)]
    pub index_name: String,
    /// Query vector (JSON array)
    #[arg(long)]
    pub vector: String,
    /// Number of results
    #[arg(long, default_value = "10")]
    pub top_k: u32,
}

#[derive(Debug, Args)]
pub struct DataListArgs {
    #[arg(long)]
    pub bucket: String,
    #[arg(long)]
    pub index_name: String,
    #[arg(long)]
    pub page_token: Option<String>,
    #[arg(long, default_value = "100")]
    pub page_size: u32,
}
