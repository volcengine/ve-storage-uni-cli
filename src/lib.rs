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

#![recursion_limit = "256"]

use clap::{error::ErrorKind as ClapErrorKind, Parser, Subcommand};
use std::fmt::Write as _;
use tos_core::agent::global_args::GlobalArgs;
use tos_core::infra::client::USER_AGENT_NAME_ENV;

const TOS_EXAMPLE_PREFIX_ENV: &str = "VE_STORAGE_UNI_TOS_EXAMPLE_PREFIX";
const BYTED_TOS_EXAMPLE_PREFIX_ENV: &str = "VE_STORAGE_UNI_BYTED_TOS_EXAMPLE_PREFIX";
const ADRIVE_EXAMPLE_PREFIX_ENV: &str = "VE_STORAGE_UNI_ADRIVE_EXAMPLE_PREFIX";
const TOS_CONFIG_BINARY_ENV: &str = "VE_STORAGE_UNI_TOS_CONFIG_BINARY";

#[derive(Clone, Copy)]
enum InvocationSurface {
    Unified,
    VeTosDirect,
    BytedTosDirect,
    ADriveDirect,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HelpLanguage {
    En,
    Zh,
}

const HELP_LANGUAGE_HINT_EN: &str =
    "\nLanguage:\n  --language <en|zh>      Help output language, e.g. --help --language zh\n";
const HELP_LANGUAGE_ALIAS_HINT_EN: &str =
    "\nLanguage:\n  --help-language <en|zh> Help output language, e.g. --help --help-language zh\n";

#[derive(Parser)]
#[command(
    name = "ve-storage-uni-cli",
    version,
    about = "Agent-Native CLI for Volcengine storage services",
    long_about = "Volcengine Storage Unified CLI — agent-native command-line interface for storage tools.\n\n\
        Usage:\n  \
        ve-storage-uni-cli tos <command>          ByteCloud TOS Object Storage\n  \
        ve-storage-uni-cli ve-tos <command>       TOS Object Storage\n  \
        ve-storage-uni-cli ve-adrive <command>    A-Drive\n\n\
        Tip: use the dedicated tos-cli, ve-tos-cli, and ve-adrive-cli binaries for direct invocation.",
    after_help = "Language:\n  --language <en|zh>      Help output language, e.g. --help --language zh"
)]
struct Cli {
    #[command(flatten)]
    global: GlobalArgs,

    #[command(subcommand)]
    tool: ToolCommand,
}

#[derive(Subcommand)]
enum ToolCommand {
    /// ByteCloud TOS commands (high-level + utilities)
    #[command(name = "tos")]
    TosCli {
        #[command(subcommand)]
        command: tos_cli::TosCliCommand,
    },
    // `ve-tos` remains the original Volcengine TOS surface; bare `tos` above is
    // the new ByteCloud TOS high-level surface.
    /// TOS Object Storage commands (high-level + low-level + utilities)
    #[command(name = "ve-tos")]
    Tos {
        #[command(subcommand)]
        command: Option<ve_tos_cli::TosCommand>,
    },
    /// A-Drive commands (high-level + utilities)
    #[command(name = "ve-adrive")]
    ADrive {
        #[command(subcommand)]
        command: ve_adrive_cli::ADriveCommand,
    },
}

fn normalize_help_aliases(args: &[String]) -> Vec<String> {
    if args.len() > 1 && args[1] == "help" {
        let mut normalized = args[..1].to_vec();
        normalized.extend(args[2..].iter().cloned());
        // [Review Fix #RootHelpLanguage] Root-level `help <path>` is a help
        // alias, so normalize it before language-aware help interception.
        normalized.push("--help".to_string());
        return normalized;
    }
    if args.len() > 2 && args[2] == "help" {
        let mut normalized = args[..2].to_vec();
        normalized.extend(args[3..].iter().cloned());
        normalized.push("--help".to_string());
        return normalized;
    }
    args.to_vec()
}

