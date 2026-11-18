//! Space 文档片段检索与扩读参数。
use clap::{Args, Subcommand};

pub const CHUNK_HELP: &str = "Search and read indexed document chunks in one Space. Requires the selected OAuth user and file:read permission; never switches identity or changes indexing. Empty results do not prove indexing readiness. Use --text for search text; global --query remains a JMESPath output filter.";
pub const CHUNK_READ_HELP: &str = "Use the Instance, Space, FilePath, ETag and ordinals from the same search hit. Select exactly one of --ordinals or --range (inclusive); at most 20 nonnegative ordinals. A stale version may return 404; search again instead of substituting a version. ChunkTruncated means source content was not fully indexed; reading neighbours cannot recover it.";

#[derive(Debug, Args)]
#[command(about = "Search and read document chunks", after_help = CHUNK_HELP)]
pub struct ChunkCommand {
    #[command(subcommand)]
    pub action: Option<ChunkAction>,
}

#[derive(Debug, Subcommand)]
pub enum ChunkAction {
    /// Search document chunks in a Space directory
    #[command(after_help = CHUNK_HELP)]
    Search(SearchChunkArgs),
    /// Read indexed chunks of one file version
    #[command(after_help = CHUNK_READ_HELP)]
    Read(ReadChunkArgs),
}

impl ChunkAction {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Search(_) => "search",
            Self::Read(_) => "read",
        }
    }
}

#[derive(Debug, Args)]
pub struct ChunkTargetArgs {
    /// ADrive URI with Instance and Space IDs; conflicts with explicit selectors
    #[arg(conflicts_with_all = ["instance", "space"])]
    pub target: Option<String>,
    /// Instance ID; required with explicit selectors
    #[arg(long)]
    pub instance: Option<String>,
    /// Space ID; required with explicit selectors
    #[arg(long)]
    pub space: Option<String>,
}

#[derive(Debug, Args)]
pub struct SearchChunkArgs {
    #[command(flatten)]
    pub selector: ChunkTargetArgs,
    /// Directory inside the Space; defaults to the root
    #[arg(long, conflicts_with = "target")]
    pub folder: Option<String>,
    /// Search text; nonempty after trimming, at most 100 UTF-8 bytes
    #[arg(long)]
    pub text: Option<String>,
    /// Maximum returned chunks, 1 through 100
    #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u32).range(1..=100))]
    pub limit: u32,
    /// Search only direct children; by default include subdirectories
    #[arg(long)]
    pub no_recursive: bool,
}

#[derive(Debug, Args)]
pub struct ReadChunkArgs {
    #[command(flatten)]
    pub selector: ChunkTargetArgs,
    /// Exact file path inside the Space
    #[arg(long, conflicts_with = "target")]
    pub file: Option<String>,
    /// File version from the same search hit, passed back unchanged
    #[arg(long)]
    pub etag: Option<String>,
    /// Comma-separated nonnegative chunk ordinals, at most 20 without duplicates
    #[arg(long, value_delimiter = ',', conflicts_with = "range")]
    pub ordinals: Vec<u64>,
    /// Inclusive ordinal range START-END, at most 20 chunks
    #[arg(long, conflicts_with = "ordinals")]
    pub range: Option<String>,
}
