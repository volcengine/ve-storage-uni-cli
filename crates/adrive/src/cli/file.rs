//! 文件资源 Low-Level 命令参数。
use clap::{Args, Subcommand};
use std::collections::HashMap;

use crate::cli::high_level::USERMETA_HELP;

/// 文件资源命令组；与 ve-tos `object` 一样承载核心文件 API。
#[derive(Debug, Args)]
#[command(
    about = "File core APIs",
    long_about = "Low-Level API — Core: file operations.",
    after_help = "Examples:\n  ve-adrive-cli file set-meta adrive://inst/space/docs/a.txt --meta 'owner=xsh#env=test'"
)]
pub struct FileCommand {
    #[command(subcommand)]
    pub action: Option<FileAction>,
}

#[derive(Debug, Subcommand)]
pub enum FileAction {
    /// Incrementally update file user metadata
    #[command(name = "set-meta", after_help = USERMETA_HELP)]
    SetMeta(SetMetaArgs),
}

impl FileAction {
    /// 返回公开的子命令名，用于 Envelope 和 Describe 路径。
    pub fn name(&self) -> &'static str {
        match self {
            Self::SetMeta(_) => "set-meta",
        }
    }
}

#[derive(Debug, Clone, Args)]
#[command(after_help = USERMETA_HELP)]
pub struct SetMetaArgs {
    /// Exact file URI: adrive://instance/space/path
    pub path: String,
    /// Resolve Instance and Space names to IDs
    #[arg(long)]
    pub by_name: bool,
    /// Incrementally set user metadata; omitted keys are preserved
    #[arg(long, aliases = ["usermeta", "metadata"], value_parser = crate::domain::metadata::parse_user_metadata)]
    pub meta: HashMap<String, String>,
}