fn normalize_leading_help_flag(args: &[String]) -> Vec<String> {
    let Some(help_idx) = args
        .iter()
        .position(|arg| matches!(arg.as_str(), "--help" | "-h"))
    else {
        return args.to_vec();
    };
    if help_idx == 0 || help_idx > 2 {
        return args.to_vec();
    }

    let mut command_tokens = Vec::new();
    let mut deferred_flags = Vec::new();
    let mut index = help_idx + 1;
    while index < args.len() {
        let arg = args[index].as_str();
        if arg.starts_with('-') {
            deferred_flags.push(args[index].clone());
            if flag_takes_value(arg) {
                if let Some(value) = args.get(index + 1) {
                    deferred_flags.push(value.clone());
                }
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }
        command_tokens.push(args[index].clone());
        index += 1;
    }

    if command_tokens.is_empty() {
        return args.to_vec();
    }

    let mut normalized = args[..help_idx].to_vec();
    normalized.extend(command_tokens);
    normalized.push(args[help_idx].clone());
    normalized.extend(deferred_flags);
    normalized
}

/// Check whether the effective argument list represents a `ve-tos` help request
/// or a bare `ve-tos` invocation with no subcommand.  In either case we print the
/// grouped help text and exit.
fn maybe_print_tos_grouped_help(effective_args: &[String]) -> bool {
    let tos_idx = match tool_position(effective_args, "ve-tos") {
        Some(idx) => idx,
        None => return false,
    };

    let after_tos: Vec<&String> = effective_args[(tos_idx + 1)..].iter().collect();

    if after_tos.is_empty() {
        return true;
    }

    if after_tos.len() == 1 {
        let arg = after_tos[0].as_str();
        if arg == "--help" || arg == "-h" || arg == "help" {
            return true;
        }
    }

    false
}

/// Check whether the effective argument list represents a `tos` help request
/// or a bare `tos` invocation. If so, print ByteCloud TOS grouped help.
fn maybe_print_byted_tos_grouped_help(effective_args: &[String]) -> bool {
    let tos_idx = match tool_position(effective_args, "byted-tos") {
        Some(idx) => idx,
        None => return false,
    };

    let after_tos: Vec<&String> = effective_args[(tos_idx + 1)..].iter().collect();
    if after_tos.is_empty() {
        return true;
    }
    if after_tos.len() == 1 {
        let arg = after_tos[0].as_str();
        return arg == "--help" || arg == "-h" || arg == "help";
    }
    false
}

/// Check whether the effective argument list represents a `ve-adrive` help
/// request or a bare `ve-adrive` invocation. If so, print grouped help.
fn maybe_print_adrive_grouped_help(effective_args: &[String]) -> bool {
    let adrive_idx = match tool_position(effective_args, "ve-adrive") {
        Some(idx) => idx,
        None => return false,
    };

    let after_adrive: Vec<&String> = effective_args[(adrive_idx + 1)..].iter().collect();
    if after_adrive.is_empty() {
        return true;
    }
    if after_adrive.len() == 1 {
        let arg = after_adrive[0].as_str();
        return arg == "--help" || arg == "-h" || arg == "help";
    }
    false
}

fn maybe_print_language_help(effective_args: &[String]) -> bool {
    if !is_help_request(effective_args) {
        return false;
    }
    let Some(language) = requested_help_language(effective_args) else {
        if has_help_language_arg(effective_args) {
            // [Review Fix #HelpZh1] Help language is a user-facing parameter;
            // reject unsupported or missing values instead of silently falling
            // back to English help.
            exit_help_language_error("unsupported or missing --language value; expected en or zh");
        }
        return false;
    };
    match language {
        HelpLanguage::En => print_english_help_without_language(effective_args),
        HelpLanguage::Zh => print_chinese_help(effective_args),
    }
    true
}

fn is_help_request(effective_args: &[String]) -> bool {
    effective_args
        .iter()
        .skip(1)
        .any(|arg| matches!(arg.as_str(), "--help" | "-h"))
}

fn requested_help_language(effective_args: &[String]) -> Option<HelpLanguage> {
    let mut index = 1;
    while index < effective_args.len() {
        let arg = effective_args[index].as_str();
        if let Some(value) = arg
            .strip_prefix("--language=")
            .or_else(|| arg.strip_prefix("--help-language="))
        {
            return parse_help_language(value);
        }
        if matches!(arg, "--language" | "--help-language") {
            return effective_args
                .get(index + 1)
                .and_then(|value| parse_help_language(value));
        }
        index += 1;
    }
    None
}

fn has_help_language_arg(effective_args: &[String]) -> bool {
    effective_args.iter().skip(1).any(|arg| {
        matches!(arg.as_str(), "--language" | "--help-language")
            || arg.starts_with("--language=")
            || arg.starts_with("--help-language=")
    })
}

fn parse_help_language(value: &str) -> Option<HelpLanguage> {
    match value.to_ascii_lowercase().as_str() {
        "en" | "en-us" | "english" => Some(HelpLanguage::En),
        "zh" | "zh-cn" | "cn" | "chinese" => Some(HelpLanguage::Zh),
        _ => None,
    }
}

fn print_english_help_without_language(effective_args: &[String]) {
    let sanitized_args = args_without_help_language(effective_args);
    if maybe_print_byted_tos_grouped_help(&sanitized_args) {
        tos_cli::print_grouped_help();
        return;
    }
    if maybe_print_tos_grouped_help(&sanitized_args) {
        ve_tos_cli::print_grouped_help();
        return;
    }
    if maybe_print_adrive_grouped_help(&sanitized_args) {
        ve_adrive_cli::print_grouped_help();
        return;
    }
    match Cli::try_parse_from(&sanitized_args) {
        Err(err) if matches!(err.kind(), ClapErrorKind::DisplayHelp) => {
            print_display_help_with_registry_examples(&sanitized_args, &err);
        }
        Err(err) => {
            // [Review Fix #HelpZh2] Sanitizing --language en must not turn
            // invalid command help into a successful no-op; preserve clap's
            // original non-zero parse behavior.
            err.exit();
        }
        Ok(_) => {}
    }
}

fn display_help_text_for_args(effective_args: &[String]) -> Result<String, clap::Error> {
    match Cli::try_parse_from(effective_args) {
        Err(err) if matches!(err.kind(), ClapErrorKind::DisplayHelp) => Ok(
            display_help_text_with_registry_examples(effective_args, &err),
        ),
        Err(err) => Err(err),
        Ok(_) => Ok(String::new()),
    }
}

fn localize_clap_help_zh(help: &str) -> String {
    let localized = translate_help_phrases_zh(&help)
        .replace("Usage:", "用法:")
        .replace("Arguments:", "参数:")
        .replace("Options:", "选项:")
        .replace("Commands:", "命令:")
        .replace("Language:", "语言:")
        .replace("Examples:", "示例:")
        .replace("Install examples:", "安装示例:")
        .replace("Supported KEY values:", "支持的 KEY 值:")
        .replace("MCP usage:", "MCP 用法:")
        .replace("Notes:", "备注:")
        .replace("Possible values:", "可选值:")
        .replace(
            "Print help (see a summary with '-h')",
            "显示帮助（使用 '-h' 查看摘要）",
        )
        .replace(
            "Print this message or the help of the given subcommand(s)",
            "显示此消息或指定子命令的帮助",
        )
        .replace(
            "Help output language, e.g. --help --language zh",
            "帮助输出语言，例如 --help --language zh",
        )
        .replace(
            "Help output language, e.g. --help --help-language zh",
            "帮助输出语言，例如 --help --help-language zh",
        )
        .replace("Print help", "显示帮助")
        .replace("Print version", "显示版本")
        .replace("[default:", "[默认:")
        .replace("[env:", "[环境变量:")
        .replace("[possible values:", "[可选值:");
    let Some((summary, rest)) = localized.split_once("\n\n") else {
        return localized;
    };
    if matches!(summary, "用法:" | "参数:" | "选项:" | "命令:" | "示例:") {
        localized
    } else {
        format!("说明:\n  {summary}\n\n{rest}")
    }
}

fn translate_help_phrases_zh(text: &str) -> String {
    let mut translated = text.to_string();
    for (english, chinese) in HELP_TRANSLATIONS_ZH {
        translated = translated.replace(english, chinese);
    }
    translated
}

const HELP_TRANSLATIONS_ZH: &[(&str, &str)] = &[
    ("ByteCloud TOS PSM service name.", "PSM 服务名。"),
    (
        "CLI flag only. Supported by the `tos` command surface. When omitted, `--idc`, `--cluster`, and `--addr-family` do not enable PSM mode by themselves.",
        "仅 CLI 参数。仅 tos 命令支持。未设置时，`--idc`、`--cluster` 和 `--addr-family` 不会单独启用 PSM 模式。",
    ),
    (
        "IDC used with PSM service discovery",
        "与 PSM 服务发现配合使用的 IDC",
    ),
    (
        "Cluster used with PSM service discovery",
        "与 PSM 服务发现配合使用的集群",
    ),
    (
        "Address family used with PSM service discovery: v4, v6, or dual-stack",
        "与 PSM 服务发现配合使用的地址族：v4、v6 或 dual-stack",
    ),
    ("Generate presigned URLs", "生成预签名 URL"),
    (
        "Copy local files, TOS objects, or prefixes",
        "复制本地文件、TOS 对象或前缀",
    ),
    (
        "Copy local files, objects, or prefixes between local and TOS.",
        "在本地与 TOS 之间复制本地文件、对象或前缀。",
    ),
    (
        "Move files or objects by copy plus source delete",
        "通过复制并删除源文件/对象来移动",
    ),
    (
        "Synchronize source and destination incrementally",
        "增量同步源和目标",
    ),
    ("Create a bucket", "创建 Bucket"),
    ("Remove a bucket", "删除 Bucket"),
    ("Create a folder marker", "创建文件夹标记"),
    ("Create a folder", "创建文件夹"),
    ("Delete objects or prefixes", "删除对象或前缀"),
    ("List buckets or objects", "列出 Bucket 或对象"),
    (
        "List object prefixes or objects within a bucket",
        "列出 Bucket 内的对象前缀或对象",
    ),
    ("Show bucket or object metadata", "查看 Bucket 或对象元数据"),
    (
        "Calculate object size statistics for a prefix",
        "统计前缀下的对象大小",
    ),
    ("Calculate size statistics for a prefix", "统计前缀大小"),
    (
        "Find objects by name, size, mtime, or storage class",
        "按名称、大小、修改时间或存储类型查找对象",
    ),
    (
        "Find objects by name, size, or mtime",
        "按名称、大小或修改时间查找对象",
    ),
    ("Find objects by filters", "按条件查找对象"),
    ("Stream object content", "流式输出对象内容"),
    ("Upload stdin to an object", "将标准输入上传为对象"),
    ("Generate presigned URL", "生成预签名 URL"),
    ("Generate a presigned URL", "生成预签名 URL"),
    ("Restore archived objects", "恢复归档对象"),
    ("Restore archived object", "恢复归档对象"),
    (
        "Source path (local path or tos://bucket/key)",
        "源路径（本地路径或 tos://bucket/key）",
    ),
    (
        "Destination path (local path or tos://bucket/key)",
        "目标路径（本地路径或 tos://bucket/key）",
    ),
    (
        "Source path (local path or adrive://instance/space/folder/file)",
        "源路径（本地路径或 adrive://instance/space/folder/file）",
    ),
    (
        "Destination path (local path or adrive://instance/space/folder/file)",
        "目标路径（本地路径或 adrive://instance/space/folder/file）",
    ),
    ("Source path", "源路径"),
    ("Destination path", "目标路径"),
    ("Recursive copy", "递归复制"),
    (
        "Recursive move for directories or prefixes",
        "递归移动目录或前缀",
    ),
    ("Recursive move for directories", "递归移动目录"),
    (
        "Recursive delete strategy for HNS buckets",
        "HNS Bucket 的递归删除策略",
    ),
    ("Recursive delete", "递归删除"),
    (
        "Include the source directory/prefix name under the destination prefix",
        "在目标前缀下包含源目录/前缀名称",
    ),
    (
        "Include the source directory or prefix name under the destination path",
        "在目标路径下包含源目录或前缀名称",
    ),
    (
        "Include the source directory/prefix name under the destination path",
        "在目标路径下包含源目录/前缀名称",
    ),
    (
        "Include pattern for recursive restore",
        "递归恢复的包含匹配模式",
    ),
    (
        "Exclude pattern for recursive restore",
        "递归恢复的排除匹配模式",
    ),
    (
        "Include pattern for bottom-up recursive deletes",
        "自底向上递归删除的包含匹配模式",
    ),
    (
        "Exclude pattern for bottom-up recursive deletes",
        "自底向上递归删除的排除匹配模式",
    ),
    ("Include pattern", "包含匹配模式"),
    ("Exclude pattern", "排除匹配模式"),
    (
        "Enable checkpoint for resumable transfer",
        "启用断点续传 checkpoint",
    ),
    (
        "Enable resumable transfer checkpoint",
        "启用断点续传 checkpoint",
    ),
    ("Checkpoint directory override", "覆盖 checkpoint 目录"),
    (
        "Directory for transfer checkpoint state",
        "传输 checkpoint 状态目录",
    ),
    (
        "Directory reserved for transfer checkpoint state",
        "为传输 checkpoint 状态保留的目录",
    ),
    (
        "File size threshold for checkpoint multipart/range transfer (e.g., 20MB)",
        "触发 checkpoint 分片/范围传输的文件大小阈值（例如 20MB）",
    ),
    (
        "Stdin size threshold for switching to multipart upload (e.g., 20MB)",
        "标准输入切换到分片上传的大小阈值（例如 20MB）",
    ),
    (
        "Content-Type for TOS uploads/copies",
        "TOS 上传/复制使用的 Content-Type",
    ),
    (
        "Content-Type for the uploaded object",
        "上传对象使用的 Content-Type",
    ),
    (
        "Content-Type for the uploaded file",
        "上传文件使用的 Content-Type",
    ),
    (
        "Content-Type for uploaded stdin",
        "上传标准输入使用的 Content-Type",
    ),
    (
        "Target storage class for TOS uploads/copies. Allowed: STANDARD, IA, ARCHIVE_FR, INTELLIGENT_TIERING, COLD_ARCHIVE, ARCHIVE, DEEP_COLD_ARCHIVE",
        "TOS 上传/复制的目标存储类型。允许值：STANDARD、IA、ARCHIVE_FR、INTELLIGENT_TIERING、COLD_ARCHIVE、ARCHIVE、DEEP_COLD_ARCHIVE",
    ),
    (
        "Target object storage class. Allowed: STANDARD, IA, ARCHIVE_FR, INTELLIGENT_TIERING, COLD_ARCHIVE, ARCHIVE, DEEP_COLD_ARCHIVE",
        "目标对象存储类型。允许值：STANDARD、IA、ARCHIVE_FR、INTELLIGENT_TIERING、COLD_ARCHIVE、ARCHIVE、DEEP_COLD_ARCHIVE",
    ),
    (
        "Target storage class. Allowed: STANDARD, IA, ARCHIVE_FR, INTELLIGENT_TIERING, COLD_ARCHIVE, ARCHIVE, DEEP_COLD_ARCHIVE",
        "目标存储类型。允许值：STANDARD、IA、ARCHIVE_FR、INTELLIGENT_TIERING、COLD_ARCHIVE、ARCHIVE、DEEP_COLD_ARCHIVE",
    ),
    (
        "Storage class. Allowed: STANDARD, IA, ARCHIVE_FR, INTELLIGENT_TIERING, COLD_ARCHIVE, ARCHIVE, DEEP_COLD_ARCHIVE",
        "存储类型。允许值：STANDARD、IA、ARCHIVE_FR、INTELLIGENT_TIERING、COLD_ARCHIVE、ARCHIVE、DEEP_COLD_ARCHIVE",
    ),
    (
        "Bucket storage class. Allowed: STANDARD, IA, ARCHIVE_FR, INTELLIGENT_TIERING, COLD_ARCHIVE, ARCHIVE, DEEP_COLD_ARCHIVE",
        "Bucket 存储类型。允许值：STANDARD、IA、ARCHIVE_FR、INTELLIGENT_TIERING、COLD_ARCHIVE、ARCHIVE、DEEP_COLD_ARCHIVE",
    ),
    (
        "Target object ACL for TOS uploads/copies. Allowed: private, public-read, public-read-write, authenticated-read, bucket-owner-read, bucket-owner-full-control, bucket-owner-entrusted, default",
        "TOS 上传/复制的目标对象 ACL。允许值：private、public-read、public-read-write、authenticated-read、bucket-owner-read、bucket-owner-full-control、bucket-owner-entrusted、default",
    ),
    (
        "Target object ACL. Allowed: private, public-read, public-read-write, authenticated-read, bucket-owner-read, bucket-owner-full-control, bucket-owner-entrusted, default",
        "目标对象 ACL。允许值：private、public-read、public-read-write、authenticated-read、bucket-owner-read、bucket-owner-full-control、bucket-owner-entrusted、default",
    ),
    (
        "Bucket ACL. Allowed: private, public-read, public-read-write, authenticated-read, bucket-owner-read, bucket-owner-full-control",
        "Bucket ACL。允许值：private、public-read、public-read-write、authenticated-read、bucket-owner-read、bucket-owner-full-control",
    ),
    (
        "Custom TOS metadata as key=value#key2=value2; writes x-tos-meta-* headers",
        "自定义 TOS 元数据，格式为 key=value#key2=value2；会写入 x-tos-meta-* 头",
    ),
    (
        "Maximum files/items running concurrently in this batch delete",
        "本次批量删除中最大并发文件/条目数",
    ),
    (
        "Maximum files/items running concurrently in this batch restore",
        "本次批量恢复中最大并发文件/条目数",
    ),
    (
        "Maximum files/items running concurrently in batch commands",
        "批量命令中最大并发文件/条目数",
    ),
    (
        "Maximum entries to return from the current directory level",
        "当前目录层级最多返回的条目数",
    ),
    (
        "Maximum prefixes listed concurrently when recursive listing uses delimiter=\"/\"",
        "递归列举使用 delimiter=\"/\" 时最大并发列举前缀数",
    ),
    (
        "Maximum prefixes listed concurrently when the bucket is listed hierarchically",
        "按层级列举 Bucket 时最大并发列举前缀数",
    ),
    (
        "Maximum folder prefixes listed concurrently in recursive batch commands",
        "递归批量命令中最大并发列举文件夹前缀数",
    ),
    (
        "Maximum folder prefixes listed concurrently in recursive batch deletes",
        "递归批量删除中最大并发列举文件夹前缀数",
    ),
    (
        "Maximum folder prefixes listed concurrently while measuring recursively",
        "递归统计时最大并发列举文件夹前缀数",
    ),
    (
        "Maximum parts/ranges running concurrently for one large file",
        "单个大文件最大并发分片/范围数",
    ),
    (
        "Recursive listing mode: auto, flat, or hierarchical",
        "递归列举模式：auto、flat 或 hierarchical",
    ),
    (
        "Use bucket defaults: HNS lists with delimiter=\"/\"; FNS lists flat",
        "使用 Bucket 默认策略：HNS 使用 delimiter=\"/\" 分层列举；FNS 平铺列举",
    ),
    (
        "List recursively with delimiter=\"\"",
        "使用 delimiter=\"\" 递归平铺列举",
    ),
    (
        "List recursively by prefix with delimiter=\"/\"",
        "使用 delimiter=\"/\" 按前缀递归列举",
    ),
    (
        "Progress granularity: part (default) or byte",
        "进度粒度：part（默认）或 byte",
    ),
    ("Count completed transfer parts", "统计已完成的传输分片数"),
    ("Count transferred bytes", "统计已传输字节数"),
    ("Destination overwrite strategy", "目标覆盖策略"),
    (
        "Always overwrite destination when the operation supports it",
        "操作支持时始终覆盖目标",
    ),
    (
        "Do not overwrite existing destination",
        "不覆盖已存在目标",
    ),
    (
        "Overwrite only when source timestamp is newer than destination",
        "仅当源时间戳新于目标时覆盖",
    ),
    (
        "Write batch success/failure report to this path",
        "将批量成功/失败报告写入此路径",
    ),
    (
        "Write only failed items to the batch report",
        "批量报告中仅写入失败条目",
    ),
    (
        "Write planned transfer manifest to this path",
        "将计划传输 manifest 写入此路径",
    ),
    (
        "Write planned delete manifest to this path",
        "将计划删除 manifest 写入此路径",
    ),
    (
        "Write planned restore manifest to this path",
        "将计划恢复 manifest 写入此路径",
    ),
    (
        "Do not write a planned transfer manifest",
        "不写入计划传输 manifest",
    ),
    (
        "Do not write a planned delete manifest",
        "不写入计划删除 manifest",
    ),
    (
        "Do not write a planned restore manifest",
        "不写入计划恢复 manifest",
    ),
    ("Bandwidth limit (e.g., 100MB)", "带宽限制（例如 100MB）"),
    ("Bandwidth limit", "带宽限制"),
    (
        "Force overwrite without confirmation",
        "无需确认强制覆盖",
    ),
    (
        "Force overwrite/delete confirmation",
        "强制确认覆盖/删除",
    ),
    (
        "Force delete without confirmation",
        "无需确认强制删除",
    ),
    (
        "Do not overwrite existing objects (sets if-none-match: *)",
        "不覆盖已存在对象（设置 if-none-match: *）",
    ),
    (
        "Do not overwrite an existing object",
        "不覆盖已存在对象",
    ),
    (
        "Do not overwrite existing files",
        "不覆盖已存在文件",
    ),
    (
        "Do not overwrite an existing file",
        "不覆盖已存在文件",
    ),
    (
        "Delete extraneous files/folders from destination",
        "删除目标端多余的文件/文件夹",
    ),
    (
        "Delete extraneous destination files/folders",
        "删除目标端多余的文件/文件夹",
    ),
    (
        "Delete extraneous files from destination",
        "删除目标端多余的文件",
    ),
    (
        "Confirm deletion when --delete is enabled",
        "启用 --delete 时确认删除",
    ),
    (
        "Compare by size only (skip mtime)",
        "仅按大小比较（跳过 mtime）",
    ),
    (
        "Use exact timestamps for comparison",
        "使用精确时间戳比较",
    ),
    ("Bucket Core APIs", "Bucket 核心 API"),
    ("Bucket core APIs", "Bucket 核心 API"),
    ("Object Core APIs", "Object 核心 API"),
    ("Object core APIs", "Object 核心 API"),
    ("Multipart Core APIs", "分片核心 API"),
    ("Multipart upload core APIs", "分片上传核心 API"),
    ("Turbo append upload APIs", "Turbo 追加上传 API"),
    ("Turbo core APIs", "Turbo 核心 API"),
    ("Bucket storage quota", "Bucket 存储配额"),
    ("Bucket policy management", "Bucket 策略管理"),
    ("Lifecycle rule management", "生命周期规则管理"),
    ("Bucket default storage class", "Bucket 默认存储类型"),
    ("Bucket CORS configuration", "Bucket CORS 配置"),
    ("CORS configuration", "CORS 配置"),
    ("Bucket versioning configuration", "Bucket 版本控制配置"),
    ("Versioning configuration", "版本控制配置"),
    ("Cross-region replication", "跨区域复制"),
    ("Bucket encryption configuration", "Bucket 加密配置"),
    ("Server-side encryption configuration", "服务端加密配置"),
    ("Custom domain binding", "自定义域名绑定"),
    ("Event notification configuration", "事件通知配置"),
    ("Static website hosting", "静态网站托管"),
    ("Mirror back-to-source rules", "镜像回源规则"),
    ("Bucket inventory configuration", "Bucket 清单配置"),
    ("Bucket tagging management", "Bucket 标签管理"),
    ("Bucket ACL management", "Bucket ACL 管理"),
    ("Bucket rename configuration", "Bucket 重命名配置"),
    ("Real-time log analysis", "实时日志分析"),
    ("Access monitoring configuration", "访问监控配置"),
    ("WORM / object lock configuration", "WORM / 对象锁配置"),
    ("Bucket trash configuration", "Bucket 回收站配置"),
    ("Requester pays configuration", "请求者付费配置"),
    ("Access log storage configuration", "访问日志存储配置"),
    ("Bucket RenameObject configuration", "Bucket RenameObject 配置"),
    ("Intelligent tiering configuration", "智能分层配置"),
    ("Transfer acceleration configuration", "传输加速配置"),
    ("CDN notification configuration", "CDN 通知配置"),
    ("HTTPS/TLS configuration", "HTTPS/TLS 配置"),
    ("Pay-by-traffic configuration", "按流量计费配置"),
    ("Max-age cache configuration", "Max-age 缓存配置"),
    ("Data redundancy transition", "数据冗余转换"),
    (
        "Data processing (image styles, workflows, audits)",
        "数据处理（图片样式、工作流、审计）",
    ),
    ("Advanced data processing APIs", "高级数据处理 API"),
    ("Object set management", "对象集合管理"),
    ("Advanced object set APIs", "高级对象集合 API"),
    ("Accelerator management", "加速器管理"),
    ("Advanced accelerator control APIs", "高级加速器控制 API"),
    ("Multi-region access point", "多区域接入点"),
    ("多区域接入点 APIs", "多区域接入点 API"),
    ("Access point management", "接入点管理"),
    ("Access point APIs", "接入点 API"),
    ("Converged access point", "融合接入点"),
    ("融合接入点 APIs", "融合接入点 API"),
    (
        "Intelligent retrieval / dataset management",
        "智能检索 / 数据集管理",
    ),
    ("Intelligent retrieval dataset APIs", "智能检索数据集 API"),
    ("Control plane operations", "控制面操作"),
    ("Advanced control APIs", "高级控制 API"),
    ("Discover CLI capabilities", "发现 CLI 能力"),
    ("Inspect API metadata", "查看 API 元数据"),
    ("Raw API passthrough", "原始 API 透传"),
    (
        "Guarded API metadata and dry-run planning utility",
        "带保护的 API 元数据与 dry-run 计划工具",
    ),
    ("Configuration management", "配置管理"),
    ("Generate shell completion", "生成 shell 补全"),
    ("Start MCP server", "启动 MCP 服务器"),
    ("Start or plan MCP serving", "启动或规划 MCP 服务"),
    ("Manage/export skill metadata", "管理/导出 Skill 元数据"),
    (
        "List TOS skill metadata or export Markdown SKILL.md files",
        "列出 TOS Skill 元数据或导出 Markdown SKILL.md 文件",
    ),
    ("Environment diagnostics", "环境诊断"),
    ("Create a new bucket", "创建新 Bucket"),
    ("Get bucket metadata (HeadBucket)", "获取 Bucket 元数据（HeadBucket）"),
    ("Delete a bucket", "删除 Bucket"),
    ("List all buckets", "列出所有 Bucket"),
    ("Get bucket statistics", "获取 Bucket 统计信息"),
    ("Get bucket detailed information", "获取 Bucket 详细信息"),
    ("Get bucket location", "获取 Bucket 位置"),
    ("List objects (ListObjectsV2)", "列出对象（ListObjectsV2）"),
    (
        "Object list URI (tos://bucket or tos://bucket/prefix/)",
        "对象列举 URI（tos://bucket 或 tos://bucket/prefix/）",
    ),
    ("Object prefix", "对象前缀"),
    ("Delimiter", "分隔符"),
    ("Maximum keys per response", "单次响应最大 key 数"),
    ("Continuation token", "Continuation token"),
    (
        "View: groups (default — group summary with command counts), text (one-line summaries: `<command>\\t<description>`), compact (capability rows without parameters), full (capability rows + parameters + command tree). `tree` is accepted as a legacy alias for `compact`",
        "视图：groups（默认，按分组汇总命令数量）、text（单行摘要：`<command>\\t<description>`）、compact（不含参数的能力行）、full（能力行 + 参数 + 命令树）。`tree` 作为兼容别名等同于 `compact`",
    ),
    ("Filter by command group", "按命令分组过滤"),
    ("Search keywords", "搜索关键词"),
    ("Filter by layer", "按层级过滤"),
    (
        "Path to list (tos://bucket or tos://bucket/prefix/)",
        "要列出的路径（tos://bucket 或 tos://bucket/prefix/）",
    ),
    (
        "Path to list (adrive://instance/space or adrive://instance/space/folder/)",
        "要列出的路径（adrive://instance/space 或 adrive://instance/space/folder/）",
    ),
    (
        "Target path (tos://bucket/key or tos://bucket/prefix/)",
        "目标路径（tos://bucket/key 或 tos://bucket/prefix/）",
    ),
    (
        "Target path (adrive://instance/space/folder/file or adrive://instance/space/folder/)",
        "目标路径（adrive://instance/space/folder/file 或 adrive://instance/space/folder/）",
    ),
    ("Bucket URI (tos://bucket)", "Bucket URI（tos://bucket）"),
    (
        "Folder path (tos://bucket/folder/)",
        "文件夹路径（tos://bucket/folder/）",
    ),
    (
        "Folder path (adrive://instance/space/folder/subfolder)",
        "文件夹路径（adrive://instance/space/folder/subfolder）",
    ),
    (
        "Path to inspect (tos://bucket or tos://bucket/key)",
        "要查看的路径（tos://bucket 或 tos://bucket/key）",
    ),
    (
        "Path to inspect (adrive://instance/space/folder/file)",
        "要查看的路径（adrive://instance/space/folder/file）",
    ),
    (
        "Path to measure (tos://bucket or tos://bucket/prefix/)",
        "要统计的路径（tos://bucket 或 tos://bucket/prefix/）",
    ),
    (
        "Path to measure (adrive://instance/space or adrive://instance/space/folder/)",
        "要统计的路径（adrive://instance/space 或 adrive://instance/space/folder/）",
    ),
    (
        "Search path (tos://bucket or tos://bucket/prefix/)",
        "搜索路径（tos://bucket 或 tos://bucket/prefix/）",
    ),
    (
        "Search path (adrive://instance/space or adrive://instance/space/folder/)",
        "搜索路径（adrive://instance/space 或 adrive://instance/space/folder/）",
    ),
    ("Object path (tos://bucket/key)", "对象路径（tos://bucket/key）"),
    (
        "Object path to write (tos://bucket/key)",
        "要写入的对象路径（tos://bucket/key）",
    ),
    (
        "File path (adrive://instance/space/folder/file)",
        "文件路径（adrive://instance/space/folder/file）",
    ),
    (
        "File path to write (adrive://instance/space/folder/file)",
        "要写入的文件路径（adrive://instance/space/folder/file）",
    ),
    (
        "Archived object path or prefix (tos://bucket/key or tos://bucket/prefix/)",
        "归档对象路径或前缀（tos://bucket/key 或 tos://bucket/prefix/）",
    ),
    (
        "Resource to create (adrive://instance-name or adrive://instance-id/space-name)",
        "要创建的资源（adrive://instance-name 或 adrive://instance-id/space-name）",
    ),
    (
        "Resource to delete (adrive://instance-id or adrive://instance-id/space-id)",
        "要删除的资源（adrive://instance-id 或 adrive://instance-id/space-id）",
    ),
    (
        "Bucket name (alternative to positional URI)",
        "Bucket 名称（位置 URI 的替代写法）",
    ),
    ("Bucket name", "Bucket 名称"),
    (
        "Instance name (alternative to positional URI)",
        "实例名称（位置 URI 的替代写法）",
    ),
    ("ADrive instance identifier", "ADrive 实例标识"),
    ("ADrive space identifier", "ADrive 空间标识"),
    (
        "Space name (used with --instance)",
        "空间名称（与 --instance 搭配使用）",
    ),
    (
        "Folder path (used with --instance --space)",
        "文件夹路径（与 --instance --space 搭配使用）",
    ),
    (
        "Folder path to create (used with --instance --space)",
        "要创建的文件夹路径（与 --instance --space 搭配使用）",
    ),
    (
        "File name (used with --instance --space --folder)",
        "文件名（与 --instance --space --folder 搭配使用）",
    ),
    ("Configuration profile name", "配置 profile 名称"),
    ("Key prefix (used with --bucket)", "对象 Key 前缀（与 --bucket 搭配使用）"),
    ("Object key (used with --bucket)", "对象 Key（与 --bucket 搭配使用）"),
    (
        "Object key or prefix (used with --bucket)",
        "对象 Key 或前缀（与 --bucket 搭配使用）",
    ),
    ("Folder key (used with --bucket)", "文件夹 Key（与 --bucket 搭配使用）"),
    (
        "Create parent folder markers as needed",
        "按需创建父级文件夹标记",
    ),
    (
        "Create parent folders as needed",
        "按需创建父级文件夹",
    ),
    (
        "Region override for this request",
        "仅本次请求覆盖 region",
    ),
    (
        "AZ redundancy mode. Allowed: single-az, multi-az",
        "AZ 冗余模式。允许值：single-az、multi-az",
    ),
    (
        "Bucket type. Allowed: fns, hns",
        "Bucket 类型。允许值：fns、hns",
    ),
    ("Enable bucket object lock", "启用 Bucket 对象锁"),
    ("Confirm bucket deletion", "确认删除 Bucket"),
    ("Confirm resource deletion", "确认删除资源"),
    ("Recursive folder delete strategy", "递归文件夹删除策略"),
    (
        "Delete children before parent directory objects",
        "先删除子项，再删除父目录对象",
    ),
    (
        "Ask the service to delete a directory object recursively",
        "请求服务端递归删除目录对象",
    ),
    (
        "Delete children before parent folders",
        "先删除子项，再删除父文件夹",
    ),
    (
        "Ask the service to delete the folder directly",
        "请求服务端直接删除文件夹",
    ),
    (
        "Delete every object version (and delete markers) instead of only the current version. Required when the bucket has versioning enabled and the caller wants permanent removal",
        "删除每个对象版本（以及删除标记），而不是仅删除当前版本。Bucket 启用版本控制且调用方需要永久删除时必填。",
    ),
    (
        "Also abort incomplete multipart uploads matching the prefix",
        "同时中止与前缀匹配的未完成分片上传",
    ),
    (
        "Also abort incomplete multipart uploads recorded in ADrive checkpoints matching the target",
        "同时中止 ADrive checkpoint 中与目标匹配的未完成分片上传",
    ),
    (
        "Checkpoint directory to scan when --include-uploads is set",
        "设置 --include-uploads 时要扫描的 checkpoint 目录",
    ),
    ("Region", "区域"),
    ("Custom service endpoint", "自定义服务 endpoint"),
    (
        "Custom Data Plane endpoint",
        "自定义数据面 endpoint",
    ),
    (
        "CLI flag only. Environment region variables are read by each tool's profile loader so they stay at the lowest precedence (CLI > Config > Env).",
        "仅支持 CLI 参数。环境变量中的 region 由各工具的 profile 加载器读取，因此保持最低优先级（CLI > Config > Env）。",
    ),
    (
        "CLI flag only. Environment endpoint variables are read by each tool's profile loader so they stay at the lowest precedence.",
        "仅支持 CLI 参数。环境变量中的 endpoint 由各工具的 profile 加载器读取，因此保持最低优先级。",
    ),
    (
        "Maximum buckets, objects, or prefixes to return from the current level",
        "当前层级最多返回的 Bucket、对象或前缀数量",
    ),
    (
        "Continuation token returned by a previous listing",
        "上一次列举返回的 continuation token",
    ),
    ("Custom control-plane endpoint", "自定义控制面 endpoint"),
    (
        "Custom Control Plane endpoint (used only by the TOS command surface)",
        "自定义控制面 endpoint（仅 TOS 命令面使用）",
    ),
    (
        "CLI flag only. Tool-specific control endpoint variables are read by the profile loader when that tool supports control-plane operations.",
        "仅支持 CLI 参数。当工具支持控制面操作时，工具专属 control endpoint 环境变量由 profile 加载器读取。",
    ),
    ("Account ID for control-plane endpoints", "控制面 endpoint 使用的账号 ID"),
    (
        "Account ID for Control Plane endpoints",
        "控制面 endpoint 使用的账号 ID",
    ),
    (
        "CLI flag only. Tool-specific account ID variables are read by the profile loader when that tool supports control-plane operations.",
        "仅支持 CLI 参数。当工具支持控制面操作时，工具专属账号 ID 环境变量由 profile 加载器读取。",
    ),
    ("Human-readable sizes", "以人类可读格式显示大小"),
    ("Maximum directory depth", "最大目录深度"),
    (
        "Number of largest/oldest object samples to keep",
        "保留的最大/最旧对象样本数量",
    ),
    (
        "Number of largest/oldest file samples to keep",
        "保留的最大/最旧文件样本数量",
    ),
    (
        "Include estimated monthly storage cost by storage class",
        "按存储类型包含预估月度存储成本",
    ),
    (
        "Override storage price, e.g. STANDARD=0.12 (CNY/GB/month)",
        "覆盖存储单价，例如 STANDARD=0.12（元/GB/月）",
    ),
    (
        "Write traversed-object manifest to this path. No manifest is written unless this is set.",
        "将已遍历对象 manifest 写入此路径；未设置时不会写 manifest。",
    ),
    (
        "Write traversed-object manifest to this path. No manifest is written unless this is set",
        "将已遍历对象 manifest 写入此路径；未设置时不会写 manifest",
    ),
    (
        "Write traversed-file manifest to this path. No manifest is written unless this is set.",
        "将已遍历文件 manifest 写入此路径；未设置时不会写 manifest。",
    ),
    (
        "Write traversed-file manifest to this path. No manifest is written unless this is set",
        "将已遍历文件 manifest 写入此路径；未设置时不会写 manifest",
    ),
    (
        "Write matched-object manifest to this path. No manifest is written unless this is set.",
        "将匹配对象 manifest 写入此路径；未设置时不会写 manifest。",
    ),
    (
        "Write matched-object manifest to this path. No manifest is written unless this is set",
        "将匹配对象 manifest 写入此路径；未设置时不会写 manifest",
    ),
    (
        "Write matched-file manifest to this path. No manifest is written unless this is set.",
        "将匹配文件 manifest 写入此路径；未设置时不会写 manifest。",
    ),
    (
        "Write matched-file manifest to this path. No manifest is written unless this is set",
        "将匹配文件 manifest 写入此路径；未设置时不会写 manifest",
    ),
    ("Name pattern", "名称匹配模式"),
    ("Size filter (e.g., +1GB, -100KB)", "大小过滤器（例如 +1GB、-100KB）"),
    ("Modification time filter", "修改时间过滤器"),
    ("Storage class filter", "存储类型过滤器"),
    ("Byte range (e.g., 0-1023)", "字节范围（例如 0-1023）"),
    ("Object version ID", "对象版本 ID"),
    ("Version ID", "版本 ID"),
    (
        "URL expiration time (e.g., 3600)",
        "URL 过期时间（例如 3600）",
    ),
    ("HTTP method (GET, PUT)", "HTTP 方法（GET、PUT）"),
    (
        "Restore all archived objects under a prefix",
        "恢复前缀下的所有归档对象",
    ),
    (
        "Restore objects listed in a manifest file",
        "恢复 manifest 文件中列出的对象",
    ),
    ("Restore days", "恢复天数"),
    (
        "Restore tier (Expedited, Standard, Bulk)",
        "恢复档位（Expedited、Standard、Bulk）",
    ),
    (
        "Confirm batch restore and cost-related side effects",
        "确认批量恢复及相关费用影响",
    ),
    (
        "Stdin input is uploaded after EOF: press Ctrl+D on Unix/macOS, or Ctrl+Z then Enter on Windows. Ctrl+C cancels the command.",
        "标准输入会在 EOF 后上传：Unix/macOS 按 Ctrl+D，Windows 按 Ctrl+Z 后回车。Ctrl+C 会取消命令。",
    ),
    (
        "Output format: json / xml / table / csv / yaml / markdown",
        "输出格式：json / xml / table / csv / yaml / markdown",
    ),
    (
        "Output format (json, table, csv, yaml, markdown)",
        "输出格式（json、table、csv、yaml、markdown）",
    ),
    (
        "Markdown — emit Envelope as a human-readable Markdown report so Agents (and humans) can paste CLI responses directly into chat / docs",
        "Markdown：将 Envelope 输出为人类可读的 Markdown 报告，便于 Agent 和用户直接粘贴到聊天或文档中",
    ),
    ("Sort field", "排序字段"),
    (
        "Select columns for table/csv output (comma-separated, e.g. key,size,last_modified)",
        "选择 table/csv 输出列（逗号分隔，例如 key,size,last_modified）",
    ),
    ("JMESPath filter expression", "JMESPath 过滤表达式"),
    (
        "Preview the effect of the command without executing it",
        "只预览命令影响，不执行实际操作",
    ),
    (
        "Write listing manifest to this path. No manifest is written unless this is set",
        "将列举 manifest 写入此路径；未设置时不会写 manifest",
    ),
    (
        "Print structured self-description of the command",
        "输出命令的结构化自描述",
    ),
    (
        "Auto-confirm destructive prompts in an interactive shell. In pipe (non-TTY) contexts delete-class critical operations still require `--force` plus an exact `--confirm <RESOURCE>` match",
        "在交互式 shell 中自动确认破坏性提示。在管道（非 TTY）场景下，删除类高危操作仍需要 `--force` 和精确匹配的 `--confirm <RESOURCE>`。",
    ),
    (
        "Auto-confirm destructive prompts in an interactive shell",
        "在交互式 shell 中自动确认破坏性提示",
    ),
    (
        "Confirm critical delete operations by typing the exact public resource URI (for example `tos://bucket/prefix` or `adrive://inst/space/path`). Required with `--force` in pipe (non-TTY) contexts",
        "通过输入精确的公开资源 URI 确认高危删除操作（例如 `tos://bucket/prefix` 或 `adrive://inst/space/path`）。管道（非 TTY）场景下与 `--force` 搭配时必填。",
    ),
    (
        "Confirm critical deletes with the exact tos:// or adrive:// target",
        "使用精确的 tos:// 或 adrive:// 目标确认高危删除",
    ),
    ("Disable colored output.", "禁用彩色输出。"),
    (
        "`NO_COLOR` is commonly set to `1` in many environments, so this flag accepts the usual boolean spellings: `1/0/true/false/on/off`.",
        "`NO_COLOR` 在许多环境中通常设为 `1`，因此该参数接受常见布尔写法：`1/0/true/false/on/off`。",
    ),
    (
        "Disable colored output (also honors `NO_COLOR=1`)",
        "禁用彩色输出（也遵循 `NO_COLOR=1`）",
    ),
    (
        "Include extra diagnostic output where supported",
        "在支持的场景输出额外诊断信息",
    ),
    (
        "Disable prompts and progress output",
        "禁用提示和进度输出",
    ),
    (
        "Enable listing-phase echo output even when stderr is not a TTY",
        "即使 stderr 不是 TTY，也启用 list 阶段回显",
    ),
    ("Disable listing-phase echo output", "禁用 list 阶段回显"),
    (
        "Enable execution progress output even when stderr is not a TTY",
        "即使 stderr 不是 TTY，也启用执行阶段进度输出",
    ),
    ("Disable execution progress output", "禁用执行阶段进度输出"),
    (
        "Export skills as Markdown SKILL.md directories",
        "将 Skill 导出为 Markdown SKILL.md 目录",
    ),
    (
        "List or export TOS skill metadata",
        "列出或导出 TOS Skill 元数据",
    ),
    (
        "List or export ADrive skill metadata",
        "列出或导出 ADrive Skill 元数据",
    ),
    (
        "List built-in TOS skills or export them as Markdown SKILL.md directories for Codex/Agent runtimes.",
        "列出内置 TOS Skill，或将其导出为 Codex/Agent 运行时可用的 Markdown SKILL.md 目录。",
    ),
    (
        "List built-in ADrive skills or export them as Markdown SKILL.md directories for Codex/Agent runtimes.",
        "列出内置 ADrive Skill，或将其导出为 Codex/Agent 运行时可用的 Markdown SKILL.md 目录。",
    ),
    (
        "Exported files are portable skill-pack artifacts. They are not required to run `serve`; `serve` rebuilds tools from the in-process registry.",
        "导出的文件是可移植的 skill-pack 产物。运行 `serve` 不依赖这些文件；`serve` 会从进程内 registry 重新构建工具。",
    ),
    (
        "Exported files are portable skill-pack artifacts. They are not required to run `serve`; `serve` rebuilds MCP tools from the in-process registry.",
        "导出的文件是可移植的 skill-pack 产物。运行 `serve` 不依赖这些文件；`serve` 会从进程内 registry 重新构建 MCP 工具。",
    ),
    (
        "Output directory for exported Markdown skill files",
        "导出的 Markdown Skill 文件输出目录",
    ),
    ("Output directory", "输出目录"),
    ("Documentation language: en or zh", "文档语言：en 或 zh"),
    (
        "Documentation language for generated skill metadata: en (default) or zh",
        "生成的 Skill 元数据文档语言：en（默认）或 zh",
    ),
    (
        "Documentation language for generated SKILL.md files: en (default) or zh",
        "生成的 SKILL.md 文件文档语言：en（默认）或 zh",
    ),
    (
        "Export writes dir/SKILL.md plus dir/{domain}/{skill_name}/SKILL.md and refuses to overwrite existing files.",
        "导出会写入 dir/SKILL.md 以及 dir/{domain}/{skill_name}/SKILL.md，并拒绝覆盖已存在文件。",
    ),
    (
        "Use --language zh to generate Chinese Markdown skill docs.",
        "使用 --language zh 生成中文 Markdown Skill 文档。",
    ),
    (
        "Use --dry-run to preview target paths and conflicts without creating files.",
        "使用 --dry-run 预览目标路径和冲突，不创建文件。",
    ),
    (
        "MCP tool names match exported skill names, e.g. `ve_adrive_ls` for `ve-adrive ls`.",
        "MCP 工具名与导出的 Skill 名称一致，例如 `ve-adrive ls` 对应 `ve_adrive_ls`。",
    ),
    (
        "Copy local files, ADrive files, or folders",
        "复制本地文件、ADrive 文件或文件夹",
    ),
    (
        "Move files or folders by copy plus source delete",
        "通过复制并删除源文件/文件夹来移动",
    ),
    ("Create an instance or space", "创建实例或空间"),
    ("Delete an instance or space", "删除实例或空间"),
    ("Delete a file or folder", "删除文件或文件夹"),
    (
        "List instances, spaces, files, or folders",
        "列出实例、空间、文件或文件夹",
    ),
    (
        "Show instance, space, file, or folder metadata",
        "查看实例、空间、文件或文件夹元数据",
    ),
    (
        "Calculate file size statistics for a folder",
        "统计文件夹大小",
    ),
    ("Find files by name, size, or mtime", "按名称、大小或修改时间查找文件"),
    ("Stream file content", "流式输出文件内容"),
    ("Upload stdin to a file", "将标准输入上传为文件"),
    (
        "Treat ADrive instance/space target segments as names and resolve them to IDs",
        "将 ADrive 目标中的 instance/space 片段按名称解析为 ID",
    ),
    (
        "Treat the parent instance target as a name when creating a space",
        "创建空间时将父实例目标按名称解析",
    ),
    (
        "Instance name to create, or existing instance ID when --space is set",
        "要创建的实例名称，或设置 --space 时使用的已有实例 ID",
    ),
    (
        "Space name to create under --instance",
        "在 --instance 下要创建的空间名称",
    ),
    (
        "Display name for the created instance or space",
        "已创建实例或空间的显示名称",
    ),
    (
        "Description for the created instance or space",
        "已创建实例或空间的描述",
    ),
    (
        "Enable search indexing for a newly-created space",
        "为新建空间启用搜索索引",
    ),
    (
        "Instance ID to delete, or containing instance ID when --space is set",
        "要删除的实例 ID，或设置 --space 时的所属实例 ID",
    ),
    (
        "Space ID to delete under --instance",
        "在 --instance 下要删除的空间 ID",
    ),
    ("Folder path inside the space", "空间内的文件夹路径"),
    ("File name inside the folder", "文件夹内的文件名"),
    ("生成预签名 URLs", "生成预签名 URL"),
    ("恢复归档对象s", "恢复归档对象"),
];

fn args_without_help_language(effective_args: &[String]) -> Vec<String> {
    let mut sanitized = Vec::with_capacity(effective_args.len());
    let mut index = 0;
    while index < effective_args.len() {
        let arg = effective_args[index].as_str();
        if arg == "--language" || arg == "--help-language" {
            index += 2;
            continue;
        }
        if arg.starts_with("--language=") || arg.starts_with("--help-language=") {
            index += 1;
            continue;
        }
        sanitized.push(effective_args[index].clone());
        index += 1;
    }
    sanitized
}

fn print_chinese_help(effective_args: &[String]) {
    let sanitized_args = args_without_help_language(effective_args);
    if maybe_print_byted_tos_grouped_help(&sanitized_args) {
        print!("{}", byted_tos_grouped_help_zh());
        return;
    }
    if maybe_print_tos_grouped_help(&sanitized_args) {
        print!("{}", tos_grouped_help_zh());
        return;
    }
    if maybe_print_adrive_grouped_help(&sanitized_args) {
        print!("{}", adrive_grouped_help_zh());
        return;
    }
    if !is_byted_tos_invocation(&sanitized_args)
        && !is_tos_invocation(&sanitized_args)
        && !is_adrive_invocation(&sanitized_args)
    {
        if is_unified_root_help_request(&sanitized_args) {
            print!("{}", unified_grouped_help_zh());
        } else {
            // [Review Fix #HelpZh4] A localized top-level help request must not
            // hide an invalid command token behind the generic root help.
            exit_unknown_chinese_help(&recovered_any_command_path(&sanitized_args));
        }
        return;
    }
    match display_help_text_for_args(&sanitized_args) {
        Ok(help) => print!("{}", localize_clap_help_zh(&help)),
        Err(err)
            if matches!(
                err.kind(),
                ClapErrorKind::UnknownArgument | ClapErrorKind::InvalidSubcommand
            ) =>
        {
            exit_unknown_chinese_help(&recovered_any_command_path(&sanitized_args));
        }
        Err(err) => err.exit(),
    }
}

fn is_unified_root_help_request(effective_args: &[String]) -> bool {
    let after_binary = &effective_args[1..];
    after_binary
        .iter()
        .all(|arg| matches!(arg.as_str(), "--help" | "-h"))
}

fn exit_help_language_error(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(tos_core::agent::error::ExitCode::ValidationError.as_i32());
}

fn exit_unknown_chinese_help(command: &str) -> ! {
    // [Review Fix #HelpZh3] A Chinese help request for an unknown subcommand is
    // still a validation error; returning root help with exit 0 would hide typos
    // in scripts and agent-generated commands.
    eprintln!("未知命令：{command}");
    eprintln!("请运行根命令的 `--help --language zh` 查看可用命令。");
    std::process::exit(tos_core::agent::error::ExitCode::ValidationError.as_i32());
}

fn recovered_any_command_path(effective_args: &[String]) -> String {
    if is_byted_tos_invocation(effective_args) {
        recovered_command_path(effective_args, "byted-tos")
    } else if is_tos_invocation(effective_args) {
        recovered_command_path(effective_args, "ve-tos")
    } else if is_adrive_invocation(effective_args) {
        recovered_command_path(effective_args, "ve-adrive")
    } else {
        let mut command = vec!["ve-storage-uni-cli".to_string()];
        let mut iter = effective_args.iter().skip(1).peekable();
        while let Some(arg) = iter.next() {
            if arg.starts_with('-') {
                if flag_takes_value(arg) {
                    let _ = iter.next();
                }
                continue;
            }
            command.push(arg.clone());
        }
        command.join(" ")
    }
}

fn unified_grouped_help_zh() -> String {
    let mut output = String::new();
    let _ = writeln!(output, "Volcengine Storage Unified CLI - Agent-Native\n");
    let _ = writeln!(output, "用法:");
    let _ = writeln!(output, "  ve-storage-uni-cli tos <命令> [选项]");
    let _ = writeln!(output, "  ve-storage-uni-cli ve-tos <命令> [选项]");
    let _ = writeln!(output, "  ve-storage-uni-cli ve-adrive <命令> [选项]\n");
    let _ = writeln!(output, "工具:");
    let _ = writeln!(
        output,
        "  tos                    ByteCloud TOS 高阶对象存储命令"
    );
    let _ = writeln!(
        output,
        "  ve-tos                 火山引擎 TOS 高阶、低阶与工具命令"
    );
    let _ = writeln!(
        output,
        "  ve-adrive              ADrive 文件管理与工具命令\n"
    );
    append_root_common_options_zh(&mut output);
    append_help_language_section_zh(&mut output);
    translate_help_phrases_zh(&output)
}

fn tos_grouped_help_zh() -> String {
    let prefix = std::env::var(TOS_EXAMPLE_PREFIX_ENV).unwrap_or_else(|_| "ve-tos-cli".to_string());
    let mut output = String::new();
    let _ = writeln!(output, "TOS Object Storage CLI - Agent-Native\n");
    append_tos_usage_zh(&mut output, &prefix);
    for (title, category) in [
        ("高阶命令", "high_level"),
        ("低阶 API - 核心", "core"),
        ("低阶 API - Bucket 配置", "bucket_config"),
        ("低阶 API - 高级能力", "advanced"),
        ("能力 / 工具", "utilities"),
    ] {
        append_tos_category_zh(&mut output, title, category);
    }
    append_tos_target_syntax_zh(&mut output);
    append_root_common_options_zh(&mut output);
    append_help_language_section_zh(&mut output);
    append_tos_examples_zh(&mut output, &prefix);
    translate_help_phrases_zh(&output)
}

fn byted_tos_grouped_help_zh() -> String {
    let prefix =
        std::env::var(BYTED_TOS_EXAMPLE_PREFIX_ENV).unwrap_or_else(|_| "tos-cli".to_string());
    let mut output = String::new();
    let _ = writeln!(output, "TOS CLI - Agent-Native\n");
    let _ = writeln!(output, "用法:\n  {prefix} <命令> [选项]\n");
    append_byted_capability_group_zh(&mut output, "高阶命令", "high_level");
    append_byted_capability_group_zh(&mut output, "能力 / 工具", "utilities");
    append_tos_target_syntax_zh(&mut output);
    append_root_common_options_zh(&mut output);
    append_byted_tos_psm_options_zh(&mut output);
    append_help_language_section_zh(&mut output);
    append_byted_examples_zh(&mut output, &prefix);
    translate_help_phrases_zh(&output)
}

fn adrive_grouped_help_zh() -> String {
    let prefix =
        std::env::var(ADRIVE_EXAMPLE_PREFIX_ENV).unwrap_or_else(|_| "ve-adrive-cli".to_string());
    let mut output = String::new();
    let _ = writeln!(output, "ADrive CLI - Agent-Native\n");
    let _ = writeln!(output, "用法:\n  {prefix} <命令> [选项]\n");
    append_adrive_capability_group_zh(&mut output, "高阶命令", "high_level");
    append_adrive_capability_group_zh(&mut output, "能力 / 工具", "utilities");
    append_adrive_target_syntax_zh(&mut output);
    append_root_common_options_zh(&mut output);
    append_help_language_section_zh(&mut output);
    append_adrive_examples_zh(&mut output, &prefix);
    translate_help_phrases_zh(&output)
}

fn append_tos_usage_zh(output: &mut String, prefix: &str) {
    let _ = writeln!(output, "用法:");
    let _ = writeln!(output, "  {prefix} <命令> [选项]");
    if prefix == "ve-tos-cli" {
        let _ = writeln!(output, "  ve-storage-uni-cli ve-tos <命令> [选项]\n");
    } else {
        let _ = writeln!(output, "  ve-tos-cli <命令> [选项]\n");
    }
}

fn append_tos_category_zh(output: &mut String, title: &str, category: &str) {
    let _ = writeln!(output, "{title}:");
    for entry in ve_tos_cli::registry::command_groups()
        .iter()
        .filter(|entry| entry.category == category)
    {
        let _ = writeln!(output, "  {:<26} {}", entry.name, entry.description);
    }
    output.push('\n');
}

fn append_byted_capability_group_zh(output: &mut String, title: &str, group: &str) {
    let _ = writeln!(output, "{title}:");
    for row in tos_cli::registry::capabilities()
        .iter()
        .filter(|row| row.group == group)
    {
        let command = row.command.strip_prefix("tos ").unwrap_or(row.command);
        let _ = writeln!(output, "  {:<22} {}", command, row.description);
    }
    output.push('\n');
}

fn append_adrive_capability_group_zh(output: &mut String, title: &str, group: &str) {
    let _ = writeln!(output, "{title}:");
    for row in ve_adrive_cli::registry::capabilities()
        .iter()
        .filter(|row| adrive_group_matches(row.group, group))
    {
        let command = row
            .command
            .strip_prefix("ve-adrive ")
            .unwrap_or(row.command);
        let _ = writeln!(output, "  {:<22} {}", command, row.description);
    }
    output.push('\n');
}

fn adrive_group_matches(row_group: &str, target_group: &str) -> bool {
    matches!(
        (row_group, target_group),
        ("High-Level", "high_level") | ("Capabilities / Utilities", "utilities")
    ) || row_group == target_group
}

fn append_tos_target_syntax_zh(output: &mut String) {
    let _ = writeln!(output, "TOS 目标语法:");
    let _ = writeln!(output, "  URI:     tos://<bucket>/<key>");
    let _ = writeln!(
        output,
        "  Flags:   --bucket <BUCKET> --key <KEY> / --prefix <PREFIX>\n"
    );
}

fn append_byted_tos_psm_options_zh(output: &mut String) {
    let _ = writeln!(output, "ByteCloud TOS PSM 选项:");
    let _ = writeln!(
        output,
        "      --psm <PSM>             PSM 服务名（通过 BNS 服务发现访问）"
    );
    let _ = writeln!(
        output,
        "      --idc <IDC>             与 --psm 配合使用的 IDC"
    );
    let _ = writeln!(
        output,
        "      --cluster <CLUSTER>     与 --psm 配合使用的集群"
    );
    let _ = writeln!(
        output,
        "      --addr-family <VALUE>   与 --psm 配合使用的地址族：v4、v6 或 dual-stack\n"
    );
}

fn append_adrive_target_syntax_zh(output: &mut String) {
    let _ = writeln!(output, "ADrive 目标语法:");
    let _ = writeln!(
        output,
        "  URI:     adrive://<instance>/<space>/<folder>/<file>"
    );
    let _ = writeln!(
        output,
        "  Flags:   --instance <ID> --space <ID> --folder <PATH> --file <NAME>"
    );
    let _ = writeln!(
        output,
        "  Names:   使用 --by-name 时按名称解析 instance/space\n"
    );
}

fn append_root_common_options_zh(output: &mut String) {
    let _ = writeln!(output, "常用选项:");
    let _ = writeln!(output, "  -P, --profile <PROFILE>     配置 profile 名称");
    let _ = writeln!(output, "  -r, --region <REGION>       区域");
    let _ = writeln!(output, "  -e, --endpoint <ENDPOINT>   自定义 endpoint");
    let _ = writeln!(
        output,
        "  -o, --output <FORMAT>       输出格式：json/table/csv/yaml/markdown"
    );
    let _ = writeln!(output, "      --query <QUERY>         JMESPath 过滤表达式");
    let _ = writeln!(
        output,
        "      --dry-run               只预览计划，不执行变更"
    );
    let _ = writeln!(output, "      --describe              输出结构化命令描述");
    let _ = writeln!(output, "  -h, --help                  显示帮助");
    let _ = writeln!(output, "  -V, --version               显示版本\n");
}

fn append_help_language_section_zh(output: &mut String) {
    let _ = writeln!(output, "语言:");
    let _ = writeln!(
        output,
        "  --language <en|zh>      帮助输出语言，例如 --help --language zh\n"
    );
}

fn append_tos_examples_zh(output: &mut String, prefix: &str) {
    let _ = writeln!(output, "示例:");
    let _ = writeln!(output, "  {prefix} mb tos://mybucket");
    let _ = writeln!(output, "  {prefix} ls tos://mybucket/");
    let _ = writeln!(output, "  {prefix} cp ./a.txt tos://mybucket/docs/a.txt");
    let _ = writeln!(
        output,
        "  {prefix} rm tos://mybucket/docs/a.txt --force --confirm tos://mybucket/docs/a.txt\n"
    );
    let _ = writeln!(output, "查看命令详情：{prefix} <命令> --help --language zh");
}

fn append_byted_examples_zh(output: &mut String, prefix: &str) {
    let _ = writeln!(output, "示例:");
    let _ = writeln!(output, "  {prefix} ls tos://mybucket/");
    let _ = writeln!(output, "  {prefix} cp ./a.txt tos://mybucket/docs/a.txt");
    let _ = writeln!(output, "  {prefix} capabilities --view full\n");
    let _ = writeln!(output, "查看命令详情：{prefix} <命令> --help --language zh");
}

fn append_adrive_examples_zh(output: &mut String, prefix: &str) {
    let _ = writeln!(output, "示例:");
    let _ = writeln!(output, "  {prefix} crt adrive://inst-1/space-1");
    let _ = writeln!(output, "  {prefix} ls adrive://inst-1/space-1/docs/");
    let _ = writeln!(
        output,
        "  {prefix} cp ./a.txt adrive://inst-1/space-1/docs/a.txt\n"
    );
    let _ = writeln!(output, "查看命令详情：{prefix} <命令> --help --language zh");
}

/// Runs the unified dispatcher binary entry point.
pub async fn run_multi_tool() {
    init_tracing();
    let args: Vec<String> = std::env::args().collect();
    run_with_args(args, InvocationSurface::Unified).await;
}

/// Runs the dedicated ByteCloud TOS CLI entry point used by the `tos-cli` entry crate.
pub async fn run_byted_tos_cli() {
    init_tracing();
    let args: Vec<String> = std::env::args().collect();
    if maybe_print_direct_version(&args, "tos-cli") {
        return;
    }
    let effective_args = direct_tool_args(args, "byted-tos");
    run_with_args(effective_args, InvocationSurface::BytedTosDirect).await;
}

/// Runs the dedicated TOS CLI entry point used by the `ve-tos-cli` entry crate.
pub async fn run_tos_cli() {
    init_tracing();
    let args: Vec<String> = std::env::args().collect();
    if maybe_print_direct_version(&args, "ve-tos-cli") {
        return;
    }
    let effective_args = direct_tool_args(args, "ve-tos");
    run_with_args(effective_args, InvocationSurface::VeTosDirect).await;
}

/// Runs the dedicated ADrive CLI entry point used by the `ve-adrive-cli` entry crate.
pub async fn run_adrive_cli() {
    init_tracing();
    let args: Vec<String> = std::env::args().collect();
    if maybe_print_direct_version(&args, "ve-adrive-cli") {
        return;
    }
    let effective_args = direct_tool_args(args, "ve-adrive");
    run_with_args(effective_args, InvocationSurface::ADriveDirect).await;
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .try_init();
}

struct EnvGuard {
    key: &'static str,
    previous: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &'static str) -> Self {
        let previous = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self { key, previous }
    }

    fn set_from_env_or_remove(key: &'static str, source_key: &'static str) -> Self {
        let previous = std::env::var(key).ok();
        if let Ok(value) = std::env::var(source_key) {
            std::env::set_var(key, value);
        } else {
            std::env::remove_var(key);
        }
        Self { key, previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        if let Some(value) = self.previous.take() {
            std::env::set_var(self.key, value);
        } else {
            std::env::remove_var(self.key);
        }
    }
}

fn direct_tool_args(args: Vec<String>, tool: &str) -> Vec<String> {
    let public_tool = public_tool_name(tool);
    let mut new_args = vec!["ve-storage-uni-cli".to_string(), public_tool.to_string()];
    new_args.extend(args.into_iter().skip(1));
    new_args
}

fn maybe_print_direct_version(args: &[String], binary_name: &str) -> bool {
    if args.len() == 2 && matches!(args[1].as_str(), "--version" | "-V") {
        println!("{} {}", binary_name, env!("CARGO_PKG_VERSION"));
        return true;
    }
    false
}

fn public_tool_name(tool: &str) -> &str {
    match tool {
        "byted-tos" => "tos",
        other => other,
    }
}

fn canonical_tool_command(tool: &str) -> &str {
    match tool {
        "byted-tos" => "tos",
        other => other,
    }
}

async fn run_with_args(args: Vec<String>, invocation_surface: InvocationSurface) {
    let binary_name = std::path::Path::new(&args[0])
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("ve-storage-uni-cli")
        .to_string();

    let effective_surface = match binary_name.as_str() {
        "tos" | "tos-cli" => InvocationSurface::BytedTosDirect,
        "ve-tos" | "ve-tos-cli" => InvocationSurface::VeTosDirect,
        "ve-adrive" | "ve-adrive-cli" => InvocationSurface::ADriveDirect,
        _ => invocation_surface,
    };
    configure_example_prefixes(effective_surface);

    let effective_args: Vec<String> = match binary_name.as_str() {
        "tos" | "tos-cli" | "ve-tos" | "ve-tos-cli" | "ve-adrive" | "ve-adrive-cli" => {
            let tool_name = match binary_name.as_str() {
                "tos" | "tos-cli" => "tos".to_string(),
                "ve-tos" | "ve-tos-cli" => "ve-tos".to_string(),
                "ve-adrive" | "ve-adrive-cli" => "ve-adrive".to_string(),
                other => other.to_string(),
            };
            let mut new_args = vec!["ve-storage-uni-cli".to_string(), tool_name];
            new_args.extend(args[1..].iter().cloned());
            new_args
        }
        _ => args,
    };
    let effective_args = normalize_help_aliases(&effective_args);
    let effective_args = normalize_leading_help_flag(&effective_args);
    let _byted_profile_env_guard = is_byted_tos_invocation(&effective_args)
        .then(|| EnvGuard::set_from_env_or_remove("TOS_PROFILE", "BYTE_TOS_PROFILE"));
    let _byted_output_env_guard = is_byted_tos_invocation(&effective_args)
        .then(|| EnvGuard::set_from_env_or_remove("TOS_OUTPUT", "BYTE_TOS_OUTPUT"));

    if maybe_print_language_help(&effective_args) {
        return;
    }
    if maybe_print_byted_tos_grouped_help(&effective_args) {
        tos_cli::print_grouped_help();
        return;
    }
    if maybe_print_tos_grouped_help(&effective_args) {
        ve_tos_cli::print_grouped_help();
        return;
    }
    if maybe_print_adrive_grouped_help(&effective_args) {
        ve_adrive_cli::print_grouped_help();
        return;
    }

    // [Review Fix #19] Use try_parse_from so `tos` parse failures can be rendered as failed Envelope.
    let cli = match Cli::try_parse_from(&effective_args) {
        Ok(cli) => {
            // [Review Fix #1] Preserve parameter-independent trailing `--describe`
            // only when clap parsed the command but did not attach the flag to
            // global args; richer handler-level describe output keeps precedence.
            if has_flag(&effective_args, "--describe") && !cli.global.describe {
                if maybe_emit_byted_tos_describe_recovery(&effective_args) {
                    return;
                }
                if maybe_emit_tos_describe_recovery(&effective_args) {
                    return;
                }
                if maybe_emit_adrive_describe_recovery(&effective_args) {
                    return;
                }
            }
            cli
        }
        Err(err) => handle_parse_error(&effective_args, err),
    };

    match cli.tool {
        ToolCommand::TosCli { command } => {
            handle_byted_tos(cli.global, command).await;
        }
        ToolCommand::Tos { command } => {
            handle_tos(cli.global, command).await;
        }
        ToolCommand::ADrive { command } => {
            handle_adrive(cli.global, command).await;
        }
    }
}

fn configure_example_prefixes(invocation_surface: InvocationSurface) {
    match invocation_surface {
        InvocationSurface::Unified => {
            std::env::set_var(USER_AGENT_NAME_ENV, "ve-storage-uni-cli");
            std::env::set_var(BYTED_TOS_EXAMPLE_PREFIX_ENV, "ve-storage-uni-cli tos");
            std::env::set_var(TOS_EXAMPLE_PREFIX_ENV, "ve-storage-uni-cli ve-tos");
            std::env::set_var(ADRIVE_EXAMPLE_PREFIX_ENV, "ve-storage-uni-cli ve-adrive");
        }
        InvocationSurface::VeTosDirect => {
            std::env::set_var(USER_AGENT_NAME_ENV, "ve-tos-cli");
            std::env::set_var(BYTED_TOS_EXAMPLE_PREFIX_ENV, "ve-storage-uni-cli tos");
            std::env::set_var(TOS_EXAMPLE_PREFIX_ENV, "ve-tos-cli");
            std::env::set_var(ADRIVE_EXAMPLE_PREFIX_ENV, "ve-storage-uni-cli ve-adrive");
        }
        InvocationSurface::BytedTosDirect => {
            std::env::set_var(USER_AGENT_NAME_ENV, "tos-cli");
            std::env::set_var(BYTED_TOS_EXAMPLE_PREFIX_ENV, "tos-cli");
            std::env::set_var(TOS_EXAMPLE_PREFIX_ENV, "ve-storage-uni-cli ve-tos");
            std::env::set_var(ADRIVE_EXAMPLE_PREFIX_ENV, "ve-storage-uni-cli ve-adrive");
        }
        InvocationSurface::ADriveDirect => {
            std::env::set_var(USER_AGENT_NAME_ENV, "ve-adrive-cli");
            std::env::set_var(BYTED_TOS_EXAMPLE_PREFIX_ENV, "ve-storage-uni-cli tos");
            std::env::set_var(TOS_EXAMPLE_PREFIX_ENV, "ve-storage-uni-cli ve-tos");
            std::env::set_var(ADRIVE_EXAMPLE_PREFIX_ENV, "ve-adrive-cli");
        }
    }
}

fn handle_parse_error(effective_args: &[String], err: clap::Error) -> Cli {
    if matches!(err.kind(), ClapErrorKind::DisplayHelp) {
        print_display_help_with_registry_examples(effective_args, &err);
        std::process::exit(0);
    }
    if matches!(err.kind(), ClapErrorKind::DisplayVersion) {
        let _ = err.print();
        std::process::exit(0);
    }
    if maybe_emit_byted_tos_describe_recovery(effective_args) {
        std::process::exit(0);
    }
    if maybe_emit_tos_describe_recovery(effective_args) {
        std::process::exit(0);
    }
    if maybe_emit_adrive_describe_recovery(effective_args) {
        std::process::exit(0);
    }
    if is_byted_tos_invocation(effective_args) {
        // [Review Fix #1] New `tos` parse errors follow the Agent envelope contract.
        emit_parse_error(effective_args, "byted-tos", &err);
        std::process::exit(tos_core::agent::error::ExitCode::ValidationError.as_i32());
    }
    if is_tos_invocation(effective_args) {
        // [Review Fix #19] Parse-time validation errors are part of the Agent error contract.
        emit_parse_error(effective_args, "ve-tos", &err);
        std::process::exit(tos_core::agent::error::ExitCode::ValidationError.as_i32());
    }
    if is_adrive_invocation(effective_args) {
        emit_parse_error(effective_args, "ve-adrive", &err);
        std::process::exit(tos_core::agent::error::ExitCode::ValidationError.as_i32());
    }
    err.exit();
}

// [Review Fix #TOS-HelpExamples] Keep generated/derived low-level help aligned
// with ADrive by appending registry examples when clap structs lack after_help.
fn print_display_help_with_registry_examples(effective_args: &[String], err: &clap::Error) {
    let help = display_help_text_with_registry_examples(effective_args, err);
    if err.use_stderr() {
        eprint!("{help}");
    } else {
        print!("{help}");
    }
}

fn display_help_text_with_registry_examples(
    effective_args: &[String],
    err: &clap::Error,
) -> String {
    let mut help = contextualized_help_text(effective_args, &err.to_string());
    if !help.contains("Examples:") {
        if let Some(examples) = tos_registry_help_examples(effective_args) {
            if !examples.is_empty() {
                let block = format!(
                    "\nExamples:\n{}\n",
                    examples
                        .iter()
                        .map(|example| format!("  {example}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                );
                help.push_str(&block);
            }
        }
    }
    append_help_language_hint(help)
}

fn append_help_language_hint(mut help: String) -> String {
    if help.contains("\nLanguage:") || help.contains("\n语言:") {
        return help;
    }
    // `--language` is handled before clap parsing, so clap cannot list it on
    // leaf command help by itself.
    if help_uses_language_for_command_behavior(&help) {
        // [Review Fix #1] Skill commands already use `--language` for generated
        // documentation, so document the accepted help-only alias there.
        help.push_str(HELP_LANGUAGE_ALIAS_HINT_EN);
    } else {
        help.push_str(HELP_LANGUAGE_HINT_EN);
    }
    help
}

fn help_uses_language_for_command_behavior(help: &str) -> bool {
    help.contains("--language <LANGUAGE>")
        || help.contains("--language zh")
        || help.contains("Documentation language")
}

fn contextualized_help_text(effective_args: &[String], help: &str) -> String {
    if is_byted_tos_invocation(effective_args) {
        let prefix =
            std::env::var(BYTED_TOS_EXAMPLE_PREFIX_ENV).unwrap_or_else(|_| "tos-cli".to_string());
        let help = help
            .replace("ve-storage-uni-cli ve-tos", &prefix)
            .replace("ve-tos-cli", &prefix)
            .replace("ve-tos-specific", "TOS-specific")
            .replace("[profile.ve-tos]", "[profile.tos]")
            .replace("[default.ve-tos]", "[default.tos]")
            .replace("[active-profile.ve-tos]", "[active-profile.tos]")
            .replace("[<profile>.ve-tos]", "[<profile>.tos]")
            .replace("<profile>.ve-tos.", "<profile>.tos.")
            .replace("staging.ve-tos.", "staging.tos.")
            // [Review Fix #11] ByteCloud `tos` parses profile/output through
            // BYTE_TOS_* guards, so its help must not advertise legacy TOS_*.
            .replace("[env: TOS_PROFILE=]", "[env: BYTE_TOS_PROFILE=]")
            .replace("[env: TOS_OUTPUT=]", "[env: BYTE_TOS_OUTPUT=]")
            .replace("For `ve-tos`", "For `tos`");
        let help = strip_control_plane_global_help(&help);
        return strip_byted_tos_unsupported_help(&help);
    }
    if is_tos_invocation(effective_args) {
        let prefix =
            std::env::var(TOS_EXAMPLE_PREFIX_ENV).unwrap_or_else(|_| "ve-tos-cli".to_string());
        let help = help.replace("ve-tos-cli", &prefix);
        return strip_psm_global_help(&help);
    }
    if is_adrive_invocation(effective_args) {
        let prefix = std::env::var(ADRIVE_EXAMPLE_PREFIX_ENV)
            .unwrap_or_else(|_| "ve-adrive-cli".to_string());
        let help = help.replace("ve-adrive-cli", &prefix);
        let help = strip_control_plane_global_help(&help);
        return strip_psm_global_help(&help);
    }
    help.to_string()
}

fn strip_control_plane_global_help(help: &str) -> String {
    strip_clap_help_option_blocks(help, &["--control-endpoint", "--account-id"])
}

fn strip_psm_global_help(help: &str) -> String {
    strip_clap_help_option_blocks(help, &["--psm", "--idc", "--cluster", "--addr-family"])
}

fn strip_byted_tos_unsupported_help(help: &str) -> String {
    // [Review Fix #TOS-StorageHelp] Strip the whole clap option block so the
    // removed `--storage-class` flag cannot leave its description under the
    // neighboring option.
    strip_clap_help_option_blocks(help, &["--storage-class"])
}

fn strip_clap_help_option_blocks(help: &str, option_names: &[&str]) -> String {
    let mut stripped_lines = Vec::new();
    let mut is_skipping_block = false;
    for line in help.lines() {
        if is_skipping_block {
            if is_clap_help_block_boundary(line) {
                is_skipping_block = false;
            } else {
                continue;
            }
        }

        if option_names.iter().any(|option| line.contains(option)) {
            is_skipping_block = true;
            continue;
        }
        stripped_lines.push(line);
    }

    let mut stripped = stripped_lines.join("\n");
    if help.ends_with('\n') {
        stripped.push('\n');
    }
    stripped
}

fn is_clap_help_block_boundary(line: &str) -> bool {
    if line.is_empty() {
        return false;
    }
    line.starts_with("  -")
        || line.starts_with("      --")
        || line.starts_with("  <")
        || !line.starts_with(' ')
}

fn tos_registry_help_examples(effective_args: &[String]) -> Option<Vec<String>> {
    if !is_tos_invocation(effective_args) {
        return None;
    }
    let command = recovered_command_path(effective_args, "ve-tos");
    if command == "ve-tos" {
        return None;
    }
    let row = ve_tos_cli::registry::capability_row_for_command(&command, false)?;
    if row.examples.is_empty() {
        Some(vec![format!(
            "{} --describe",
            public_tos_example_command(&command)
        )])
    } else {
        Some(row.examples)
    }
}

fn public_tos_example_command(command: &str) -> String {
    ve_tos_cli::registry::public_tos_command(command)
}

fn maybe_emit_byted_tos_describe_recovery(effective_args: &[String]) -> bool {
    if !has_flag(effective_args, "--describe")
        || tool_position(effective_args, "byted-tos").is_none()
    {
        return false;
    }
    let _config_guard = EnvGuard::set(TOS_CONFIG_BINARY_ENV, "tos");
    let global = recovered_global_args(effective_args);
    let command = recovered_command_path(effective_args, "byted-tos");
    let data = if command == "tos" {
        serde_json::json!({
            "tool": "tos",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "ByteCloud TOS CLI high-level object storage workflows and utilities",
            "uri_format": "tos://bucket/key",
            "implemented_layers": ["high_level", "utilities"],
            "unimplemented_layers": ["low_level"],
            "listing_semantics": {"delimiter": "/"},
        })
    } else if let Some(desc) = tos_cli::handler::meta::describe_tos_command_metadata(&command) {
        serde_json::to_value(desc).unwrap_or_else(|_| serde_json::json!({}))
    } else if let Some(row) = tos_cli::registry::find_capability(&command) {
        serde_json::json!({
            "command": row.command,
            "layer": row.layer,
            "description": row.description,
            "risk_level": row.risk_level,
            "supports_dry_run": row.supports_dry_run,
            "supports_force": row.supports_force,
            "parameters": row.parameters,
            "examples": row.examples
                .iter()
                .map(|example| tos_cli::registry::public_tos_command(example))
                .collect::<Vec<_>>(),
        })
    } else {
        return false;
    };
    let envelope = tos_core::agent::envelope::Envelope::success(command, data);
    let _ = ve_tos_cli::handler::common::output_result(&global, &envelope);
    true
}

fn maybe_emit_tos_describe_recovery(effective_args: &[String]) -> bool {
    if !has_flag(effective_args, "--describe") || tool_position(effective_args, "ve-tos").is_none()
    {
        return false;
    }
    let _config_guard = EnvGuard::set(TOS_CONFIG_BINARY_ENV, "ve-tos");
    let global = recovered_global_args(effective_args);
    let command = recovered_command_path(effective_args, "ve-tos");
    if is_tos_group_describe_command(&command) {
        return false;
    }
    let data = if command == "ve-tos" {
        ve_tos_cli::registry::describe_tos_group()
    } else if let Some(desc) = ve_tos_cli::registry::describe_command_metadata(&command) {
        serde_json::to_value(desc).unwrap_or_else(|_| serde_json::json!({}))
    } else if let Some(group) = command
        .split_whitespace()
        .nth(1)
        .and_then(ve_tos_cli::registry::find_group)
    {
        serde_json::json!({
            "command": group.command,
            "kind": "group",
            "layer": format!("{:?}", group.layer).to_lowercase(),
            "category": group.category,
            "description": group.description,
            "supports_help": group.supports_help,
            "supports_describe": group.supports_describe,
            "implemented": group.implemented,
        })
    } else {
        return false;
    };
    let envelope = tos_core::agent::envelope::Envelope::success(command, data);
    let _ = ve_tos_cli::handler::common::output_result(&global, &envelope);
    true
}

fn is_tos_group_describe_command(command: &str) -> bool {
    if command == "ve-tos" {
        return true;
    }
    if ve_tos_cli::registry::capability_row_for_command(command, false).is_some() {
        return false;
    }
    command
        .strip_prefix("ve-tos ")
        .and_then(ve_tos_cli::registry::find_group)
        .map(|group| group.command == command)
        .unwrap_or(false)
}

fn maybe_emit_adrive_describe_recovery(effective_args: &[String]) -> bool {
    if !has_flag(effective_args, "--describe")
        || tool_position(effective_args, "ve-adrive").is_none()
    {
        return false;
    }
    let global = recovered_global_args(effective_args);
    let command = recovered_command_path(effective_args, "ve-adrive");
    let mut data = if command == "ve-adrive" {
        serde_json::json!({
            "tool": "ve-adrive",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "ADrive CLI high-level file operations and agent utilities",
            "uri_format": "adrive://instance/space/folder/file",
            "implemented_layers": ["high_level", "utilities"],
            "unimplemented_layers": ["raw_api_execution"],
            "groups": [
                {
                    "name": "high_level",
                    "command": "ve-adrive",
                    "description": "File management operations with adrive:// URI and flag targets"
                },
                {
                    "name": "utilities",
                    "command": "ve-adrive capabilities",
                    "description": "Discovery, configuration, diagnostics, completion, skill, API passthrough, and MCP utilities"
                }
            ]
        })
    } else if let Some(desc) =
        ve_adrive_cli::handler::high_level::describe_high_level_command_path(&command)
    {
        serde_json::to_value(desc).unwrap_or_else(|_| serde_json::json!({}))
    } else if let Some(desc) =
        ve_adrive_cli::handler::meta::describe_adrive_command_metadata(&command)
    {
        desc
    } else if command.starts_with("ve-adrive api") {
        serde_json::json!({
            "command": command,
            "layer": "meta",
            "description": "Guarded ADrive utility API planning; direct raw execution is not implemented yet",
            "risk_level": "high",
            "supports_dry_run": true,
            "supports_pipe": false,
            "mode": "guarded_utility_passthrough",
            "raw_api_execution_implemented": false,
        })
    } else if let Some(row) = ve_adrive_cli::registry::find_capability(&command) {
        serde_json::json!({
            "command": row.command,
            "layer": row.layer.replace('-', "_"),
            "description": row.description,
            "risk_level": row.risk_level,
            "supports_dry_run": true,
            "supports_pipe": false,
            "parameters": row.parameters,
            "examples": row.examples
                .iter()
                .map(|example| public_adrive_example(example))
                .collect::<Vec<_>>(),
        })
    } else {
        return false;
    };
    ve_adrive_cli::handler::common::publicize_adrive_output_value(&mut data);
    let envelope = tos_core::agent::envelope::Envelope::success(
        ve_adrive_cli::handler::common::public_adrive_command_path(&command),
        data,
    );
    let _ = ve_tos_cli::handler::common::output_result(&global, &envelope);
    true
}

fn public_adrive_example(example: &str) -> String {
    public_adrive_command(example)
}

fn public_adrive_command(command: &str) -> String {
    let prefix =
        std::env::var(ADRIVE_EXAMPLE_PREFIX_ENV).unwrap_or_else(|_| "ve-adrive-cli".to_string());
    command
        .strip_prefix("ve-adrive ")
        .or_else(|| command.strip_prefix("ve-adrive-cli "))
        .or_else(|| command.strip_prefix("ve-storage-uni-cli ve-adrive "))
        .map(|suffix| format!("{prefix} {suffix}"))
        .unwrap_or_else(|| command.to_string())
}

fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|arg| arg == flag)
}

fn tool_position(args: &[String], tool: &str) -> Option<usize> {
    args.iter()
        .skip(1)
        .position(|arg| arg_matches_tool(arg, tool))
        .map(|pos| pos + 1)
}

fn arg_matches_tool(arg: &str, tool: &str) -> bool {
    match tool {
        "byted-tos" => arg == "tos",
        other => arg == other,
    }
}

fn recovered_command_path(effective_args: &[String], tool: &str) -> String {
    let Some(tool_idx) = tool_position(effective_args, tool) else {
        return tool.to_string();
    };
    let mut tokens = Vec::new();
    let mut iter = effective_args[(tool_idx + 1)..].iter().peekable();
    while let Some(arg) = iter.next() {
        if arg.starts_with('-') {
            if flag_takes_value(arg.as_str()) {
                let _ = iter.next();
            }
            continue;
        }
        tokens.push(arg.as_str());
    }
    if tokens.is_empty() {
        return canonical_tool_command(tool).to_string();
    }
    if tool == "byted-tos" {
        for len in (1..=tokens.len()).rev() {
            let candidate = format!("tos {}", tokens[..len].join(" "));
            if tos_cli::registry::find_capability(&candidate).is_some() {
                return candidate;
            }
        }
        return format!("tos {}", tokens.join(" "));
    }
    if tool == "ve-tos" {
        for len in (1..=tokens.len()).rev() {
            let candidate = format!("ve-tos {}", tokens[..len].join(" "));
            if ve_tos_cli::registry::capability_row_for_command(&candidate, false).is_some()
                || ve_tos_cli::registry::find_command_tree_entry(&candidate).is_some()
                || tokens
                    .first()
                    .and_then(|group| ve_tos_cli::registry::find_group(group))
                    .map(|group| group.command == candidate)
                    .unwrap_or(false)
            {
                return candidate;
            }
        }
    }
    format!("{} {}", canonical_tool_command(tool), tokens.join(" "))
}

fn recovered_global_args(effective_args: &[String]) -> GlobalArgs {
    let mut global = GlobalArgs::default();
    global.describe = has_flag(effective_args, "--describe");
    global.dry_run = has_flag(effective_args, "--dry-run");
    global.yes = has_flag(effective_args, "--yes") || has_flag(effective_args, "-y");
    global.verbose = has_flag(effective_args, "--verbose") || has_flag(effective_args, "-v");
    global.quiet = has_flag(effective_args, "--quiet") || has_flag(effective_args, "-q");
    let mut iter = effective_args.iter().peekable();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--output" | "-o" => {
                if let Some(value) = iter.next() {
                    global.output = parse_output_format(value);
                }
            }
            "--query" => {
                global.query = iter.next().cloned();
            }
            "--profile" | "-P" => {
                if let Some(value) = iter.next() {
                    global.profile = value.clone();
                }
            }
            "--config-path" => {
                if let Some(value) = iter.next() {
                    global.config_path = Some(value.into());
                }
            }
            "--region" | "-r" => global.region = iter.next().cloned(),
            "--endpoint" | "-e" => global.endpoint = iter.next().cloned(),
            "--psm" => global.psm = iter.next().cloned(),
            "--idc" => global.idc = iter.next().cloned(),
            "--cluster" => global.cluster = iter.next().cloned(),
            "--addr-family" | "--addr_family" => global.addr_family = iter.next().cloned(),
            "--control-endpoint" => global.control_endpoint = iter.next().cloned(),
            "--account-id" => global.account_id = iter.next().cloned(),
            "--confirm" => global.confirm = iter.next().cloned(),
            "--trace-dir" => global.trace_dir = iter.next().cloned(),
            "--trace-redact" => {
                if let Some(value) = iter.next() {
                    global.trace_redact = value.clone();
                }
            }
            "--no-color" => {
                global.no_color = true;
                if iter
                    .peek()
                    .map(|value| {
                        matches!(value.as_str(), "true" | "false" | "1" | "0" | "on" | "off")
                    })
                    .unwrap_or(false)
                {
                    if let Some(value) = iter.next() {
                        global.no_color = matches!(value.as_str(), "true" | "1" | "on");
                    }
                }
            }
            _ => {}
        }
    }
    global
}

fn flag_takes_value(flag: &str) -> bool {
    matches!(
        flag,
        "--output"
            | "-o"
            | "--query"
            | "--profile"
            | "-P"
            | "--config-path"
            | "--region"
            | "-r"
            | "--endpoint"
            | "-e"
            | "--psm"
            | "--idc"
            | "--cluster"
            | "--addr-family"
            | "--addr_family"
            | "--control-endpoint"
            | "--account-id"
            | "--confirm"
            | "--trace-dir"
            | "--trace-redact"
            | "--language"
            | "--help-language"
    )
}

fn parse_output_format(value: &str) -> Option<tos_core::agent::output::OutputFormat> {
    use tos_core::agent::output::OutputFormat;
    match value {
        "json" => Some(OutputFormat::Json),
        "xml" => Some(OutputFormat::Xml),
        "table" => Some(OutputFormat::Table),
        "csv" => Some(OutputFormat::Csv),
        "yaml" => Some(OutputFormat::Yaml),
        "markdown" => Some(OutputFormat::Markdown),
        _ => None,
    }
}

fn is_tos_invocation(effective_args: &[String]) -> bool {
    tool_position(effective_args, "ve-tos").is_some()
}

fn is_byted_tos_invocation(effective_args: &[String]) -> bool {
    tool_position(effective_args, "byted-tos").is_some()
}

fn is_adrive_invocation(effective_args: &[String]) -> bool {
    tool_position(effective_args, "ve-adrive").is_some()
}

fn emit_parse_error(effective_args: &[String], tool: &str, err: &clap::Error) {
    use tos_core::agent::envelope::{Envelope, ErrorDetail, ErrorKind};
    use tos_core::agent::error::AgentErrorCategory;
    use tos_core::agent::error::ExitCode;

    let exit_code = ExitCode::ValidationError;
    let public_tool = public_tool_name(tool);
    // [Review Fix #1] Parse-time errors now also expose the Agent decision
    // fields required by the stable schema.
    let suggested_action = format!("validate arguments, then run {public_tool} <command> --help");
    let envelope = Envelope::<()>::failed(
        parse_error_command(effective_args, tool),
        ErrorDetail {
            status_code: None,
            code: "ValidationError".to_string(),
            message: err.to_string(),
            exit_code: exit_code.as_i32(),
            kind: ErrorKind::ValidationError,
            category: AgentErrorCategory::InvalidParam,
            suggested_action: Some(suggested_action),
            fix_command: Some(format!("{public_tool} <command> --help")),
            doctor_hint: Some(format!("{public_tool} capabilities --view groups")),
            docs_url: if tool == "tos" {
                Some("https://www.volcengine.com/docs/6349".to_string())
            } else {
                None
            },
        },
    );
    let mut value = serde_json::to_value(&envelope).unwrap_or_default();
    if tool == "ve-adrive" {
        ve_adrive_cli::handler::common::publicize_adrive_output_value(&mut value);
    }
    eprintln!(
        "{}",
        serde_json::to_string_pretty(&value).unwrap_or_default()
    );
}

fn parse_error_command(effective_args: &[String], tool: &str) -> String {
    let mut command = Vec::new();
    let mut seen_tool = false;
    for arg in effective_args.iter().skip(1) {
        if !seen_tool {
            if arg_matches_tool(arg, tool) {
                seen_tool = true;
                command.push(canonical_tool_command(tool).to_string());
            }
            continue;
        }
        if arg.starts_with('-') {
            break;
        }
        command.push(arg.clone());
        if command.len() >= 3 {
            break;
        }
    }
    let command_path = if command.is_empty() {
        canonical_tool_command(tool).to_string()
    } else {
        command.join(" ")
    };
    if tool == "ve-adrive" {
        ve_adrive_cli::handler::common::public_adrive_command_path(&command_path)
    } else {
        command_path
    }
}

async fn handle_byted_tos(global: GlobalArgs, command: tos_cli::TosCliCommand) {
    use tos_core::agent::envelope::{Envelope, ErrorDetail, ErrorKind};
    use tos_core::agent::error::ExitCode;

    let _config_guard = EnvGuard::set(TOS_CONFIG_BINARY_ENV, "tos");
    let command_path = tos_cli::command_path(&command);
    let exit_code = match async {
        reject_control_plane_globals(&global, "tos")?;
        handle_byted_tos_inner(&global, command).await
    }
    .await
    {
        Ok(code) => code,
        Err(err) => {
            let exit_code = err.exit_code();
            let semantics = err.agent_semantics();
            let error_detail = ErrorDetail {
                status_code: semantics.status_code,
                code: semantics.code.clone(),
                message: semantics.message.clone(),
                exit_code: exit_code.as_i32(),
                kind: match exit_code {
                    ExitCode::AuthFailed => ErrorKind::AuthFailed,
                    ExitCode::ConfigMissing => ErrorKind::ConfigMissing,
                    ExitCode::ResourceNotFound => ErrorKind::ResourceNotFound,
                    ExitCode::PermissionDenied => ErrorKind::PermissionDenied,
                    ExitCode::ValidationError => ErrorKind::ValidationError,
                    ExitCode::RateLimited => ErrorKind::RateLimited,
                    ExitCode::TransferFailed => ErrorKind::TransferFailed,
                    ExitCode::Conflict => ErrorKind::Conflict,
                    _ => ErrorKind::Unknown,
                },
                category: semantics.category,
                suggested_action: Some(semantics.suggested_action.clone()),
                fix_command: suggest_byted_tos_fix(&err, &command_path),
                doctor_hint: Some("tos doctor".to_string()),
                docs_url: None,
            };
            let mut envelope = Envelope::<()>::failed(command_path, error_detail);
            if let Some(request_id) = semantics.request_id {
                envelope = envelope.with_request_id(request_id);
            }
            eprintln!(
                "{}",
                serde_json::to_string_pretty(&envelope).unwrap_or_default()
            );
            exit_code.as_i32()
        }
    };

    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}

fn reject_control_plane_globals(
    global: &GlobalArgs,
    tool: &str,
) -> Result<(), tos_core::agent::error::CliError> {
    use tos_core::agent::error::CliError;

    if global.control_endpoint.is_some() {
        return Err(CliError::ValidationError(format!(
            "{tool} does not support --control-endpoint"
        )));
    }
    if global.account_id.is_some() {
        return Err(CliError::ValidationError(format!(
            "{tool} does not support --account-id"
        )));
    }
    Ok(())
}

fn reject_psm_globals(
    global: &GlobalArgs,
    tool: &str,
) -> Result<(), tos_core::agent::error::CliError> {
    use tos_core::agent::error::CliError;

    for (flag, present) in [
        ("--psm", global.psm.is_some()),
        ("--idc", global.idc.is_some()),
        ("--cluster", global.cluster.is_some()),
        ("--addr-family", global.addr_family.is_some()),
    ] {
        if present {
            return Err(CliError::ValidationError(format!(
                "{tool} does not support {flag}"
            )));
        }
    }
    Ok(())
}

async fn handle_byted_tos_inner(
    global: &GlobalArgs,
    command: tos_cli::TosCliCommand,
) -> Result<i32, tos_core::agent::error::CliError> {
    match command {
        tos_cli::TosCliCommand::Cp(args) => {
            tos_cli::handler::high_level::handle_high_level_command(
                global,
                tos_cli::TosCliCommand::Cp(args),
            )
            .await
        }
        tos_cli::TosCliCommand::Mv(args) => {
            tos_cli::handler::high_level::handle_high_level_command(
                global,
                tos_cli::TosCliCommand::Mv(args),
            )
            .await
        }
        tos_cli::TosCliCommand::Sync(args) => {
            tos_cli::handler::high_level::handle_high_level_command(
                global,
                tos_cli::TosCliCommand::Sync(args),
            )
            .await
        }
        tos_cli::TosCliCommand::Mkdir(args) => {
            tos_cli::handler::high_level::handle_high_level_command(
                global,
                tos_cli::TosCliCommand::Mkdir(args),
            )
            .await
        }
        tos_cli::TosCliCommand::Rm(args) => {
            tos_cli::handler::high_level::handle_high_level_command(
                global,
                tos_cli::TosCliCommand::Rm(args),
            )
            .await
        }
        tos_cli::TosCliCommand::Ls(args) => {
            tos_cli::handler::high_level::handle_high_level_command(
                global,
                tos_cli::TosCliCommand::Ls(args),
            )
            .await
        }
        tos_cli::TosCliCommand::Stat(args) => {
            tos_cli::handler::high_level::handle_high_level_command(
                global,
                tos_cli::TosCliCommand::Stat(args),
            )
            .await
        }
        tos_cli::TosCliCommand::Du(args) => {
            tos_cli::handler::high_level::handle_high_level_command(
                global,
                tos_cli::TosCliCommand::Du(args),
            )
            .await
        }
        tos_cli::TosCliCommand::Find(args) => {
            tos_cli::handler::high_level::handle_high_level_command(
                global,
                tos_cli::TosCliCommand::Find(args),
            )
            .await
        }
        tos_cli::TosCliCommand::Cat(args) => {
            tos_cli::handler::high_level::handle_high_level_command(
                global,
                tos_cli::TosCliCommand::Cat(args),
            )
            .await
        }
        tos_cli::TosCliCommand::Put(args) => {
            tos_cli::handler::high_level::handle_high_level_command(
                global,
                tos_cli::TosCliCommand::Put(args),
            )
            .await
        }
        tos_cli::TosCliCommand::Presign(args) => {
            tos_cli::handler::high_level::handle_high_level_command(
                global,
                tos_cli::TosCliCommand::Presign(args),
            )
            .await
        }
        tos_cli::TosCliCommand::Capabilities(args) => {
            tos_cli::handler::meta::handle_capabilities_command(global, &args).await
        }
        tos_cli::TosCliCommand::Api(args) => {
            tos_cli::handler::meta::handle_api_command(global, &args).await
        }
        tos_cli::TosCliCommand::Config(cmd) => {
            tos_cli::handler::meta::handle_config_command(global, &cmd).await
        }
        tos_cli::TosCliCommand::Completion(args) => {
            tos_cli::handler::meta::handle_completion_command(global, &args).await
        }
        tos_cli::TosCliCommand::Serve(args) => {
            tos_cli::handler::meta::handle_serve_command(global, &args).await
        }
        tos_cli::TosCliCommand::Skill(cmd) => {
            tos_cli::handler::meta::handle_skill_command(global, &cmd).await
        }
        tos_cli::TosCliCommand::Doctor(args) => {
            tos_cli::handler::meta::handle_doctor_command(global, &args).await
        }
    }
}

fn suggest_byted_tos_fix(
    err: &tos_core::agent::error::CliError,
    command_path: &str,
) -> Option<String> {
    use tos_core::agent::error::CliError;
    match err {
        CliError::ConfigMissing(_) => Some("tos config init".to_string()),
        CliError::AuthFailed(_) => Some("tos config init (reconfigure credentials)".to_string()),
        CliError::ValidationError(_) => {
            let public_command = command_path.strip_prefix("tos ").unwrap_or(command_path);
            Some(format!("tos {public_command} --help"))
        }
        _ => None,
    }
}

async fn handle_tos(global: GlobalArgs, command: Option<ve_tos_cli::TosCommand>) {
    use tos_core::agent::envelope::{Envelope, ErrorDetail, ErrorKind};
    use tos_core::agent::error::ExitCode;

    let _config_guard = EnvGuard::set(TOS_CONFIG_BINARY_ENV, "ve-tos");
    let exit_code = match handle_tos_inner(&global, &command).await {
        Ok(code) => code,
        Err(err) => {
            let exit_code = err.exit_code();
            let semantics = err.agent_semantics();
            let error_detail = ErrorDetail {
                status_code: semantics.status_code,
                code: semantics.code.clone(),
                message: semantics.message.clone(),
                exit_code: exit_code.as_i32(),
                kind: match exit_code {
                    ExitCode::AuthFailed => ErrorKind::AuthFailed,
                    ExitCode::ConfigMissing => ErrorKind::ConfigMissing,
                    ExitCode::ResourceNotFound => ErrorKind::ResourceNotFound,
                    ExitCode::PermissionDenied => ErrorKind::PermissionDenied,
                    ExitCode::ValidationError => ErrorKind::ValidationError,
                    ExitCode::RateLimited => ErrorKind::RateLimited,
                    ExitCode::TransferFailed => ErrorKind::TransferFailed,
                    ExitCode::Conflict => ErrorKind::Conflict,
                    _ => ErrorKind::Unknown,
                },
                category: semantics.category,
                suggested_action: Some(semantics.suggested_action.clone()),
                fix_command: suggest_fix(&err),
                doctor_hint: Some("ve-tos capabilities --view groups".to_string()),
                docs_url: Some("https://www.volcengine.com/docs/6349".to_string()),
            };
            let mut envelope =
                Envelope::<()>::failed(ve_tos_cli::command_path(&command), error_detail);
            if let Some(request_id) = semantics.request_id {
                envelope = envelope.with_request_id(request_id);
            }
            eprintln!(
                "{}",
                serde_json::to_string_pretty(&envelope).unwrap_or_default()
            );
            exit_code.as_i32()
        }
    };

    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}

async fn handle_tos_inner(
    global: &GlobalArgs,
    command: &Option<ve_tos_cli::TosCommand>,
) -> Result<i32, tos_core::agent::error::CliError> {
    reject_psm_globals(global, "ve-tos")?;
    if command.is_none() {
        if global.describe {
            ve_tos_cli::handler::common::output_result(
                global,
                &ve_tos_cli::registry::describe_tos_group(),
            )?;
            return Ok(0);
        }
        return Err(tos_core::agent::error::CliError::ValidationError(
            "`ve-tos` requires a subcommand; use `ve-tos --help` or `ve-tos --describe`"
                .to_string(),
        ));
    }

    let command = command.as_ref().expect("checked above");

    match command {
        ve_tos_cli::TosCommand::Bucket(cmd) => {
            ve_tos_cli::handler::bucket::handle_bucket_command(global, &cmd.action).await
        }
        ve_tos_cli::TosCommand::Object(cmd) => {
            ve_tos_cli::handler::object::handle_object_command(global, &cmd.action).await
        }
        ve_tos_cli::TosCommand::Multipart(cmd) => {
            ve_tos_cli::handler::multipart::handle_multipart_command(global, &cmd.action).await
        }
        ve_tos_cli::TosCommand::Turbo(cmd) => {
            ve_tos_cli::handler::turbo::handle_turbo_command(global, &cmd.action).await
        }
        ve_tos_cli::TosCommand::Quota(cmd) => {
            ve_tos_cli::handler::bucket_config::handle_quota_command(global, &cmd.action).await
        }
        ve_tos_cli::TosCommand::Policy(cmd) => {
            ve_tos_cli::handler::bucket_config::handle_policy_command(global, &cmd.action).await
        }
        ve_tos_cli::TosCommand::Lifecycle(cmd) => {
            ve_tos_cli::handler::bucket_config::handle_lifecycle_command(global, &cmd.action).await
        }
        ve_tos_cli::TosCommand::Storageclass(cmd) => {
            ve_tos_cli::handler::bucket_config::handle_storageclass_command(global, &cmd.action)
                .await
        }
        ve_tos_cli::TosCommand::Cors(cmd) => {
            ve_tos_cli::handler::bucket_config::handle_cors_command(global, &cmd.action).await
        }
        ve_tos_cli::TosCommand::Versioning(cmd) => {
            ve_tos_cli::handler::bucket_config::handle_versioning_command(global, &cmd.action).await
        }
        ve_tos_cli::TosCommand::Replication(cmd) => {
            ve_tos_cli::handler::bucket_config::handle_replication_command(global, &cmd.action)
                .await
        }
        ve_tos_cli::TosCommand::Encryption(cmd) => {
            ve_tos_cli::handler::bucket_config::handle_encryption_command(global, &cmd.action).await
        }
        ve_tos_cli::TosCommand::CustomDomain(cmd) => {
            ve_tos_cli::handler::bucket_config::handle_custom_domain_command(global, &cmd.action)
                .await
        }
        ve_tos_cli::TosCommand::Notification(cmd) => {
            ve_tos_cli::handler::bucket_config::handle_notification_command(global, &cmd.action)
                .await
        }
        ve_tos_cli::TosCommand::Website(cmd) => {
            ve_tos_cli::handler::bucket_config::handle_website_command(global, &cmd.action).await
        }
        ve_tos_cli::TosCommand::Mirror(cmd) => {
            ve_tos_cli::handler::bucket_config::handle_mirror_command(global, &cmd.action).await
        }
        ve_tos_cli::TosCommand::Inventory(cmd) => {
            ve_tos_cli::handler::bucket_config::handle_inventory_command(global, &cmd.action).await
        }
        ve_tos_cli::TosCommand::Tagging(cmd) => {
            ve_tos_cli::handler::bucket_config::handle_tagging_command(global, &cmd.action).await
        }
        ve_tos_cli::TosCommand::Acl(cmd) => {
            ve_tos_cli::handler::bucket_config::handle_acl_command(global, &cmd.action).await
        }
        ve_tos_cli::TosCommand::Rename(cmd) => {
            ve_tos_cli::handler::bucket_config::handle_rename_command(global, &cmd.action).await
        }
        ve_tos_cli::TosCommand::RealTimeLog(cmd) => {
            ve_tos_cli::handler::bucket_config::handle_real_time_log_command(global, &cmd.action)
                .await
        }
        ve_tos_cli::TosCommand::AccessMonitor(cmd) => {
            ve_tos_cli::handler::bucket_config::handle_access_monitor_command(global, &cmd.action)
                .await
        }
        ve_tos_cli::TosCommand::Worm(cmd) => {
            ve_tos_cli::handler::bucket_config::handle_worm_command(global, &cmd.action).await
        }
        ve_tos_cli::TosCommand::Trash(cmd) => {
            ve_tos_cli::handler::bucket_config::handle_trash_command(global, &cmd.action).await
        }
        ve_tos_cli::TosCommand::Payment(cmd) => {
            ve_tos_cli::handler::bucket_config::handle_payment_command(global, &cmd.action).await
        }
        ve_tos_cli::TosCommand::Logging(cmd) => {
            ve_tos_cli::handler::bucket_config::handle_logging_command(global, &cmd.action).await
        }
        ve_tos_cli::TosCommand::IntelligentTiering(cmd) => {
            ve_tos_cli::handler::bucket_config::handle_intelligent_tiering_command(
                global,
                &cmd.action,
            )
            .await
        }
        ve_tos_cli::TosCommand::TransferAcceleration(cmd) => {
            ve_tos_cli::handler::bucket_config::handle_transfer_acceleration_command(
                global,
                &cmd.action,
            )
            .await
        }
        ve_tos_cli::TosCommand::CdnNotification(cmd) => {
            ve_tos_cli::handler::bucket_config::handle_cdn_notification_command(global, &cmd.action)
                .await
        }
        ve_tos_cli::TosCommand::HttpsConfig(cmd) => {
            ve_tos_cli::handler::bucket_config::handle_https_config_command(global, &cmd.action)
                .await
        }
        ve_tos_cli::TosCommand::PayByTraffic(cmd) => {
            ve_tos_cli::handler::bucket_config::handle_pay_by_traffic_command(global, &cmd.action)
                .await
        }
        ve_tos_cli::TosCommand::MaxAge(cmd) => {
            ve_tos_cli::handler::bucket_config::handle_max_age_command(global, &cmd.action).await
        }
        ve_tos_cli::TosCommand::RedundancyTransition(cmd) => {
            ve_tos_cli::handler::bucket_config::handle_redundancy_transition_command(
                global,
                &cmd.action,
            )
            .await
        }
        ve_tos_cli::TosCommand::DataProcess(cmd) => {
            ve_tos_cli::handler::advanced::handle_data_process_command(global, &cmd.action).await
        }
        ve_tos_cli::TosCommand::ObjectSet(cmd) => {
            ve_tos_cli::handler::advanced::handle_object_set_command(global, &cmd.action).await
        }
        ve_tos_cli::TosCommand::Accelerator(cmd) => {
            ve_tos_cli::handler::advanced::handle_accelerator_command(global, &cmd.action).await
        }
        ve_tos_cli::TosCommand::Mrap(cmd) => {
            ve_tos_cli::handler::advanced::handle_mrap_command(global, &cmd.action).await
        }
        ve_tos_cli::TosCommand::Ap(cmd) => {
            ve_tos_cli::handler::advanced::handle_ap_command(global, &cmd.action).await
        }
        ve_tos_cli::TosCommand::Cap(cmd) => {
            ve_tos_cli::handler::advanced::handle_cap_command(global, &cmd.action).await
        }
        ve_tos_cli::TosCommand::Dataset(cmd) => {
            ve_tos_cli::handler::advanced::handle_dataset_command(global, &cmd.action).await
        }
        ve_tos_cli::TosCommand::Control(cmd) => {
            ve_tos_cli::handler::advanced::handle_control_command(global, &cmd.action).await
        }
        ve_tos_cli::TosCommand::Cp(_)
        | ve_tos_cli::TosCommand::Mv(_)
        | ve_tos_cli::TosCommand::Sync(_)
        | ve_tos_cli::TosCommand::Mb(_)
        | ve_tos_cli::TosCommand::Rb(_)
        | ve_tos_cli::TosCommand::Mkdir(_)
        | ve_tos_cli::TosCommand::Rm(_)
        | ve_tos_cli::TosCommand::Ls(_)
        | ve_tos_cli::TosCommand::Stat(_)
        | ve_tos_cli::TosCommand::Du(_)
        | ve_tos_cli::TosCommand::Find(_)
        | ve_tos_cli::TosCommand::Cat(_)
        | ve_tos_cli::TosCommand::Put(_)
        | ve_tos_cli::TosCommand::Presign(_)
        | ve_tos_cli::TosCommand::Restore(_) => {
            ve_tos_cli::handler::high_level::handle_high_level_command(global, command).await
        }
        ve_tos_cli::TosCommand::Config(cmd) => {
            ve_tos_cli::handler::config::handle_config_command(global, &cmd.action).await
        }
        ve_tos_cli::TosCommand::Capabilities(args) => {
            ve_tos_cli::handler::meta::handle_capabilities_command(global, args).await
        }
        ve_tos_cli::TosCommand::Api(args) => {
            ve_tos_cli::handler::meta::handle_api_command(global, args).await
        }
        ve_tos_cli::TosCommand::Skill(cmd) => {
            ve_tos_cli::handler::meta::handle_skill_command(global, cmd).await
        }
        ve_tos_cli::TosCommand::Completion(args) => {
            ve_tos_cli::handler::meta::handle_completion_command(global, args).await
        }
        ve_tos_cli::TosCommand::Serve(args) => {
            ve_tos_cli::handler::meta::handle_serve_command(global, args).await
        }
        ve_tos_cli::TosCommand::Doctor(args) => {
            ve_tos_cli::handler::meta::handle_doctor_command(global, args).await
        }
    }
}

fn suggest_fix(err: &tos_core::agent::error::CliError) -> Option<String> {
    use tos_core::agent::error::CliError;
    match err {
        CliError::ConfigMissing(_) => Some("ve-tos config init".to_string()),
        CliError::AuthFailed(_) => Some("ve-tos config init (reconfigure credentials)".to_string()),
        CliError::ResourceNotFound(msg) if msg.contains("NoSuchBucket") => {
            Some("ve-tos bucket create --bucket <name> --region <region>".to_string())
        }
        _ => None,
    }
}

async fn handle_adrive(global: GlobalArgs, command: ve_adrive_cli::ADriveCommand) {
    use tos_core::agent::envelope::{Envelope, ErrorDetail, ErrorKind};
    use tos_core::agent::error::ExitCode;

    let exit_code = match async {
        reject_control_plane_globals(&global, "ve-adrive")?;
        reject_psm_globals(&global, "ve-adrive")?;
        handle_adrive_inner(&global, &command).await
    }
    .await
    {
        Ok(code) => code,
        Err(err) => {
            let exit_code = err.exit_code();
            let semantics = err.agent_semantics();
            let error_detail = ErrorDetail {
                status_code: semantics.status_code,
                code: semantics.code.clone(),
                message: semantics.message.clone(),
                exit_code: exit_code.as_i32(),
                kind: match exit_code {
                    ExitCode::AuthFailed => ErrorKind::AuthFailed,
                    ExitCode::ConfigMissing => ErrorKind::ConfigMissing,
                    ExitCode::ResourceNotFound => ErrorKind::ResourceNotFound,
                    ExitCode::PermissionDenied => ErrorKind::PermissionDenied,
                    ExitCode::ValidationError => ErrorKind::ValidationError,
                    ExitCode::RateLimited => ErrorKind::RateLimited,
                    ExitCode::TransferFailed => ErrorKind::TransferFailed,
                    ExitCode::Conflict => ErrorKind::Conflict,
                    _ => ErrorKind::Unknown,
                },
                category: semantics.category,
                suggested_action: Some(semantics.suggested_action.clone()),
                fix_command: suggest_adrive_fix(&err, &command),
                doctor_hint: Some("ve-adrive doctor".to_string()),
                docs_url: None,
            };
            let command_path = ve_adrive_cli::handler::common::public_adrive_command_path(
                &ve_adrive_cli::command_path(&command),
            );
            let mut envelope = Envelope::<()>::failed(command_path, error_detail);
            if let Some(request_id) = semantics.request_id {
                envelope = envelope.with_request_id(request_id);
            }
            // [Review Fix #5] Runtime errors are emitted from the unified
            // entrypoint, so publicize them before printing just like normal
            // ADrive handler output.
            let mut value = serde_json::to_value(&envelope).unwrap_or(serde_json::Value::Null);
            ve_adrive_cli::handler::common::publicize_adrive_output_value(&mut value);
            eprintln!(
                "{}",
                serde_json::to_string_pretty(&value).unwrap_or_default()
            );
            exit_code.as_i32()
        }
    };

    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}

fn suggest_adrive_fix(
    err: &tos_core::agent::error::CliError,
    command: &ve_adrive_cli::ADriveCommand,
) -> Option<String> {
    use tos_core::agent::error::CliError;
    match err {
        CliError::ConfigMissing(_) => Some("ve-adrive config init".to_string()),
        CliError::AuthFailed(_) => {
            Some("ve-adrive config init (reconfigure IDS credentials)".to_string())
        }
        CliError::ValidationError(message) if message.contains("raw API execution") => {
            Some("ve-adrive api <group> <action> --dry-run".to_string())
        }
        CliError::ValidationError(_) => {
            let command_path = ve_adrive_cli::command_path(command);
            let public_command = command_path
                .strip_prefix("ve-adrive ")
                .unwrap_or(&command_path);
            Some(format!("ve-adrive {public_command} --help"))
        }
        _ => None,
    }
}

async fn handle_adrive_inner(
    global: &GlobalArgs,
    command: &ve_adrive_cli::ADriveCommand,
) -> Result<i32, tos_core::agent::error::CliError> {
    match command {
        ve_adrive_cli::ADriveCommand::Cp(_)
        | ve_adrive_cli::ADriveCommand::Mv(_)
        | ve_adrive_cli::ADriveCommand::Sync(_)
        | ve_adrive_cli::ADriveCommand::Crt(_)
        | ve_adrive_cli::ADriveCommand::Del(_)
        | ve_adrive_cli::ADriveCommand::Rm(_)
        | ve_adrive_cli::ADriveCommand::Ls(_)
        | ve_adrive_cli::ADriveCommand::Stat(_)
        | ve_adrive_cli::ADriveCommand::Du(_)
        | ve_adrive_cli::ADriveCommand::Find(_)
        | ve_adrive_cli::ADriveCommand::Cat(_)
        | ve_adrive_cli::ADriveCommand::Put(_)
        | ve_adrive_cli::ADriveCommand::Mkdir(_) => {
            ve_adrive_cli::handler::high_level::handle_high_level_command(global, command).await
        }
        ve_adrive_cli::ADriveCommand::Capabilities(args) => {
            ve_adrive_cli::handler::meta::handle_capabilities_command(global, args).await
        }
        ve_adrive_cli::ADriveCommand::Api(args) => {
            ve_adrive_cli::handler::meta::handle_api_command(global, args).await
        }
        ve_adrive_cli::ADriveCommand::Config(cmd) => {
            ve_adrive_cli::handler::meta::handle_config_command(global, cmd).await
        }
        ve_adrive_cli::ADriveCommand::Completion(args) => {
            ve_adrive_cli::handler::meta::handle_completion_command(global, args).await
        }
        ve_adrive_cli::ADriveCommand::Serve(args) => {
            ve_adrive_cli::handler::meta::handle_serve_command(global, args).await
        }
        ve_adrive_cli::ADriveCommand::Doctor(args) => {
            ve_adrive_cli::handler::meta::handle_doctor_command(global, args).await
        }
        ve_adrive_cli::ADriveCommand::Skill(cmd) => {
            ve_adrive_cli::handler::meta::handle_skill_command(global, cmd).await
        }
    }
}
