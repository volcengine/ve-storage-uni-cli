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

use serde::Serialize;
use serde_json::{json, Value};
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command as TokioCommand;
use tokio::time::timeout;
use tos_core::agent::dryrun::{DryRunResult, Impact};
use tos_core::agent::envelope::Envelope;
use tos_core::agent::error::CliError;
use tos_core::agent::global_args::GlobalArgs;
use tos_core::agent::output::OutputFormat;
use tos_core::infra::client::storage_user_agent;
use tos_core::infra::config::{
    overlay_effective_credentials, overlay_effective_oauth_credentials, redact_effective,
    AdriveOverride, Binary, ConfigFile, EffectiveProfile, FieldSource,
    DEFAULT_HTTP_CONNECT_TIMEOUT_SECONDS, DEFAULT_HTTP_MAX_CONNECTIONS,
    DEFAULT_HTTP_MAX_RETRY_COUNT, DEFAULT_HTTP_REQUEST_TIMEOUT_SECONDS,
    DEFAULT_TOS_BATCH_REPORT_FORMAT, DEFAULT_TOS_PROGRESS_ENABLED,
};
use tos_core::infra::credentials::{CredentialSection, CredentialsFile};

use crate::cli::meta::{
    ApiArgs, CapabilitiesArgs, CompletionArgs, ConfigAction, ConfigCommand, DoctorArgs,
    DocumentationLanguage, ServeArgs, SkillAction, SkillCommand,
};
use crate::cli::ADriveAuthArgs;
use crate::domain::auth::{
    oauth_client_id_diagnostics, AuthMode, OAuthClientIdDiagnostics, ResolvedAuthMode,
};
use crate::domain::client::resolve_endpoint_and_region;
use crate::handler::common::{
    build_runtime_profile, inspect_selected_credentials, inspect_unified_credentials_for_profile,
    output_envelope, output_result, output_result_with_columns, public_adrive_command_path,
    UnifiedCredentialInspection,
};
use crate::registry::{
    business_domain, business_domains, capabilities, command_domains, find_capability,
    CapabilityRow,
};
#[derive(Debug, Serialize)]
struct SkillList {
    language: &'static str,
    skills: Vec<SkillDefinition>,
}

#[derive(Debug, Clone, Serialize)]
struct SkillDefinition {
    schema_version: &'static str,
    name: String,
    domain: String,
    command: String,
    #[serde(skip)]
    internal_command: String,
    description: String,
    risk_level: String,
    input_schema: Value,
    examples: Vec<String>,
    usage: SkillUsage,
}

#[derive(Debug, Clone, Serialize)]
struct SkillUsage {
    format: &'static str,
    source: &'static str,
    mcp_tool_name: String,
    mcp_server: String,
    serve_reads_exported_files: bool,
    exported_file_use: &'static str,
    default_mcp_call: &'static str,
}

#[derive(Debug, Serialize)]
struct McpCommandExecution {
    command: String,
    argv: Vec<String>,
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
}

const ADRIVE_EXAMPLE_PREFIX_ENV: &str = "VE_STORAGE_UNI_ADRIVE_EXAMPLE_PREFIX";
const ADRIVE_DEFAULT_CHECKPOINT_DIR: &str = "~/.tos/checkpoints/ve-adrive";
const ADRIVE_DEFAULT_BATCH_REPORT_DIR: &str = "~/.tos/reports/ve-adrive";

// Exact owner catalog: identifiers inside each full phrase remain unchanged.
const ADRIVE_METADATA_TRANSLATIONS_ZH: &[(&str, &str)] = &[
    // Capability descriptions.
    ("Copy local files, ADrive files, or folders", "复制本地文件、ADrive 文件或文件夹"),
    ("Move files or folders by same-space rename or copy plus source delete", "通过同空间重命名，或复制后删除源文件/文件夹来移动"),
    ("Move files or folders by copy plus source delete", "通过复制后删除源文件/文件夹来移动"),
    ("Synchronize source and destination incrementally", "增量同步源与目标"),
    ("Create an instance or space", "创建 Instance 或 Space"),
    ("Delete an instance or space", "删除 Instance 或 Space"),
    ("Delete a file, folder, or recursively clear a space", "删除文件、文件夹，或递归清空空间"),
    ("List instances, spaces, or files by target depth", "按目标层级列出 Instance、Space 或文件"),
    ("Show file or folder metadata", "查看文件或文件夹元数据"),
    ("Show instance, space, file, or folder metadata", "查看 Instance、Space、文件或文件夹元数据"),
    ("Calculate file size statistics for a folder", "统计文件夹的文件大小"),
    ("Find files by name, size, or mtime", "按名称、大小或 mtime 查找文件"),
    ("Stream file content", "流式输出文件内容"),
    ("Upload stdin to a file", "将 stdin 上传为文件"),
    ("Create a folder", "创建文件夹"),
    ("Discover CLI capabilities", "发现 CLI 能力"),
    ("Inspect API metadata; execution is unimplemented", "查看 API 元数据；暂不支持执行"),
    ("Manage ADrive CLI configuration", "管理 ADrive CLI 配置"),
    ("Generate shell completion scripts and installation snippets for ve-adrive-cli / ve-adrive", "为 ve-adrive-cli / ve-adrive 生成 shell 补全脚本和安装片段"),
    ("Start registry-backed MCP server over stdio or local HTTP/SSE", "通过 stdio 或本地 HTTP/SSE 启动由 registry 支持的 MCP 服务"),
    ("List ADrive skill metadata or export Markdown SKILL.md files for external Agents and adapters", "列出 ADrive Skill 元数据，或为外部 Agent 和适配器导出 Markdown SKILL.md 文件"),
    ("Environment diagnostics", "环境诊断"),
    // Shared target and transfer parameters.
    ("Treat ADrive instance/space target segments as names and resolve them to IDs", "将 ADrive instance/space 目标段视为名称并解析为 ID"),
    ("ADrive instance identifier", "ADrive Instance 标识符"),
    ("ADrive space identifier", "ADrive Space 标识符"),
    ("Folder path inside the space", "Space 内的文件夹路径"),
    ("File name inside the folder", "文件夹内的文件名"),
    ("Local path or adrive://instance/space/path source", "本地路径或 adrive://instance/space/path 源路径"),
    ("Local path or adrive://instance/space/path destination", "本地路径或 adrive://instance/space/path 目标路径"),
    ("Traverse folders recursively", "递归遍历文件夹"),
    ("Include the source directory or prefix name under the destination path", "在目标路径下包含源目录或前缀名称"),
    ("Include only paths matching this pattern during recursive transfers", "递归传输时仅包含匹配此模式的路径"),
    ("Exclude paths matching this pattern during recursive transfers", "递归传输时排除匹配此模式的路径"),
    ("Include only paths matching this pattern during recursive moves", "递归移动时仅包含匹配此模式的路径"),
    ("Exclude paths matching this pattern during recursive moves", "递归移动时排除匹配此模式的路径"),
    ("Include only paths matching this pattern", "仅包含匹配此模式的路径"),
    ("Exclude paths matching this pattern", "排除匹配此模式的路径"),
    ("Allow overwrite/delete operations without interactive confirmation", "允许覆盖/删除操作，无需交互确认"),
    ("Fail when the destination already exists", "目标已存在时失败"),
    ("Fail when the destination file already exists", "目标文件已存在时失败"),
    ("Destination overwrite strategy", "目标覆盖策略"),
    ("Enable resumable upload/download or recursive item checkpointing", "启用可恢复上传/下载或递归项目 checkpoint"),
    ("Directory for transfer checkpoint state", "传输 checkpoint 状态目录"),
    ("Directory reserved for transfer checkpoint state", "为传输 checkpoint 状态预留的目录"),
    ("File size threshold for checkpoint multipart/range transfer", "checkpoint 分片/范围传输的文件大小阈值"),
    ("Throttle upload/download bandwidth, e.g. 100MB", "限制上传/下载带宽，例如 100MB"),
    ("Maximum files/items running concurrently in batch execution", "批量执行时并发运行的最大文件/项目数"),
    ("Maximum folder prefixes listed concurrently in recursive batch commands", "递归批量命令中并发列出的最大文件夹前缀数"),
    ("Maximum parts/ranges running concurrently for one large file", "单个大文件并发运行的最大分片/范围数"),
    ("Progress granularity: part or byte", "进度粒度：part 或 byte"),
    ("Enable execution progress output on stderr", "在 stderr 启用执行进度输出"),
    ("Disable execution progress output on stderr", "在 stderr 禁用执行进度输出"),
    ("Enable listing-phase echo output on stderr", "在 stderr 启用列举阶段回显"),
    ("Disable listing-phase echo output on stderr", "在 stderr 禁用列举阶段回显"),
    ("Write planned transfer manifest CSV base path", "写入计划传输 manifest CSV 基础路径"),
    ("Disable planned manifest output", "禁用计划 manifest 输出"),
    ("Write success/failure report CSV base path", "写入成功/失败报告 CSV 基础路径"),
    ("Write only failed items to the batch report", "批量报告中仅写入失败项目"),
    // Create/delete parameters.
    ("adrive://instance-name or adrive://instance-id/space-name target", "adrive://instance-name 或 adrive://instance-id/space-name 目标"),
    ("Instance name to create, or existing instance ID when --space is set", "要创建的 Instance 名称；设置 --space 时为现有 Instance ID"),
    ("Space name to create under --instance", "在 --instance 下创建的 Space 名称"),
    ("Instance service type: saas, paas, or arkclaw; default is arkclaw for AK/SK and paas for OAuth", "Instance 服务类型：saas、paas 或 arkclaw；AK/SK 默认 arkclaw，OAuth 默认 paas"),
    ("Display name for the created instance or space", "所创建 Instance 或 Space 的显示名称"),
    ("Description for the created instance or space", "所创建 Instance 或 Space 的描述"),
    ("Enable search indexing for a newly-created space", "为新创建的 Space 启用搜索索引"),
    ("Space owner type: user or group; OAuth defaults to user and group requires --owner-id", "Space 所有者类型：user 或 group；OAuth 默认为 user，group 需要 --owner-id"),
    ("Space owner identifier; OAuth user ownership defaults to the logged-in user_id, while OAuth group ownership requires this option", "Space 所有者标识符；OAuth user 所有权默认为已登录 user_id，OAuth group 所有权需要此选项"),
    ("adrive://instance-id or adrive://instance-id/space-id target", "adrive://instance-id 或 adrive://instance-id/space-id 目标"),
    ("Instance ID to delete, or containing instance ID when --space is set", "要删除的 Instance ID；设置 --space 时为其所属 Instance ID"),
    ("Space ID to delete under --instance", "在 --instance 下删除的 Space ID"),
    ("Required safety gate for destructive deletion; non-interactive critical execution also requires global --confirm <target>", "破坏性删除的必需安全门；非交互 critical 执行还需要全局 --confirm <target>"),
    ("adrive://instance/space/folder[/file] target", "adrive://instance/space/folder[/file] 目标"),
    ("Delete folders recursively when supported", "在支持时递归删除文件夹"),
    ("Recursive folder delete strategy: bottom-up or direct", "递归文件夹删除策略：bottom-up 或 direct"),
    ("Include only paths matching this pattern during bottom-up recursive deletes", "自底向上递归删除时仅包含匹配此模式的路径"),
    ("Exclude paths matching this pattern during bottom-up recursive deletes", "自底向上递归删除时排除匹配此模式的路径"),
    ("Also abort incomplete multipart uploads recorded in ADrive checkpoints matching the target", "同时中止 ADrive checkpoint 中与目标匹配的未完成分片上传"),
    ("Checkpoint directory to scan when include-uploads is enabled", "启用 include-uploads 时扫描的 checkpoint 目录"),
    ("Maximum files/items running concurrently in this batch delete", "本次批量删除中并发运行的最大文件/项目数"),
    ("Maximum folder prefixes listed concurrently in recursive batch deletes", "递归批量删除中并发列出的最大文件夹前缀数"),
    ("Write planned delete manifest CSV base path", "写入计划删除 manifest CSV 基础路径"),
    // Listing, statistics, and stdin parameters.
    ("Optional adrive://instance[/space[/folder]] target", "可选的 adrive://instance[/space[/folder]] 目标"),
    ("List spaces under this instance when space is omitted", "省略 space 时列出此 Instance 下的 Space"),
    ("List files under this space when provided", "提供时列出此 Space 下的文件"),
    ("Folder prefix for file listing", "文件列举的文件夹前缀"),
    ("Maximum entries to return from the current directory level", "当前目录层级返回的最大条目数"),
    ("Pagination marker returned by a previous listing", "上一次列举返回的分页 marker"),
    ("OAuth Space collection: user (default) or group", "OAuth Space 集合：user（默认）或 group"),
    ("Comma-separated table/csv columns", "逗号分隔的 table/csv 列"),
    ("Sort field", "排序字段"),
    ("Render human-readable sizes", "以人类可读格式显示大小"),
    ("Optionally write listing manifest CSV base path", "可选写入列举 manifest CSV 基础路径"),
    ("Optional adrive://instance/space/folder target", "可选的 adrive://instance/space/folder 目标"),
    ("Maximum directory aggregation depth", "最大目录聚合深度"),
    ("Render human-readable total size", "以人类可读格式显示总大小"),
    ("Include estimated monthly storage cost by storage class", "包含按存储类型估算的每月存储成本"),
    ("Override storage price as CLASS=PRICE in CNY/GB/month", "以 CLASS=PRICE 覆盖存储价格，单位 CNY/GB/月"),
    ("Enable traversal echo output", "启用遍历回显输出"),
    ("Disable traversal echo output", "禁用遍历回显输出"),
    ("Legacy alias to enable traversal echo when list echo flags are absent", "未提供 list echo flags 时启用遍历回显的旧版别名"),
    ("Legacy alias to disable traversal echo when list echo flags are absent", "未提供 list echo flags 时禁用遍历回显的旧版别名"),
    ("Maximum folder prefixes listed concurrently while measuring recursively", "递归统计时并发列出的最大文件夹前缀数"),
    ("Number of largest and oldest file samples to keep in verbose diagnostics; 0 disables samples", "verbose 诊断中保留的最大和最旧文件样本数；0 禁用样本"),
    ("Optionally write traversed-file manifest CSV base path", "可选写入已遍历文件 manifest CSV 基础路径"),
    ("Name glob or substring", "名称 glob 或子字符串"),
    ("Size predicate such as +100MB or -1GB", "大小条件，例如 +100MB 或 -1GB"),
    ("Relative modified time predicate such as -7d", "相对修改时间条件，例如 -7d"),
    ("Optionally write matched-file manifest CSV base path", "可选写入匹配文件 manifest CSV 基础路径"),
    ("ADrive destination URI: adrive://instance/space/path", "ADrive 目标 URI：adrive://instance/space/path"),
    ("Content-Type for uploaded stdin", "上传 stdin 的 Content-Type"),
    ("Stdin size threshold for multipart upload; defaults to shared checkpoint_threshold", "stdin 分片上传的大小阈值；默认为共享 checkpoint_threshold"),
    ("Create parent folders as needed", "按需创建父文件夹"),
    // Sync, API, MCP, authentication, and common schema parameters.
    ("Delete extraneous destination files/folders", "删除目标中多余的文件/文件夹"),
    ("Required safety gate when --delete is enabled", "启用 --delete 时的必需安全门"),
    ("Compare by size only", "仅按大小比较"),
    ("Use exact timestamps for comparison", "使用精确时间戳比较"),
    ("IDS API group", "IDS API 分组"),
    ("IDS API action", "IDS API 操作"),
    ("JSON request body or file:// path", "JSON 请求体或 file:// 路径"),
    ("Reserved for future ADrive raw API execution; currently unimplemented", "为未来 ADrive 原始 API 执行保留；当前尚未实现"),
    ("Shell name: bash, zsh, fish, or powershell", "Shell 名称：bash、zsh、fish 或 powershell"),
    ("Enable the long-running MCP runtime", "启用长时间运行的 MCP runtime"),
    ("MCP transport: stdio or sse", "MCP 传输方式：stdio 或 sse"),
    ("SSE port; runtime binds 127.0.0.1:<port>", "SSE 端口；runtime 绑定 127.0.0.1:<port>"),
    ("Inspect authentication status or manage OAuth. Unified uses the same-name external profile, ignores local AK/SK and OAuth credentials, and delegates login/logout to `ve login` / `ve logout`", "查看鉴权状态或管理 OAuth。Unified 使用同名外部 profile，忽略本地 AK/SK 和 OAuth 凭证，并将登录/登出交由 `ve login` / `ve logout`"),
    ("Per-invocation override: --auth-mode <MODE>; supported values are aksk, oauth, or unified. ADRIVE_AUTH_MODE supplies the environment value", "单次调用覆盖：--auth-mode <MODE>；支持值为 aksk、oauth 或 unified。环境变量由 ADRIVE_AUTH_MODE 提供"),
    ("Authentication action: status (default), login, or logout", "鉴权操作：status（默认）、login 或 logout"),
    ("OAuth login Instance; required unless the selected profile or ADRIVE_DEFAULT_INSTANCE supplies it", "OAuth 登录 Instance；除非所选 profile 或 ADRIVE_DEFAULT_INSTANCE 已提供，否则必填"),
    ("OAuth Authorization Server for login; required unless the selected profile or ADRIVE_AUTH_ENDPOINT supplies it", "OAuth 登录授权服务器；除非所选 profile 或 ADRIVE_AUTH_ENDPOINT 已提供，否则必填"),
    ("Human-readable device name shown during OAuth authorization", "OAuth 授权期间显示的人类可读设备名称"),
    ("Unified uses the same-name external profile, ignores local AK/SK and OAuth credentials, and delegates login/logout to `ve login` / `ve logout`", "Unified 使用同名外部 profile，忽略本地 AK/SK 和 OAuth 凭证，并将登录/登出交由 `ve login` / `ve logout`"),
    ("Execute the CLI command; false returns a plan only", "执行 CLI 命令；false 时仅返回计划"),
    ("Pass global --dry-run to the CLI command", "向 CLI 命令传递全局 --dry-run"),
    ("Pass global --describe to the CLI command", "向 CLI 命令传递全局 --describe"),
    ("Configuration profile name", "配置 profile 名称"),
    ("Output format, defaults to json", "输出格式，默认为 json"),
    ("Optional global region override", "可选的全局 region 覆盖"),
    ("Optional global endpoint override", "可选的全局 endpoint 覆盖"),
    ("Include extra diagnostic output where supported", "在支持时包含额外诊断输出"),
    ("Disable prompts and progress output", "禁用提示和进度输出"),
    // Describe-only metadata phrases.
    ("Quote paths and JSON/JMESPath expressions that contain shell metacharacters.", "包含 shell 元字符的路径和 JSON/JMESPath 表达式需要加引号。"),
    ("The command returns an Envelope; extract payload fields from data.*.", "命令返回 Envelope；请从 data.* 提取负载字段。"),
    ("Generate shell completion scripts and installation snippets for ADrive CLI names.", "为 ADrive CLI 名称生成 shell 补全脚本和安装片段。"),
    ("Start the registry-backed ADrive MCP server.", "启动由 registry 支持的 ADrive MCP 服务。"),
    ("List ADrive skill metadata from the live registry.", "列出 live registry 中的 ADrive Skill 元数据。"),
    ("Export ADrive Markdown SKILL.md files for external consumers.", "为外部使用者导出 ADrive Markdown SKILL.md 文件。"),
    ("Documentation language for generated skill metadata: en (default) or zh", "生成 Skill 元数据的文档语言：en（默认）或 zh"),
    ("Optional skill name, command path, or business domain filter", "可选的 Skill 名称、命令路径或业务域过滤器"),
    ("Output directory for exported Markdown skill files", "导出 Markdown Skill 文件的输出目录"),
    ("Documentation language for generated SKILL.md files: en (default) or zh", "生成 SKILL.md 文件的文档语言：en（默认）或 zh"),
    // High-level and utility Describe routing prose; examples stay untouched.
    ("accept adrive://instance/space/path URI or --instance/--space/--folder/--file flags; --by-name resolves instance/space names to IDs before execution", "接受 adrive://instance/space/path URI 或 --instance/--space/--folder/--file flags；--by-name 在执行前将 instance/space 名称解析为 ID"),
    ("returns a deterministic plan without mutating local files or ADrive resources", "返回确定性计划，不修改本地文件或 ADrive 资源"),
    ("success and failure paths use Envelope plus --query and multi-format rendering", "成功和失败路径使用 Envelope，并支持 --query 与多格式渲染"),
    ("execution stderr; auto-enabled on TTY, disabled by --no-progress or --quiet, forced by --progress", "执行进度输出到 stderr；TTY 上自动启用，--no-progress 或 --quiet 禁用，--progress 强制启用"),
    ("listing stderr; auto-enabled on TTY, disabled by --no-list-echo or --quiet, forced by --list-echo", "列举回显输出到 stderr；TTY 上自动启用，--no-list-echo 或 --quiet 禁用，--list-echo 强制启用"),
    ("stable task fingerprint plus atomic lock when the command supports checkpoint state", "命令支持 checkpoint 状态时使用稳定任务指纹和原子锁"),
    ("ADrive Low-Level CLI is not implemented; High-Level commands wrap IDS API actions directly", "ADrive Low-Level CLI 尚未实现；High-Level 命令直接封装 IDS API 操作"),
    ("Quote ADrive paths that contain spaces or shell metacharacters: ve-adrive cp 'adrive://inst/space/path with space.txt' ./out.txt", "包含空格或 shell 元字符的 ADrive 路径需要加引号：ve-adrive cp 'adrive://inst/space/path with space.txt' ./out.txt"),
    ("JMESPath literals inside --query use backticks; keep the expression inside single quotes in POSIX shells.", "--query 内的 JMESPath 字面量使用反引号；在 POSIX shell 中请将表达式放在单引号内。"),
    ("Use --output json when piping ADrive output into jq or another parser.", "将 ADrive 输出通过管道传给 jq 或其他解析器时，请使用 --output json。"),
    ("the command returns an Envelope; install by extracting data.script, then source bash output, add ~/.zfunc to zsh fpath and run compinit, write fish output under ~/.config/fish/completions, or append PowerShell output to $PROFILE", "命令返回 Envelope；安装时提取 data.script，然后 source bash 输出、将 ~/.zfunc 加入 zsh fpath 并运行 compinit、把 fish 输出写入 ~/.config/fish/completions，或把 PowerShell 输出追加到 $PROFILE"),
    ("generated scripts register ve-adrive-cli and ve-adrive", "生成的脚本会注册 ve-adrive-cli 和 ve-adrive"),
    ("Instance: AK/SK defaults --service-type to arkclaw and OAuth defaults it to paas. Space: OAuth user Space ownership defaults to the logged-in user_id; --owner-type group requires --owner-id.", "Instance：AK/SK 的 --service-type 默认为 arkclaw，OAuth 默认为 paas。Space：OAuth user Space 所有权默认为已登录 user_id；--owner-type group 需要 --owner-id。"),
    ("critical delete paths require --force and, in non-interactive shells, exact --confirm <deleted-source-or-target>", "critical 删除路径需要 --force；在非交互 shell 中还需要精确的 --confirm <deleted-source-or-target>"),
    ("AK/SK no target -> list_instances collection; OAuth no target -> the credentials-bound single Instance, missing binding -> oauth_instance_required, and OAuth root rejects --marker; AK/SK instance target -> list_spaces; OAuth instance target -> list_my_spaces and --owner-type group -> list_my_group_spaces; instance/space[/folder] target -> list_files", "AK/SK 无目标 -> list_instances 集合；OAuth 无目标 -> 凭证绑定的单个 Instance，缺少绑定 -> oauth_instance_required，且 OAuth 根路径拒绝 --marker；AK/SK instance 目标 -> list_spaces；OAuth instance 目标 -> list_my_spaces，--owner-type group -> list_my_group_spaces；instance/space[/folder] 目标 -> list_files"),
    ("instance listing returns data.instances; space listing returns data.spaces; JSON file listing returns raw data.files/data.folders; table/csv render a synthesized typed row view", "Instance 列举返回 data.instances；Space 列举返回 data.spaces；JSON 文件列举返回原始 data.files/data.folders；table/csv 渲染合成的类型化行视图"),
    ("reads stdin and writes it to exactly one ADrive file target; pipe-friendly for cat | gzip | put", "读取 stdin 并写入恰好一个 ADrive 文件目标；适用于 cat | gzip | put 管道"),
    ("stdin input at or above --multipart-threshold uses initiate_multipart_upload + upload_part + complete_multipart_upload; failures abort_multipart_upload", "达到或超过 --multipart-threshold 的 stdin 输入使用 initiate_multipart_upload + upload_part + complete_multipart_upload；失败时执行 abort_multipart_upload"),
    ("rm accepts file/folder targets only; use ve-adrive del for instance or space deletion", "rm 仅接受文件/文件夹目标；删除 Instance 或 Space 请使用 ve-adrive del"),
    ("bottom-up mode lists children then deletes files before folders; direct mode asks the service to delete the folder target", "bottom-up 模式先列出子项，再先删文件后删文件夹；direct 模式请求服务删除文件夹目标"),
    ("sync --delete is critical: interactive shells may confirm or use --force; non-interactive shells require --force plus exact --confirm <destination>", "sync --delete 属于 critical：交互 shell 可确认或使用 --force；非交互 shell 需要 --force 和精确的 --confirm <destination>"),
    ("stdio uses stdin/stdout and opens no TCP listener; sse starts a local rmcp HTTP/SSE listener on 127.0.0.1:<port>", "stdio 使用 stdin/stdout 且不打开 TCP listener；sse 在 127.0.0.1:<port> 启动本地 rmcp HTTP/SSE listener"),
    ("MCP tools are rebuilt from the in-process skill registry; exported Markdown skill files are not read by serve", "MCP 工具由进程内 Skill registry 重建；serve 不读取导出的 Markdown Skill 文件"),
    ("tools/call plans by default; include execute=true to run the underlying CLI command", "tools/call 默认生成计划；包含 execute=true 才运行底层 CLI 命令"),
    ("Markdown SKILL.md pack with root index plus per-domain command skills", "包含根索引和按域命令 Skill 的 Markdown SKILL.md 包"),
    ("external Agent catalogs, prompt context, documentation generators, adapters, and MCP tool advertisement", "外部 Agent 目录、prompt 上下文、文档生成器、适配器和 MCP 工具声明"),
    ("serve uses the same live registry data but does not read the exported Markdown skill directory", "serve 使用相同的 live registry 数据，但不读取导出的 Markdown Skill 目录"),
    // [Review Fix #ADriveZh5] Root/config/raw Describe and Skill usage prose
    // lives outside capability rows but must share the exact owner catalog.
    ("ADrive CLI high-level file operations and agent utilities", "ADrive CLI 高层文件操作与 Agent 工具"),
    ("File management operations with adrive:// URI and flag targets", "使用 adrive:// URI 和 flags 目标的文件管理操作"),
    ("Discovery, configuration, diagnostics, completion, skill, API passthrough, and MCP utilities", "发现、配置、诊断、补全、Skill、API passthrough 和 MCP 工具"),
    ("Guarded ADrive utility API planning; direct raw execution is not implemented yet", "受保护的 ADrive 工具 API 规划；暂不支持直接执行原始 API"),
    ("Configuration management", "配置管理"),
    ("Initialize configuration", "初始化配置"),
    ("Show effective configuration", "显示生效配置"),
    ("Set configuration value", "设置配置值"),
    ("Derived from the live ADrive CLI capability registry.", "源自 live ADrive CLI capability registry。"),
    ("Portable Markdown skill pack for external agents, documentation generators, prompts, or adapters. The built-in MCP server rebuilds tools from the in-process registry instead of reading exported files.", "供外部 Agent、文档生成器、prompt 或适配器使用的 portable Markdown Skill 包。内置 MCP 服务从进程内 registry 重建工具，不读取导出文件。"),
    ("tools/call returns a plan by default; include argument execute=true to run the underlying CLI command.", "tools/call 默认返回计划；包含参数 execute=true 才运行底层 CLI 命令。"),
];

const ADRIVE_HIGH_LEVEL_SEMANTICS: &[(&str, &[&str])] = &[
    (
        "cp",
        &[
            "local path -> adrive://instance/space/path uploads with put_file or multipart upload",
            "adrive://instance/space/path -> local path downloads with get_file and atomic local persist",
            "adrive://instance/space/path -> adrive://instance/space/path copies with copy_file where service-side copy is available",
            "recursive transfers enumerate folders first and honor include/exclude filters",
        ],
    ),
    (
        "mv",
        &[
            "same instance and same space uses rename_file or rename_folder",
            "cross-space or local/remote moves run copy first, then delete the source after destination success",
            "critical source delete requires --force plus exact --confirm <source> in non-interactive shells",
        ],
    ),
    (
        "sync",
        &[
            "builds a source/destination diff from list_files, size, and mtime where available",
            "--delete removes extraneous destination files/folders and upgrades the command to critical risk",
            "transfer phases reuse cp overwrite, checkpoint, and report behavior",
        ],
    ),
    (
        "crt",
        &[
            "adrive://instance -> create_instance; --service-type accepts saas, paas, or arkclaw, defaulting to arkclaw for AK/SK and paas for OAuth",
            "adrive://instance/space -> create_space; OAuth defaults to user ownership with the logged-in user_id, while --owner-type group requires --owner-id",
        ],
    ),
    (
        "del",
        &[
            "adrive://instance -> delete_instance",
            "adrive://instance/space -> delete_space",
            "critical deletes require --force plus exact --confirm <target> in non-interactive shells",
        ],
    ),
    (
        "rm",
        &[
            "adrive://instance/space/path -> delete_file or delete_folder",
            "--recursive plans a folder traversal before deletion unless direct recursive mode is selected",
            "critical deletes require --force plus exact --confirm <target> in non-interactive shells",
        ],
    ),
    // [Review Fix #1] Root and Instance listing routes depend on the selected
    // authentication mode and must remain explicit for capability consumers.
    (
        "ls",
        &[
            "AK/SK no target -> list_instances collection",
            "OAuth no target -> the credentials-bound single Instance; missing binding -> oauth_instance_required; OAuth root rejects --marker",
            "AK/SK instance target -> list_spaces",
            "OAuth instance target -> list_my_spaces; --owner-type group -> list_my_group_spaces",
            "--instance + --space or adrive://instance/space[/folder] -> list_files",
        ],
    ),
    (
        "stat",
        &["adrive://instance/space/path -> head_file metadata for a file or folder"],
    ),
    (
        "du",
        &["adrive://instance/space/folder -> read-only list_files traversal with size, histogram, and optional cost summaries"],
    ),
    (
        "find",
        &["adrive://instance/space/folder -> read-only list_files traversal filtered by name, size, and mtime"],
    ),
    ("cat", &["adrive://instance/space/file -> get_file body streamed to stdout"]),
    ("put", &["stdin -> adrive://instance/space/file upload; multipart is used above the configured threshold"]),
    (
        "mkdir",
        &["adrive://instance/space/folder -> create_folder; --parents creates missing parent folders as needed"],
    ),
];

const CAPABILITY_GROUP_TABLE_COLUMNS: &[&str] = &[
    "name",
    "group",
    "command",
    "layer",
    "description",
    "implemented",
    "command_count",
];

const CAPABILITY_ROW_TABLE_COLUMNS: &[&str] = &[
    "command",
    "group",
    "layer",
    "description",
    "risk_level",
    "destructive",
    "supports_dry_run",
    "supports_force",
];

/// Handle ADrive capabilities.
pub async fn handle_capabilities_command(
    global: &GlobalArgs,
    args: &CapabilitiesArgs,
) -> Result<i32, CliError> {
    let rows = filtered_capabilities(args);
    let view = if args.view == "tree" {
        "full"
    } else {
        args.view.as_str()
    };
    let groups = capability_group_rows(&rows);
    let search_scores = capability_search_scores(args, &rows);
    let commands = capability_command_rows(&rows);
    let payload = match args.view.as_str() {
        "groups" => json!({
            "tool": "ve-adrive",
            "version": env!("CARGO_PKG_VERSION"),
            "service_name": "ids",
            "view": view,
            "groups": groups,
            "capabilities": [],
            "commands": [],
            "search_scores": search_scores,
            "uri_format": "adrive://instance/space/folder/file",
            "high_level_semantics": high_level_semantics(),
        }),
        "text" => json!({
            "tool": "ve-adrive",
            "version": env!("CARGO_PKG_VERSION"),
            "service_name": "ids",
            "view": view,
            "groups": groups,
            "capabilities": [],
            "commands": [],
            "lines": rows.iter().map(|row| {
                format!("{} [{}] - {}", row.command, row.risk_level, row.description)
            }).collect::<Vec<_>>(),
            "search_scores": search_scores,
        }),
        "compact" => json!({
            "tool": "ve-adrive",
            "version": env!("CARGO_PKG_VERSION"),
            "service_name": "ids",
            "view": view,
            "groups": groups,
            "capabilities": rows.iter().map(compact_capability).collect::<Vec<_>>(),
            "commands": commands,
            "search_scores": search_scores,
        }),
        "full" | "tree" => json!({
            "tool": "ve-adrive",
            "version": env!("CARGO_PKG_VERSION"),
            "service_name": "ids",
            "view": view,
            "groups": groups,
            "capabilities": rows.iter().map(public_capability_row).collect::<Vec<_>>(),
            "commands": commands,
            "search_scores": search_scores,
            "uri_format": "adrive://instance/space/folder/file",
            "parameters": {
                "instance": "A-Drive instance identifier",
                "space": "Space within the instance",
                "folder": "Folder path within the space",
                "file": "File name within the folder"
            },
            "high_level_semantics": high_level_semantics(),
        }),
        other => {
            return Err(CliError::ValidationError(format!(
                "unsupported capabilities view '{}': expected groups, text, compact, full, or tree",
                other
            )));
        }
    };
    output_result_with_columns(
        global,
        &Envelope::success("ve-adrive capabilities", payload),
        capabilities_table_columns(global, args.view.as_str()),
    )?;
    Ok(0)
}

fn filtered_capabilities(args: &CapabilitiesArgs) -> Vec<CapabilityRow> {
    let facet_filtered: Vec<&CapabilityRow> = capabilities()
        .iter()
        .filter(|row| {
            args.group
                .as_deref()
                .map(|group| capability_matches_group(row, group))
                .unwrap_or(true)
        })
        .filter(|row| {
            args.layer
                .as_deref()
                .map(|layer| normalize_facet(row.layer) == normalize_facet(layer))
                .unwrap_or(true)
        })
        .collect();

    let Some(term) = args.search.as_deref() else {
        return facet_filtered.into_iter().cloned().collect();
    };

    let mut scored: Vec<(CapabilityRow, f64)> = facet_filtered
        .into_iter()
        .filter_map(|row| {
            let candidates: Vec<&str> = std::iter::once(row.command)
                .chain(std::iter::once(row.description))
                .chain(row.api_actions.iter().copied())
                .collect();
            let score = rank_best(term, &candidates);
            if score >= 0.85 {
                Some((row.clone(), score))
            } else {
                None
            }
        })
        .collect();

    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().map(|(row, _)| row).collect()
}

/// Compute the best fuzzy match score for `term` against a set of candidate
/// strings. Uses Jaro-Winkler similarity with boosted scores for exact,
/// substring, and prefix matches — aligned with the TOS capabilities search.
fn rank_best(term: &str, candidates: &[&str]) -> f64 {
    let t_lower = term.to_lowercase();
    candidates
        .iter()
        .map(|c| {
            let c_lower = c.to_lowercase();
            if c_lower == t_lower {
                return 1.0;
            }
            if c_lower.contains(&t_lower) {
                return 0.92;
            }
            if c_lower.starts_with(&t_lower) {
                return 0.9;
            }
            strsim::jaro_winkler(&t_lower, &c_lower) as f64
        })
        .fold(0.0_f64, f64::max)
}

fn capability_group_rows(rows: &[CapabilityRow]) -> Vec<Value> {
    let mut groups = Vec::new();
    for (name, layer, command, description) in [
        (
            "High-Level",
            "high-level",
            "ve-adrive",
            "File management operations with adrive:// URI support",
        ),
        (
            "Capabilities / Utilities",
            "utility",
            "ve-adrive capabilities",
            "CLI configuration and introspection utilities",
        ),
    ] {
        let commands = rows
            .iter()
            .filter(|row| row.layer == layer || row.group == name)
            .map(|row| row.command)
            .collect::<Vec<_>>();
        if commands.is_empty() {
            continue;
        }
        let category = if layer == "utility" {
            "utilities".to_string()
        } else {
            layer.replace('-', "_")
        };
        groups.push(json!({
            "name": name,
            "group": category,
            "command": command,
            "layer": layer.replace('-', "_"),
            "category": category,
            "description": description,
            "implemented": true,
            "command_count": commands.len(),
            "commands": commands,
        }));
    }
    groups
}

fn capability_matches_group(row: &CapabilityRow, group: &str) -> bool {
    let requested = normalize_facet(group);
    let row_group = normalize_facet(row.group);
    let row_layer = normalize_facet(row.layer);
    let row_domain = normalize_facet(row.domain);
    let category = if row.layer == "utility" {
        "utilities".to_string()
    } else {
        row_layer.clone()
    };
    requested == row_group
        || requested == row_layer
        || requested == row_domain
        || requested == category
        || (requested == "capabilities_utilities" && row.layer == "utility")
}

fn normalize_facet(value: &str) -> String {
    let mut normalized = String::new();
    let mut last_was_sep = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            normalized.push(ch.to_ascii_lowercase());
            last_was_sep = false;
        } else if !last_was_sep {
            normalized.push('_');
            last_was_sep = true;
        }
    }
    normalized.trim_matches('_').to_string()
}

fn capability_command_rows(rows: &[CapabilityRow]) -> Vec<Value> {
    rows.iter()
        .map(|row| {
            json!({
                "name": row.domain,
                "command": row.command,
                "layer": row.layer.replace('-', "_"),
                "category": if row.layer == "utility" { "utilities" } else { row.layer },
                "description": row.description,
                "supports_help": true,
                "supports_describe": true,
                "implemented": true,
                "parameters": row.parameters,
                "subcommands": [],
            })
        })
        .collect()
}

fn capability_search_scores(args: &CapabilitiesArgs, rows: &[CapabilityRow]) -> Vec<Value> {
    let Some(term) = args.search.as_deref() else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|row| {
            let candidates: Vec<&str> = std::iter::once(row.command)
                .chain(std::iter::once(row.description))
                .chain(row.api_actions.iter().copied())
                .collect();
            let score = rank_best(term, &candidates);
            (score >= 0.85).then(|| {
                json!({
                    "kind": "capability",
                    "command": row.command,
                    "score": score,
                    "matched_field": "command_or_api",
                })
            })
        })
        .collect()
}

fn compact_capability(row: &CapabilityRow) -> Value {
    json!({
        "command": row.command,
        "domain": row.domain,
        "group": row.group,
        "layer": row.layer,
        "description": row.description,
        "risk_level": row.risk_level,
        "destructive": row.destructive,
        "supports_force": row.supports_force,
        // [Review Fix #3] Registry metadata is authoritative; the previous
        // hand-written allowlist incorrectly reported auth describe as false.
        "supports_dry_run": row.supports_dry_run,
        "api_actions": row.api_actions,
    })
}

fn high_level_semantics() -> Value {
    let mut semantics = serde_json::Map::new();
    for (command, lines) in ADRIVE_HIGH_LEVEL_SEMANTICS {
        semantics.insert((*command).to_string(), json!(*lines));
    }
    Value::Object(semantics)
}

fn capabilities_table_columns(global: &GlobalArgs, view: &str) -> Option<&'static [&'static str]> {
    if global.query.is_some() {
        // [Review Fix #4] Explicit JMESPath selection owns the table shape.
        return None;
    }
    match view {
        "groups" => Some(CAPABILITY_GROUP_TABLE_COLUMNS),
        "compact" | "full" | "tree" => Some(CAPABILITY_ROW_TABLE_COLUMNS),
        _ => None,
    }
}

/// Handle ADrive API metadata.
pub async fn handle_api_command(global: &GlobalArgs, args: &ApiArgs) -> Result<i32, CliError> {
    let command = format!("ve-adrive api {} {}", args.group, args.action);
    if args.describe {
        let capability = find_capability("ve-adrive api")
            .map(compact_capability)
            .unwrap_or_else(
                || json!({"command": "ve-adrive api", "mode": "guarded_utility_passthrough"}),
            );
        let description = if global.uses_chinese_documentation() {
            // [Review Fix #GlobalZh5] Preserve the dynamic API identifiers and
            // localize only the fixed prose surrounding them.
            format!(
                "受保护的 ADrive 工具 API 规划：{}.{}；暂不支持直接执行原始 API",
                args.group, args.action
            )
        } else {
            format!(
                "Guarded ADrive utility API planning for {}.{}; direct raw execution is not implemented yet",
                args.group, args.action
            )
        };
        let mut desc = json!({
            "command": command,
            "description": description,
            "service": "ids",
            "capability": capability,
            "mode": "guarded_utility_passthrough",
            "layer": "meta",
            "raw_api_execution_implemented": false,
            "supports_dry_run": true,
            "supports_force": false,
        });
        if global.uses_chinese_documentation() {
            localize_adrive_auth_documentation_zh(&mut desc);
        }
        output_result(global, &Envelope::success(command, desc))?;
        return Ok(0);
    }

    let request = parse_optional_request(args.request.as_deref())?;
    if !global.dry_run {
        return Err(CliError::ValidationError(
            "ADrive raw API execution is not implemented yet; use --dry-run to inspect the planned request or --describe for metadata".to_string(),
        ));
    }
    let payload = json!({
        "group": &args.group,
        "action": &args.action,
        "request": request,
        "status": "planned_not_executed",
        "mode": "guarded_utility_passthrough",
        "raw_api_execution_implemented": false,
        "message": "ADrive raw API execution is not implemented; this utility returns dry-run metadata only",
    });
    let envelope = Envelope::success(
        format!("ve-adrive api {} {}", args.group, args.action),
        payload,
    );
    output_envelope(global, &envelope)?;
    Ok(0)
}

fn parse_optional_request(request: Option<&str>) -> Result<Value, CliError> {
    let Some(request) = request else {
        return Ok(Value::Null);
    };
    let candidate = request.strip_prefix("file://").unwrap_or(request);
    let payload = if Path::new(candidate).exists() {
        fs::read_to_string(candidate)?
    } else {
        request.to_string()
    };
    serde_json::from_str(&payload)
        .map_err(|err| CliError::ValidationError(format!("invalid --request JSON: {err}")))
}

pub async fn handle_skill_command(
    global: &GlobalArgs,
    cmd: &SkillCommand,
) -> Result<i32, CliError> {
    if global.describe {
        // [Review Fix #ADrive-SkillDescribe] Describe must stay read-only even
        // for `skill export`, otherwise Agents can accidentally create files
        // while only asking for metadata.
        let command_path = match &cmd.action {
            SkillAction::List { .. } => "ve-adrive skill list",
            SkillAction::Export { .. } => "ve-adrive skill export",
        };
        let description = describe_adrive_command_metadata(command_path).ok_or_else(|| {
            CliError::ValidationError(format!("no metadata registered for {command_path}"))
        })?;
        output_result(global, &Envelope::success(command_path, description))?;
        return Ok(0);
    }
    match &cmd.action {
        SkillAction::List { language } => {
            output_result(
                global,
                &Envelope::success(
                    "ve-adrive skill list",
                    SkillList {
                        language: language.code(),
                        skills: skill_definitions_for_language(*language),
                    },
                ),
            )?;
            Ok(0)
        }
        SkillAction::Export {
            name,
            dir,
            language,
        } => {
            let export_plan = skill_markdown_export_plan(name.as_deref(), dir)?;
            if global.dry_run {
                output_result(
                    global,
                    &Envelope::success(
                        "ve-adrive skill export",
                        plan_skill_markdown_export(&export_plan, dir, *language),
                    ),
                )?;
                return Ok(0);
            }
            let exported = export_markdown_skills(export_plan, dir, *language)?;
            output_result(
                global,
                &Envelope::success("ve-adrive skill export", exported),
            )?;
            Ok(0)
        }
    }
}

fn skill_definitions() -> Vec<SkillDefinition> {
    capabilities()
        .iter()
        .map(|row| {
            let internal_command = row.command.to_string();
            let public_command = public_adrive_command_path(row.command);
            let name = public_command.replace(' ', "_").replace('-', "_");
            SkillDefinition {
                schema_version: "adrive-skill-v1",
                name: name.clone(),
                domain: business_domain(row.command).to_string(),
                command: public_command,
                internal_command,
                description: row.description.to_string(),
                risk_level: row.risk_level.to_string(),
                input_schema: skill_input_schema(row),
                examples: row
                    .examples
                    .iter()
                    .map(|example| public_adrive_example(example))
                    .collect(),
                usage: skill_usage(name),
            }
        })
        .collect()
}

fn skill_definitions_for_language(language: DocumentationLanguage) -> Vec<SkillDefinition> {
    let mut definitions = skill_definitions();
    if matches!(language, DocumentationLanguage::Zh) {
        for definition in &mut definitions {
            definition.description = localized_skill_description_zh(definition);
            definition.input_schema = localized_input_schema(&definition.input_schema, language);
            // [Review Fix #ADriveZh6] Skill usage is human-facing metadata too;
            // localizing only descriptions and schemas leaves Chinese output mixed.
            definition.usage = localized_skill_usage_zh(&definition.usage);
        }
    }
    definitions
}

fn localized_skill_usage_zh(usage: &SkillUsage) -> SkillUsage {
    SkillUsage {
        format: usage.format,
        source: adrive_metadata_translation_zh(usage.source)
            .expect("owner audit guarantees Skill usage source"),
        mcp_tool_name: usage.mcp_tool_name.clone(),
        mcp_server: usage.mcp_server.clone(),
        serve_reads_exported_files: usage.serve_reads_exported_files,
        exported_file_use: adrive_metadata_translation_zh(usage.exported_file_use)
            .expect("owner audit guarantees exported-file usage"),
        default_mcp_call: adrive_metadata_translation_zh(usage.default_mcp_call)
            .expect("owner audit guarantees MCP call usage"),
    }
}

fn localized_skill_description_zh(skill: &SkillDefinition) -> String {
    adrive_metadata_translation_zh(&skill.description)
        .expect("owner audit guarantees every ADrive skill description")
        .to_string()
}

fn localized_input_schema(schema: &Value, language: DocumentationLanguage) -> Value {
    match language {
        DocumentationLanguage::En => schema.clone(),
        DocumentationLanguage::Zh => localize_schema_descriptions_zh(schema),
    }
}

fn localize_schema_descriptions_zh(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut localized = serde_json::Map::new();
            for (key, child) in map {
                if key == "description" {
                    if let Some(description) = child.as_str() {
                        localized.insert(
                            key.clone(),
                            Value::String(
                                adrive_metadata_translation_zh(description)
                                    .expect("owner audit guarantees every schema description")
                                    .to_string(),
                            ),
                        );
                        continue;
                    }
                }
                localized.insert(key.clone(), localize_schema_descriptions_zh(child));
            }
            Value::Object(localized)
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(localize_schema_descriptions_zh)
                .collect::<Vec<_>>(),
        ),
        _ => value.clone(),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ADriveMetadataContext {
    Structured,
    HumanText,
    HumanContainer,
    ScenarioRouting,
    Usage,
    Machine,
}

/// Localize frozen ADrive human prose inside describe metadata.
///
/// Machine-readable keys, identifiers, enum literals, examples, commands,
/// and error codes are preserved even when their value matches prose in the
/// owner catalog.
pub fn localize_adrive_auth_documentation_zh(value: &mut Value) {
    localize_adrive_metadata_value_zh(value, ADriveMetadataContext::Structured);
}

fn localize_adrive_metadata_value_zh(value: &mut Value, context: ADriveMetadataContext) {
    match value {
        Value::String(text) if context == ADriveMetadataContext::HumanText => {
            if let Some(chinese) = adrive_metadata_translation_zh(text) {
                *text = chinese.to_string();
            }
        }
        Value::Array(items) => {
            for item in items {
                let item_context = adrive_array_item_context(context, item);
                localize_adrive_metadata_value_zh(item, item_context);
            }
        }
        Value::Object(map) => {
            for (key, child) in map {
                let child_context = adrive_child_metadata_context(context, key, child);
                localize_adrive_metadata_value_zh(child, child_context);
            }
        }
        _ => {}
    }
}

fn adrive_array_item_context(parent: ADriveMetadataContext, item: &Value) -> ADriveMetadataContext {
    match (parent, item) {
        (ADriveMetadataContext::Machine, _) => ADriveMetadataContext::Machine,
        (ADriveMetadataContext::HumanContainer, Value::String(_))
        | (ADriveMetadataContext::ScenarioRouting, Value::String(_)) => {
            ADriveMetadataContext::HumanText
        }
        (ADriveMetadataContext::HumanContainer, _) => ADriveMetadataContext::HumanContainer,
        (ADriveMetadataContext::ScenarioRouting, _) => ADriveMetadataContext::ScenarioRouting,
        _ => ADriveMetadataContext::Structured,
    }
}

fn adrive_child_metadata_context(
    parent: ADriveMetadataContext,
    key: &str,
    value: &Value,
) -> ADriveMetadataContext {
    // [Review Fix #ADriveZh8] Production and owner audits share this key-aware
    // policy so catalog collisions can never rewrite machine metadata.
    if parent == ADriveMetadataContext::Machine {
        return ADriveMetadataContext::Machine;
    }
    if key == "description" && value.is_string() {
        return ADriveMetadataContext::HumanText;
    }
    if is_adrive_machine_metadata_key(key) {
        return ADriveMetadataContext::Machine;
    }
    match parent {
        ADriveMetadataContext::ScenarioRouting => scenario_routing_context(key, value),
        ADriveMetadataContext::HumanContainer => human_container_context(key, value),
        ADriveMetadataContext::Usage => usage_metadata_context(key, value),
        _ => structured_metadata_context(key, value),
    }
}

fn scenario_routing_context(key: &str, value: &Value) -> ADriveMetadataContext {
    if key == "mode_resolution" {
        ADriveMetadataContext::Machine
    } else if value.is_string() {
        ADriveMetadataContext::HumanText
    } else {
        ADriveMetadataContext::ScenarioRouting
    }
}

fn human_container_context(key: &str, value: &Value) -> ADriveMetadataContext {
    if is_adrive_machine_metadata_key(key) {
        ADriveMetadataContext::Machine
    } else if value.is_string() {
        ADriveMetadataContext::HumanText
    } else {
        ADriveMetadataContext::HumanContainer
    }
}

fn usage_metadata_context(key: &str, value: &Value) -> ADriveMetadataContext {
    if matches!(key, "source" | "exported_file_use" | "default_mcp_call") && value.is_string() {
        ADriveMetadataContext::HumanText
    } else {
        ADriveMetadataContext::Machine
    }
}

fn structured_metadata_context(key: &str, value: &Value) -> ADriveMetadataContext {
    match key {
        "scenario_routing" => ADriveMetadataContext::ScenarioRouting,
        "shell_quoting_tips" | "notes" | "guidance" | "recovery" => {
            if value.is_string() {
                ADriveMetadataContext::HumanText
            } else {
                ADriveMetadataContext::HumanContainer
            }
        }
        "usage" => ADriveMetadataContext::Usage,
        _ => ADriveMetadataContext::Structured,
    }
}

fn is_adrive_machine_metadata_key(key: &str) -> bool {
    matches!(
        key,
        "api"
            | "code"
            | "command"
            | "enum"
            | "examples"
            | "id"
            | "method"
            | "name"
            | "path"
            | "type"
            | "uri"
    )
}

fn adrive_metadata_translation_zh(text: &str) -> Option<&'static str> {
    ADRIVE_METADATA_TRANSLATIONS_ZH
        .iter()
        .find_map(|(english, chinese)| (*english == text).then_some(*chinese))
}

fn public_capability_row(row: &CapabilityRow) -> Value {
    let mut value = serde_json::to_value(row).unwrap_or_else(|_| json!({}));
    value["examples"] = json!(row
        .examples
        .iter()
        .map(|example| public_adrive_example(example))
        .collect::<Vec<_>>());
    value
}

fn public_adrive_command(command: &str) -> String {
    let prefix = adrive_example_prefix();
    command
        .strip_prefix("ve-adrive ")
        .or_else(|| command.strip_prefix("ve-adrive-cli "))
        .or_else(|| command.strip_prefix("ve-storage-uni-cli ve-adrive "))
        .map(|suffix| format!("{prefix} {suffix}"))
        .unwrap_or_else(|| command.to_string())
}

fn public_adrive_example(example: &str) -> String {
    let prefix = adrive_example_prefix();
    let with_public_pipeline = example
        .replace(" | ve-adrive ", &format!(" | {prefix} "))
        .replace("ve-adrive-cli ", &format!("{prefix} "))
        .replace("ve-storage-uni-cli ve-adrive ", &format!("{prefix} "));
    public_adrive_command(&with_public_pipeline)
}

fn adrive_example_prefix() -> String {
    std::env::var(ADRIVE_EXAMPLE_PREFIX_ENV).unwrap_or_else(|_| "ve-adrive-cli".to_string())
}

fn skill_usage(name: String) -> SkillUsage {
    SkillUsage {
        format: "Markdown SKILL.md",
        source: "Derived from the live ADrive CLI capability registry.",
        mcp_tool_name: name,
        mcp_server: public_adrive_command("ve-adrive serve --mcp"),
        serve_reads_exported_files: false,
        exported_file_use: "Portable Markdown skill pack for external agents, documentation generators, prompts, or adapters. The built-in MCP server rebuilds tools from the in-process registry instead of reading exported files.",
        default_mcp_call: "tools/call returns a plan by default; include argument execute=true to run the underlying CLI command.",
    }
}

fn skill_input_schema(row: &CapabilityRow) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for parameter in row.parameters {
        properties.insert(
            parameter.name.to_string(),
            json!({
                "type": parameter_schema_type(parameter.name),
                "description": parameter.description,
            }),
        );
        if parameter.required {
            required.push(parameter.name);
        }
    }
    for (name, schema) in mcp_common_schema_properties() {
        properties.entry(name.to_string()).or_insert(schema);
    }
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn parameter_schema_type(name: &str) -> &'static str {
    if is_boolean_parameter(name) {
        "boolean"
    } else if matches!(
        name,
        "port"
            | "max-keys"
            | "max-depth"
            | "top-k"
            | "batch-concurrency"
            | "list-concurrency"
            | "multipart-concurrency"
    ) {
        "integer"
    } else {
        "string"
    }
}

fn mcp_common_schema_properties() -> [(&'static str, Value); 9] {
    [
        (
            "execute",
            json!({"type": "boolean", "description": "Execute the CLI command; false returns a plan only"}),
        ),
        (
            "dry_run",
            json!({"type": "boolean", "description": "Pass global --dry-run to the CLI command"}),
        ),
        (
            "describe",
            json!({"type": "boolean", "description": "Pass global --describe to the CLI command"}),
        ),
        (
            "profile",
            json!({"type": "string", "description": "Configuration profile name"}),
        ),
        (
            "output",
            json!({"type": "string", "description": "Output format, defaults to json"}),
        ),
        (
            "region",
            json!({"type": "string", "description": "Optional global region override"}),
        ),
        (
            "endpoint",
            json!({"type": "string", "description": "Optional global endpoint override"}),
        ),
        (
            "verbose",
            json!({"type": "boolean", "description": "Include extra diagnostic output where supported"}),
        ),
        (
            "quiet",
            json!({"type": "boolean", "description": "Disable prompts and progress output"}),
        ),
    ]
}

fn is_boolean_parameter(name: &str) -> bool {
    matches!(
        name,
        "force"
            | "by-name"
            | "recursive"
            | "include-parent"
            | "include-uploads"
            | "index-enabled"
            | "parents"
            | "no-clobber"
            | "no-manifest"
            | "report-failures-only"
            | "progress"
            | "no-progress"
            | "list-echo"
            | "no-list-echo"
            | "delete"
            | "size-only"
            | "exact-timestamps"
            | "human-readable"
            | "cost"
            | "mcp"
            | "dry_run"
            | "describe"
            | "execute"
            | "verbose"
            | "quiet"
    )
}

fn selected_skills(name: Option<&str>) -> Result<Vec<SkillDefinition>, CliError> {
    let skills = skill_definitions();
    let selected = skills
        .into_iter()
        .filter(|skill| {
            name.map(|name| {
                skill.name == name
                    || skill.domain == name
                    || skill.command == name
                    || skill.command == format!("ve-adrive {name}")
                    || skill.internal_command == name
                    || skill.internal_command == format!("ve-adrive {name}")
                    || legacy_skill_name(&skill.internal_command) == name
            })
            .unwrap_or(true)
        })
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Err(CliError::ValidationError(format!(
            "no ve-adrive skill matches '{}'",
            name.unwrap_or_default()
        )));
    }
    Ok(selected)
}

fn legacy_skill_name(command: &str) -> String {
    command.replace(' ', "_").replace('-', "_")
}

fn skill_markdown_export_plan(
    name: Option<&str>,
    dir: &str,
) -> Result<Vec<(SkillDefinition, PathBuf)>, CliError> {
    Ok(selected_skills(name)?
        .into_iter()
        .map(|skill| {
            let path = Path::new(dir)
                .join(&skill.domain)
                .join(&skill.name)
                .join("SKILL.md");
            (skill, path)
        })
        .collect())
}

fn plan_skill_markdown_export(
    export_plan: &[(SkillDefinition, PathBuf)],
    dir: &str,
    language: DocumentationLanguage,
) -> Value {
    // [Review Fix #SkillExportAlign] Expose the same path fields as tos-cli
    // and ve-tos so dry-run consumers do not need per-command branching.
    let entries = export_plan
        .iter()
        .map(|(skill, path)| {
            json!({
                "skill": skill.name,
                "domain": skill.domain,
                "command": skill.command,
                "path": path.display().to_string(),
                "conflict": path.exists(),
            })
        })
        .collect::<Vec<_>>();
    json!({
        "dry_run": true,
        "format": "markdown_skill",
        "language": language.code(),
        "dir": dir,
        "selected": export_plan.len(),
        "root_file": skill_root_path(Path::new(dir)).display().to_string(),
        "paths": export_paths(export_plan, Path::new(dir))
            .into_iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>(),
        "skill_paths": export_plan
            .iter()
            .map(|(_, path)| path.display().to_string())
            .collect::<Vec<_>>(),
        "status": "planned_not_written",
        "skill_count": export_plan.len(),
        "entries": entries,
    })
}

fn export_markdown_skills(
    export_plan: Vec<(SkillDefinition, PathBuf)>,
    dir: &str,
    language: DocumentationLanguage,
) -> Result<Value, CliError> {
    for path in export_paths(&export_plan, Path::new(dir)) {
        if path.exists() {
            return Err(CliError::Conflict(format!(
                "skill export target '{}' already exists",
                path.display()
            )));
        }
    }

    // [Review Fix #Skill6] Build parser metadata once rather than once per file.
    let root = tos_core::agent::skill_markdown::command_tree::<crate::cli::ADriveCommand>();
    let mut files = Vec::new();
    let skills = export_plan
        .iter()
        .map(|(skill, _)| skill.clone())
        .collect::<Vec<_>>();
    let root_path = skill_root_path(Path::new(dir));
    if let Some(parent) = root_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        &root_path,
        skill_index_markdown("ve-adrive", &skills, language),
    )?;
    files.push(root_path.display().to_string());
    for (skill, path) in export_plan {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, skill_markdown(&skill, language, &root))?;
        files.push(path.display().to_string());
    }

    Ok(json!({
        "dry_run": false,
        "format": "markdown_skill",
        "language": language.code(),
        "dir": dir,
        "selected": files.len().saturating_sub(1),
        "root_file": files.first().cloned(),
        "files": files,
    }))
}

fn skill_root_path(dir: &Path) -> PathBuf {
    dir.join("SKILL.md")
}

fn export_paths(export_plan: &[(SkillDefinition, PathBuf)], dir: &Path) -> Vec<PathBuf> {
    let mut paths = vec![skill_root_path(dir)];
    paths.extend(export_plan.iter().map(|(_, path)| path.clone()));
    paths
}

fn skill_index_markdown(
    surface: &str,
    skills: &[SkillDefinition],
    language: DocumentationLanguage,
) -> String {
    let mut domains = std::collections::BTreeMap::<&str, Vec<&SkillDefinition>>::new();
    for skill in skills {
        domains.entry(&skill.domain).or_default().push(skill);
    }
    let public_surface = public_adrive_example(&format!("{surface} "))
        .trim()
        .to_string();
    let mut body = match language {
        DocumentationLanguage::En => format!(
            "# {surface} skills\n\nUse this skill pack when the user wants to operate `{public_surface}` commands. Select a domain below, then use the nested command skill.\n\n"
        ),
        DocumentationLanguage::Zh => format!(
            "# {surface} Skills\n\n当用户需要操作 `{public_surface}` 命令时使用此 Skill 包。先按领域选择，再进入对应的命令 Skill。\n\n"
        ),
    };
    for (domain, skills) in domains {
        body.push_str(&format!("## {domain}\n\n"));
        for skill in skills {
            let description = match language {
                DocumentationLanguage::En => skill.description.clone(),
                DocumentationLanguage::Zh => localized_skill_description_zh(skill),
            };
            body.push_str(&format!(
                "- [{}](./{}/{}/SKILL.md): `{}` - {}\n",
                skill.name,
                skill.domain,
                skill.name,
                public_adrive_example(&skill.command),
                description
            ));
        }
        body.push('\n');
    }
    let description = match language {
        DocumentationLanguage::En => format!(
            "Use when operating {public_surface}; select a command reference from this index."
        ),
        DocumentationLanguage::Zh => {
            format!("当用户需要使用 {public_surface} 时，先从此索引选择对应命令。")
        }
    };
    tos_core::agent::skill_markdown::frontmatter(&format!("{surface}-commands"), &description)
        + &body
}

fn skill_markdown(
    skill: &SkillDefinition,
    language: DocumentationLanguage,
    root: &clap::Command,
) -> String {
    use tos_core::agent::skill_markdown::{render, CommandSkill};

    let is_chinese = matches!(language, DocumentationLanguage::Zh);
    let description = if is_chinese {
        localized_skill_description_zh(skill)
    } else {
        skill.description.clone()
    };
    // [Review Fix #Skill2] Resolve every displayed command at export time;
    // canonical registry IDs and MCP names remain stable across entrypoints.
    let public_command = public_adrive_example(&skill.command);
    let examples = skill
        .examples
        .iter()
        .map(|example| public_adrive_example(example))
        .collect::<Vec<_>>();
    let schema = localized_input_schema(&skill.input_schema, language);
    render(
        &CommandSkill {
            name: &skill.name,
            command: &skill.command,
            public_command: &public_command,
            description: &description,
            risk: &skill.risk_level,
            schema: &schema,
            examples: &examples,
            is_chinese,
        },
        root,
    )
}

/// Handle ADrive config.
pub async fn handle_config_command(
    global: &GlobalArgs,
    cmd: &ConfigCommand,
) -> Result<i32, CliError> {
    if global.describe {
        // [Review Fix #ADrive-ConfigDescribe] Capabilities advertise describe
        // support for this utility group, so group-level describe must not
        // require a leaf subcommand.
        output_result(
            global,
            &Envelope::success("ve-adrive config", describe_config_group()),
        )?;
        return Ok(0);
    }

    let Some(action) = &cmd.action else {
        return Err(CliError::ValidationError(
            "`ve-adrive config` requires a subcommand; use `ve-adrive config --help`".to_string(),
        ));
    };

    if global.dry_run {
        // [Review Fix #ADrive-ConfigDryRun] Global --dry-run must stay side-effect
        // free for config init/set, matching the TOS config handler contract.
        return handle_config_dry_run(global, action);
    }

    match action {
        ConfigAction::Init { profile } => {
            let profile_name = effective_config_init_profile(global, profile.as_deref())?;
            let config_path = global.config_path();
            // [Review Fix #2] Preserve unreadable or malformed existing config
            // files instead of replacing them with a fresh default profile.
            let mut config = ConfigFile::load_from(&config_path)?;
            let adrive_override = config
                .get_or_insert_profile(profile_name)
                .adrive
                .get_or_insert_with(AdriveOverride::default);
            if adrive_override.checkpoint_dir.is_none() {
                adrive_override.checkpoint_dir = Some(ADRIVE_DEFAULT_CHECKPOINT_DIR.to_string());
            }
            if adrive_override.batch_report_dir.is_none() {
                adrive_override.batch_report_dir =
                    Some(ADRIVE_DEFAULT_BATCH_REPORT_DIR.to_string());
            }
            if adrive_override.batch_report_format.is_none() {
                adrive_override.batch_report_format =
                    Some(DEFAULT_TOS_BATCH_REPORT_FORMAT.to_string());
            }
            if adrive_override.progress_enabled.is_none() {
                adrive_override.progress_enabled = Some(DEFAULT_TOS_PROGRESS_ENABLED);
            }
            if adrive_override.max_retry_count.is_none() {
                adrive_override.max_retry_count = Some(DEFAULT_HTTP_MAX_RETRY_COUNT);
            }
            if adrive_override.requesttimeout.is_none() {
                adrive_override.requesttimeout = Some(DEFAULT_HTTP_REQUEST_TIMEOUT_SECONDS);
            }
            if adrive_override.connecttimeout.is_none() {
                adrive_override.connecttimeout = Some(DEFAULT_HTTP_CONNECT_TIMEOUT_SECONDS);
            }
            if adrive_override.maxconnections.is_none() {
                adrive_override.maxconnections = Some(DEFAULT_HTTP_MAX_CONNECTIONS);
            }
            config.save_to_path(&config_path)?;
            let envelope = Envelope::success(
                "ve-adrive config init",
                json!({
                    "profile": profile_name,
                    "status": "initialized",
                    "config_path": config_path.display().to_string(),
                    "message": format!("Profile '{}' initialized with A-Drive defaults", profile_name),
                }),
            );
            output_envelope(global, &envelope)?;
            Ok(0)
        }
        ConfigAction::Show => {
            let config_path = global.config_path();
            let config_dir = ConfigFile::config_dir_from_path(&config_path);
            let config = ConfigFile::load_from(&config_path)?;
            // [Review Fix #9] Keep explicit credentials-path behavior aligned
            // across runtime, diagnostics, and config inspection.
            let credentials_path = global.existing_runtime_credentials_path()?;
            let credentials = CredentialsFile::load_from(&credentials_path)?;
            let mut display_config = config.clone();
            for profile_name in credentials.profile_names() {
                display_config.get_or_insert_profile(&profile_name);
            }

            // 与 tos config show 对齐：列出所有 profiles
            let binary = Binary::Adrive;
            let mut effective: Vec<EffectiveProfile> = Vec::new();
            for profile_name in display_config.profiles.keys() {
                let mut eff = display_config.get_effective_profile_in_dir(
                    profile_name,
                    binary,
                    &config_dir,
                )?;
                let stored = credentials.effective_aksk(
                    profile_name,
                    CredentialSection::ADrive,
                    &credentials_path,
                )?;
                let oauth = credentials.adrive_oauth(profile_name, &credentials_path)?;
                if !config.profiles.contains_key(profile_name)
                    && stored.is_empty()
                    && oauth.is_empty()
                {
                    continue;
                }
                overlay_effective_credentials(&mut eff, &stored);
                overlay_effective_oauth_credentials(&mut eff, &oauth);
                effective.push(redact_adrive_effective(eff));
            }

            let format = global.output.unwrap_or_else(OutputFormat::auto_detect);
            match format {
                OutputFormat::Table => {
                    println!(
                        "Config file: {}\nCredentials file: {}\n",
                        config_path.display(),
                        credentials_path.display()
                    );
                    let headers = &["PROFILE", "FIELD", "VALUE", "SOURCE"];
                    let mut rows: Vec<Vec<String>> = Vec::new();
                    for eff in &effective {
                        if let Some(ref f) = eff.auth_mode {
                            push_traced_row(&mut rows, eff, "auth_mode", f);
                        }
                        if let Some(ref f) = eff.auth_endpoint {
                            push_traced_row(&mut rows, eff, "auth_endpoint", f);
                        }
                        push_traced_row(&mut rows, eff, "region", &eff.region);
                        push_traced_row(&mut rows, eff, "endpoint", &eff.endpoint);
                        push_traced_row(&mut rows, eff, "checkpoint_dir", &eff.checkpoint_dir);
                        push_traced_row(&mut rows, eff, "batch_report_dir", &eff.batch_report_dir);
                        push_traced_row(
                            &mut rows,
                            eff,
                            "batch_report_format",
                            &eff.batch_report_format,
                        );
                        push_traced_bool_row(
                            &mut rows,
                            eff,
                            "progress_enabled",
                            &eff.progress_enabled,
                        );
                        push_traced_value_row(
                            &mut rows,
                            eff,
                            "max_retry_count",
                            &eff.max_retry_count,
                        );
                        push_traced_value_row(
                            &mut rows,
                            eff,
                            "requesttimeout",
                            &eff.requesttimeout,
                        );
                        push_traced_value_row(
                            &mut rows,
                            eff,
                            "connecttimeout",
                            &eff.connecttimeout,
                        );
                        push_traced_value_row(
                            &mut rows,
                            eff,
                            "maxconnections",
                            &eff.maxconnections,
                        );
                        push_traced_row(&mut rows, eff, "access_key_id", &eff.access_key_id);
                        push_traced_row(
                            &mut rows,
                            eff,
                            "secret_access_key",
                            &eff.secret_access_key,
                        );
                        push_traced_row(&mut rows, eff, "security_token", &eff.security_token);
                        if let Some(ref f) = eff.access_token {
                            push_traced_row(&mut rows, eff, "access_token", f);
                        }
                        if let Some(ref f) = eff.refresh_token {
                            push_traced_row(&mut rows, eff, "refresh_token", f);
                        }
                        if let Some(ref f) = eff.account_id {
                            push_traced_row(&mut rows, eff, "account_id", f);
                        }
                        if let Some(ref f) = eff.default_instance {
                            push_traced_row(&mut rows, eff, "default_instance", f);
                        }
                        if let Some(ref f) = eff.default_space {
                            push_traced_row(&mut rows, eff, "default_space", f);
                        }
                    }
                    use tos_core::agent::output::format_table;
                    println!("{}", format_table(headers, &rows));
                }
                _ => {
                    let envelope = Envelope::success(
                        "ve-adrive config show",
                        json!({
                            "config_path": config_path.display().to_string(),
                            "credentials_path": credentials_path.display().to_string(),
                            "profiles": effective,
                        }),
                    );
                    output_envelope(global, &envelope)?;
                }
            }
            Ok(0)
        }
        ConfigAction::Set { key, value } => {
            if is_sensitive_adrive_config_key(key) {
                return handle_adrive_credential_set(global, key, value);
            }
            let config_path = global.config_path();
            let mut config = ConfigFile::load_from(&config_path)?;
            // [Review Fix #ADrive-ConfigDryRun] Real execution and dry-run use the
            // same key routing so previewed ADrive writes cannot drift from saved writes.
            let segments = adrive_config_key_segments(global, key)?;
            let segment_refs = segments.iter().map(String::as_str).collect::<Vec<_>>();
            config.set_by_path(&segment_refs, value)?;
            config.save_to_path(&config_path)?;
            let envelope = Envelope::success(
                "ve-adrive config set",
                adrive_config_set_output(key, value, &segments, &config_path),
            );
            output_envelope(global, &envelope)?;
            Ok(0)
        }
    }
}

fn handle_adrive_credential_set(
    global: &GlobalArgs,
    key: &str,
    value: &str,
) -> Result<i32, CliError> {
    let segments = adrive_config_key_segments(global, key)?;
    let profile_name = segments
        .first()
        .ok_or_else(|| CliError::ValidationError("missing credential profile".to_string()))?;
    let field = segments
        .last()
        .ok_or_else(|| CliError::ValidationError("missing credential field".to_string()))?;
    // [Review Fix #10] Preserve explicit legacy key routing such as
    // `default.tos.access_key_id`; only bare ADrive keys default to ADrive.
    let section = credential_section_for_adrive_path(&segments)?;
    let credentials_path = global.credentials_path();
    let mut credentials = CredentialsFile::load_from(&credentials_path)?;
    credentials.set_aksk_field(profile_name, section, field, value)?;
    credentials.save_to_path(&credentials_path)?;

    let envelope = Envelope::success(
        "ve-adrive config set",
        json!({
            "key": key,
            "section": adrive_credential_section_path(profile_name, section),
            "field": field,
            "value": "****",
            "encrypted": true,
            "status": "saved",
            "config_path": global.config_path().display().to_string(),
            "credentials_path": credentials_path.display().to_string(),
            "message": format!("Saved {field} to credentials"),
        }),
    );
    output_envelope(global, &envelope)?;
    Ok(0)
}

fn credential_section_for_adrive_path(segments: &[String]) -> Result<CredentialSection, CliError> {
    match segments {
        [_profile, _field] => Ok(CredentialSection::Shared),
        [_profile, binary, _field] => match Binary::parse(binary) {
            Some(Binary::Tos) => Ok(CredentialSection::Tos),
            Some(Binary::VeTos) => Ok(CredentialSection::VeTos),
            Some(Binary::Adrive) => Ok(CredentialSection::ADrive),
            _ => Err(CliError::ValidationError(format!(
                "unsupported credential namespace '{binary}'"
            ))),
        },
        _ => Err(CliError::ValidationError(
            "invalid credential key path".to_string(),
        )),
    }
}

fn adrive_credential_section_path(profile_name: &str, section: CredentialSection) -> String {
    match section {
        CredentialSection::Shared => format!("[{profile_name}]"),
        CredentialSection::Tos => format!("[{profile_name}.tos]"),
        CredentialSection::VeTos => format!("[{profile_name}.ve-tos]"),
        CredentialSection::ADrive => format!("[{profile_name}.adrive]"),
    }
}

fn adrive_config_set_output(
    key: &str,
    value: &str,
    segments: &[String],
    config_path: &std::path::Path,
) -> Value {
    // [Review Fix #6] Only redact sensitive keys; non-sensitive routing values
    // such as region/endpoint must stay visible for troubleshooting.
    let section = match segments {
        [profile, _field] => format!("[{profile}]"),
        [profile, service, _field] => format!("[{profile}.{service}]"),
        _ => key.to_string(),
    };
    let field = segments.last().cloned().unwrap_or_else(|| key.to_string());
    let encrypted = is_sensitive_adrive_config_key(key);
    json!({
        "key": key,
        "section": section,
        "field": field,
        "value": redact_adrive_config_value(key, value),
        "encrypted": encrypted,
        "status": "saved",
        "config_path": config_path.display().to_string(),
        "message": format!("Saved {} to config", key),
    })
}

fn handle_config_dry_run(global: &GlobalArgs, action: &ConfigAction) -> Result<i32, CliError> {
    let dry_run = match action {
        ConfigAction::Init { profile } => {
            let profile_name = effective_config_init_profile(global, profile.as_deref())?;
            let path = global.config_path();
            DryRunResult {
                action: "config init".to_string(),
                dry_run: true,
                impact: Impact {
                    affected_objects: 0,
                    affected_bytes: 0,
                    risk_level: "low".to_string(),
                    estimated_duration: Some("< 1s".to_string()),
                    scanned_count: None,
                    preview_truncated: None,
                },
                plan: vec![
                    format!(
                        "CREATE or UPDATE template config file at '{}'",
                        path.display()
                    ),
                    format!("ENSURE [{}.adrive] section exists", profile_name),
                    format!(
                        "WRITE [{}.adrive].checkpoint_dir default if missing",
                        profile_name
                    ),
                    format!(
                        "WRITE [{}.adrive].batch_report_dir default if missing",
                        profile_name
                    ),
                    format!(
                        "WRITE [{}.adrive].batch_report_format default if missing",
                        profile_name
                    ),
                    format!(
                        "WRITE [{}.adrive].progress_enabled default if missing",
                        profile_name
                    ),
                    format!(
                        "WRITE [{}.adrive].max_retry_count default if missing",
                        profile_name
                    ),
                    format!(
                        "WRITE [{}.adrive].requesttimeout default if missing",
                        profile_name
                    ),
                    format!(
                        "WRITE [{}.adrive].connecttimeout default if missing",
                        profile_name
                    ),
                    format!(
                        "WRITE [{}.adrive].maxconnections default if missing",
                        profile_name
                    ),
                ],
                warnings: if path.exists() {
                    vec![format!(
                        "Config file already exists at '{}'; existing profiles are preserved",
                        path.display()
                    )]
                } else {
                    vec![]
                },
                confirm_command: Some(format!(
                    "ve-adrive-cli config init --profile {}",
                    profile_name
                )),
            }
        }
        ConfigAction::Set { key, value } => {
            let segments = adrive_config_key_segments(global, key)?;
            let segment_refs = segments.iter().map(String::as_str).collect::<Vec<_>>();
            let mut validation_config = ConfigFile::default();
            validation_config.set_by_path(&segment_refs, value)?;
            let redacted_value = redact_adrive_config_value(key, value);
            let plan_line = if is_sensitive_adrive_config_key(key) {
                let section = credential_section_for_adrive_path(&segments)?;
                format!(
                    "SET {}.{} in '{}' = '{}'",
                    adrive_credential_section_path(&segments[0], section),
                    segments.last().unwrap_or(key),
                    global.credentials_path().display(),
                    redacted_value
                )
            } else {
                match segments.len() {
                    2 => format!(
                        "SET [{}].{} = '{}'",
                        segments[0], segments[1], redacted_value
                    ),
                    3 => format!(
                        "SET [{}.{}].{} = '{}'",
                        segments[0], segments[1], segments[2], redacted_value
                    ),
                    _ => format!("SET {} = '{}'", key, redacted_value),
                }
            };
            DryRunResult {
                action: "config set".to_string(),
                dry_run: true,
                impact: Impact {
                    affected_objects: 0,
                    affected_bytes: 0,
                    risk_level: "low".to_string(),
                    estimated_duration: Some("< 1s".to_string()),
                    scanned_count: None,
                    preview_truncated: None,
                },
                plan: vec![plan_line],
                warnings: if is_sensitive_adrive_config_key(key) {
                    vec!["Sensitive value redacted in dry-run output".to_string()]
                } else {
                    vec![]
                },
                confirm_command: Some(format!(
                    "ve-adrive-cli --profile {} config set {} {}",
                    global.profile,
                    key,
                    redact_adrive_config_value(key, value)
                )),
            }
        }
        ConfigAction::Show => DryRunResult {
            action: "config show".to_string(),
            dry_run: true,
            impact: Impact {
                affected_objects: 0,
                affected_bytes: 0,
                risk_level: "low".to_string(),
                estimated_duration: Some("< 1s".to_string()),
                scanned_count: None,
                preview_truncated: None,
            },
            plan: vec!["READ config file and render redacted effective profiles".to_string()],
            warnings: vec![],
            confirm_command: None,
        },
    };
    output_envelope(
        global,
        &Envelope::success(config_action_command(action), dry_run),
    )?;
    Ok(0)
}

fn adrive_config_key_segments(global: &GlobalArgs, key: &str) -> Result<Vec<String>, CliError> {
    if global.profile.is_empty() {
        return Err(CliError::ValidationError(
            "Invalid profile name: profile must not be empty".to_string(),
        ));
    }
    if key.contains('.') {
        return Ok(key.split('.').map(ToString::to_string).collect());
    }
    Ok(vec![
        global.profile.clone(),
        "adrive".to_string(),
        key.to_string(),
    ])
}

fn is_sensitive_adrive_config_key(key: &str) -> bool {
    let leaf = key.rsplit('.').next().unwrap_or(key);
    matches!(
        leaf,
        "access_key_id" | "secret_access_key" | "security_token"
    )
}

fn redact_adrive_config_value(key: &str, value: &str) -> String {
    if is_sensitive_adrive_config_key(key) {
        "****".to_string()
    } else {
        value.to_string()
    }
}

fn config_action_command(action: &ConfigAction) -> &'static str {
    match action {
        ConfigAction::Init { .. } => "ve-adrive config init",
        ConfigAction::Show => "ve-adrive config show",
        ConfigAction::Set { .. } => "ve-adrive config set",
    }
}

fn effective_config_init_profile<'a>(
    global: &'a GlobalArgs,
    profile: Option<&'a str>,
) -> Result<&'a str, CliError> {
    let profile_name = profile.unwrap_or(global.profile.as_str());
    if profile_name.is_empty() {
        // [Review Fix #21] ADrive config init must honor global --profile and reject empty names.
        return Err(CliError::ValidationError(
            "Invalid profile name: profile must not be empty".to_string(),
        ));
    }
    Ok(profile_name)
}

fn describe_config_group() -> Value {
    json!({
        "command": "ve-adrive config",
        "description": "Configuration management",
        "kind": "command_group",
        "layer": "meta",
        "subcommands": [
            {
                "name": "init",
                "description": "Initialize configuration",
                "risk_level": "low",
            },
            {
                "name": "show",
                "description": "Show effective configuration",
                "risk_level": "low",
            },
            {
                "name": "set",
                "description": "Set configuration value",
                "risk_level": "low",
            },
        ],
        "supports_describe": true,
        "supports_help": true,
    })
}

/// Build registry-backed describe metadata for ADrive meta commands.
///
/// Returns `None` when `command_path` is not an ADrive meta command handled by
/// this module.
pub fn describe_adrive_command_metadata(command_path: &str) -> Option<Value> {
    // [Review Fix #ADriveZh7] Config actions bypass capability rows, so expose
    // their owned descriptions through the same Describe localization path.
    if let Some(description) = describe_config_action(command_path) {
        return Some(description);
    }
    let registry_command = match command_path {
        "ve-adrive skill list" | "ve-adrive skill export" => "ve-adrive skill",
        other => other,
    };
    let row = find_capability(registry_command)?;
    let parameters = describe_meta_parameters(command_path, row);
    Some(json!({
        "command": command_path,
        "layer": "meta",
        "description": describe_meta_command_description(command_path, row.description),
        "risk_level": row.risk_level,
        "supports_dry_run": row.supports_dry_run,
        "supports_pipe": false,
        "parameters": parameters,
        "scenario_routing": describe_meta_scenario_routing(command_path),
        "related_commands": {
            "low_level": row.api_actions,
        },
        "low_level_apis": row.api_actions,
        "wraps_apis": row.api_actions,
        "examples": describe_meta_examples(command_path),
        "output_filter_examples": [
            format!("{} --output json | jq '.data'", public_adrive_command(command_path)),
            format!("{} --output json --query 'data'", public_adrive_command(command_path)),
        ],
        "shell_quoting_tips": [
            "Quote paths and JSON/JMESPath expressions that contain shell metacharacters.",
            "The command returns an Envelope; extract payload fields from data.*."
        ],
    }))
}

fn describe_config_action(command_path: &str) -> Option<Value> {
    let description = match command_path {
        "ve-adrive config init" => "Initialize configuration",
        "ve-adrive config show" => "Show effective configuration",
        "ve-adrive config set" => "Set configuration value",
        _ => return None,
    };
    Some(json!({
        "command": command_path,
        "description": description,
        "kind": "command",
        "layer": "meta",
        "supports_describe": true,
        "supports_help": true,
    }))
}

fn describe_meta_command_description(command_path: &str, fallback: &str) -> String {
    match command_path {
        "ve-adrive completion" => {
            "Generate shell completion scripts and installation snippets for ADrive CLI names."
        }
        "ve-adrive serve" => "Start the registry-backed ADrive MCP server.",
        "ve-adrive skill list" => "List ADrive skill metadata from the live registry.",
        "ve-adrive skill export" => "Export ADrive Markdown SKILL.md files for external consumers.",
        _ => fallback,
    }
    .to_string()
}

fn describe_meta_parameters(command_path: &str, row: &CapabilityRow) -> Vec<Value> {
    match command_path {
        "ve-adrive skill list" => vec![describe_meta_parameter(
            "language",
            false,
            "Documentation language for generated skill metadata: en (default) or zh",
            "flag",
        )],
        "ve-adrive skill export" => vec![
            describe_meta_parameter(
                "name",
                false,
                "Optional skill name, command path, or business domain filter",
                "flag",
            ),
            describe_meta_parameter(
                "dir",
                false,
                "Output directory for exported Markdown skill files",
                "flag",
            ),
            describe_meta_parameter(
                "language",
                false,
                "Documentation language for generated SKILL.md files: en (default) or zh",
                "flag",
            ),
        ],
        _ => row
            .parameters
            .iter()
            .map(|parameter| {
                describe_meta_parameter(
                    parameter.name,
                    parameter.required,
                    parameter.description,
                    if parameter.name == "shell" {
                        "path"
                    } else {
                        "flag"
                    },
                )
            })
            .collect(),
    }
}

fn describe_meta_parameter(name: &str, required: bool, description: &str, location: &str) -> Value {
    json!({
        "name": name,
        "location": location,
        "required": required,
        "description": description,
        "schema": { "type": parameter_schema_type(name) },
    })
}

fn describe_meta_scenario_routing(command_path: &str) -> Value {
    let mut routing = base_meta_scenario_routing();
    match command_path {
        "ve-adrive crt" => insert_create_routing(&mut routing),
        "ve-adrive auth" => insert_auth_routing(&mut routing),
        "ve-adrive completion" => insert_completion_routing(&mut routing),
        "ve-adrive serve" => insert_serve_routing(&mut routing),
        "ve-adrive skill list" | "ve-adrive skill export" => insert_skill_routing(&mut routing),
        _ => {}
    }
    Value::Object(routing)
}

fn insert_auth_routing(routing: &mut serde_json::Map<String, Value>) {
    routing.insert(
        "mode_resolution".to_string(),
        json!("--auth-mode <MODE> > profile auth_mode > ADRIVE_AUTH_MODE > aksk"),
    );
    routing.insert(
        "unified_identity".to_string(),
        json!("Unified uses the same-name external profile, ignores local AK/SK and OAuth credentials, and delegates login/logout to `ve login` / `ve logout`"),
    );
}

fn insert_create_routing(routing: &mut serde_json::Map<String, Value>) {
    routing.insert(
        "create_defaults".to_string(),
        json!(
            "Instance: AK/SK defaults --service-type to arkclaw and OAuth defaults it to paas. Space: OAuth user Space ownership defaults to the logged-in user_id; --owner-type group requires --owner-id."
        ),
    );
}

fn base_meta_scenario_routing() -> serde_json::Map<String, Value> {
    let mut routing = serde_json::Map::new();
    routing.insert(
        "dry_run".to_string(),
        json!("returns a deterministic plan without mutating local files or ADrive resources"),
    );
    routing.insert(
        "output".to_string(),
        json!("success and failure paths use Envelope plus --query and multi-format rendering"),
    );
    routing
}

fn insert_completion_routing(routing: &mut serde_json::Map<String, Value>) {
    routing.insert(
        "install_flow".to_string(),
        json!("the command returns an Envelope; install by extracting data.script, then source bash output, add ~/.zfunc to zsh fpath and run compinit, write fish output under ~/.config/fish/completions, or append PowerShell output to $PROFILE"),
    );
    routing.insert(
        "registered_command_names".to_string(),
        json!("generated scripts register ve-adrive-cli and ve-adrive"),
    );
}

fn insert_serve_routing(routing: &mut serde_json::Map<String, Value>) {
    routing.insert(
        "transport_matrix".to_string(),
        json!("stdio uses stdin/stdout and opens no TCP listener; sse starts a local rmcp HTTP/SSE listener on 127.0.0.1:<port>"),
    );
    routing.insert(
        "tool_source".to_string(),
        json!("MCP tools are rebuilt from the in-process skill registry; exported Markdown skill files are not read by serve"),
    );
    routing.insert(
        "call_semantics".to_string(),
        json!(
            "tools/call plans by default; include execute=true to run the underlying CLI command"
        ),
    );
}

fn insert_skill_routing(routing: &mut serde_json::Map<String, Value>) {
    routing.insert(
        "format".to_string(),
        json!("Markdown SKILL.md pack with root index plus per-domain command skills"),
    );
    routing.insert(
        "consumers".to_string(),
        json!("external Agent catalogs, prompt context, documentation generators, adapters, and MCP tool advertisement"),
    );
    routing.insert(
        "serve_relationship".to_string(),
        json!(
            "serve uses the same live registry data but does not read the exported Markdown skill directory"
        ),
    );
}

fn describe_meta_examples(command_path: &str) -> Vec<String> {
    match command_path {
        "ve-adrive auth" => vec![
            public_adrive_command("ve-adrive auth status"),
            public_adrive_command("ve-adrive --profile default --auth-mode unified auth status"),
            public_adrive_command("ve-adrive config set auth_mode unified"),
            "ve login".to_string(),
            "ve logout".to_string(),
        ],
        "ve-adrive completion" => vec![
            public_adrive_command("ve-adrive completion bash --output json"),
            format!(
                "{} | jq -r '.data.script' > ~/.ve-adrive-completion.bash",
                public_adrive_command("ve-adrive completion bash --output json")
            ),
            format!(
                "{} | jq -r '.data.script' > ~/.zfunc/_ve-adrive",
                public_adrive_command("ve-adrive completion zsh --output json")
            ),
        ],
        "ve-adrive serve" => vec![
            public_adrive_command("ve-adrive serve --mcp"),
            public_adrive_command("ve-adrive serve --mcp --transport sse --port 9090"),
            public_adrive_command("ve-adrive serve --mcp --dry-run --output json"),
        ],
        "ve-adrive skill list" => vec![
            public_adrive_command("ve-adrive skill list"),
            public_adrive_command("ve-adrive skill list --language zh"),
        ],
        "ve-adrive skill export" => vec![
            public_adrive_command("ve-adrive skill export --dir ./ve-adrive-skills"),
            public_adrive_command(
                "ve-adrive skill export --name ve_adrive_ls --dir ./ve-adrive-skills --dry-run --output json",
            ),
            public_adrive_command("ve-adrive skill export --language zh --dir ./ve-adrive-skills-zh"),
        ],
        _ => vec![public_adrive_command(command_path)],
    }
}

fn redact_adrive_effective(effective: EffectiveProfile) -> EffectiveProfile {
    let mut redacted = redact_effective(effective);
    for token in [&mut redacted.access_token, &mut redacted.refresh_token]
        .into_iter()
        .flatten()
    {
        if token.value.is_some() {
            token.value = Some("****".to_string());
        }
    }
    // Keep ADrive config show aligned with runtime profile loading:
    // ADrive does not inherit shared TOS network settings or credentials.
    if redacted.region.source == FieldSource::Shared {
        redacted.region.value = None;
        redacted.region.source = FieldSource::Unset;
    }
    if redacted.endpoint.source == FieldSource::Shared {
        redacted.endpoint.value = None;
        redacted.endpoint.source = FieldSource::Unset;
    }
    if redacted.control_endpoint.source == FieldSource::Shared {
        redacted.control_endpoint.value = None;
        redacted.control_endpoint.source = FieldSource::Unset;
    }
    if redacted.access_key_id.source == FieldSource::Shared {
        redacted.access_key_id.value = None;
        redacted.access_key_id.source = FieldSource::Unset;
    }
    if redacted.secret_access_key.source == FieldSource::Shared {
        redacted.secret_access_key.value = None;
        redacted.secret_access_key.source = FieldSource::Unset;
    }
    if redacted.security_token.source == FieldSource::Shared {
        redacted.security_token.value = None;
        redacted.security_token.source = FieldSource::Unset;
    }
    redacted
}

fn push_traced_row(
    rows: &mut Vec<Vec<String>>,
    eff: &EffectiveProfile,
    field: &str,
    tf: &tos_core::infra::config::TracedField<String>,
) {
    let source = tf.source.label(&eff.profile_name, &eff.binary);
    rows.push(vec![
        eff.profile_name.clone(),
        field.to_string(),
        tf.value.clone().unwrap_or_else(|| "-".to_string()),
        source,
    ]);
}

fn push_traced_bool_row(
    rows: &mut Vec<Vec<String>>,
    eff: &EffectiveProfile,
    field: &str,
    tf: &tos_core::infra::config::TracedField<bool>,
) {
    let source = tf.source.label(&eff.profile_name, &eff.binary);
    rows.push(vec![
        eff.profile_name.clone(),
        field.to_string(),
        tf.value
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        source,
    ]);
}

fn push_traced_value_row<T>(
    rows: &mut Vec<Vec<String>>,
    eff: &EffectiveProfile,
    field: &str,
    tf: &tos_core::infra::config::TracedField<T>,
) where
    T: Clone + serde::Serialize + ToString,
{
    let source = tf.source.label(&eff.profile_name, &eff.binary);
    rows.push(vec![
        eff.profile_name.clone(),
        field.to_string(),
        tf.value
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "-".to_string()),
        source,
    ]);
}

/// Handle ADrive completion.
pub async fn handle_completion_command(
    global: &GlobalArgs,
    args: &CompletionArgs,
) -> Result<i32, CliError> {
    if global.describe {
        let description =
            describe_adrive_command_metadata("ve-adrive completion").ok_or_else(|| {
                CliError::ValidationError(
                    "no metadata registered for ve-adrive completion".to_string(),
                )
            })?;
        output_result(
            global,
            &Envelope::success("ve-adrive completion", description),
        )?;
        return Ok(0);
    }
    let script = completion_script(&args.shell)?;
    let envelope = Envelope::success(
        "ve-adrive completion",
        json!({
            "shell": &args.shell,
            "script": script,
            "command_count": capabilities().len(),
            "status": "generated",
            "message": format!("Shell completion for {} generated", args.shell),
        }),
    );
    output_envelope(global, &envelope)?;
    Ok(0)
}

/// Handle ADrive MCP serving.
pub async fn handle_serve_command(global: &GlobalArgs, args: &ServeArgs) -> Result<i32, CliError> {
    let transport = args.transport.as_str();
    if args.mcp && !global.dry_run && !global.describe {
        match transport {
            "stdio" => run_mcp_stdio(global).await?,
            "sse" => run_mcp_sse(global, args.port).await?,
            other => {
                return Err(CliError::ValidationError(format!(
                    "unsupported serve transport '{}': expected stdio or sse",
                    other
                )));
            }
        }
        return Ok(0);
    }
    output_envelope(
        global,
        &Envelope::success("ve-adrive serve", serve_plan(args)?),
    )?;
    Ok(0)
}

fn serve_plan(args: &ServeArgs) -> Result<Value, CliError> {
    if !matches!(args.transport.as_str(), "stdio" | "sse") {
        return Err(CliError::ValidationError(format!(
            "unsupported serve transport '{}': expected stdio or sse",
            args.transport
        )));
    }
    let is_sse = args.transport == "sse";
    Ok(json!({
        "mode": if args.mcp { "mcp" } else { "registry" },
        "transport": &args.transport,
        "port": is_sse.then_some(args.port),
        "protocol": "MCP standard protocol via rmcp",
        "tcp_listener": is_sse,
        "bind": is_sse.then(|| format!("127.0.0.1:{}", args.port)),
        "endpoints": if is_sse { vec!["/sse", "/message"] } else { Vec::new() },
        "authentication": if is_sse { "ephemeral_bearer" } else { "process_stdio" },
        "token_output": is_sse.then_some("stderr_once_after_bind"),
        "authorization_header_required": is_sse,
        "allowed_hosts": if is_sse {
            vec![
                format!("127.0.0.1:{}", args.port),
                format!("localhost:{}", args.port),
            ]
        } else {
            Vec::new()
        },
        "origin_policy": is_sse.then_some("missing_or_exact_http_loopback_origin_same_port"),
        "tool_source": "In-process ADrive skill registry; exported Markdown skill files are not read by serve.",
        "call_semantics": "tools/call plans by default; include execute=true to run the underlying CLI command.",
        "capabilities": capabilities().len(),
        "skill_domains": command_domains(),
        "status": "planned_not_started",
        "message": "ADrive serve exposes registry-backed MCP tools; long-running startup is intentionally deferred for dry-run/describe",
    }))
}

async fn run_mcp_stdio(global: &GlobalArgs) -> Result<(), CliError> {
    build_mcp_server(global)?
        .run_stdio()
        .await
        .map_err(CliError::Io)?;
    Ok(())
}

async fn run_mcp_sse(global: &GlobalArgs, port: u16) -> Result<(), CliError> {
    let bind: SocketAddr = ([127, 0, 0, 1], port).into();
    build_mcp_server(global)?
        .run_sse(bind)
        .await
        .map_err(CliError::Io)?;
    Ok(())
}

fn build_mcp_server(global: &GlobalArgs) -> Result<tos_core::mcp::TosMcpServer, CliError> {
    use std::sync::Arc;
    use tos_core::mcp::{
        ToolDispatcher, ToolEntry, ToolInvocation, ToolInvocationResult, TosMcpServer,
    };

    let entries = skill_definitions()
        .into_iter()
        .map(|skill| {
            ToolEntry::from_parts(
                skill.name,
                skill.description,
                skill.input_schema,
                matches!(skill.risk_level.as_str(), "high" | "critical"),
            )
        })
        .collect::<Vec<_>>();

    struct ADriveDispatcher {
        global: GlobalArgs,
    }

    impl ToolDispatcher for ADriveDispatcher {
        fn dispatch<'a>(
            &'a self,
            invocation: ToolInvocation,
        ) -> tos_core::mcp::server::DispatchFuture<'a> {
            Box::pin(async move {
                match mcp_invoke_tool(&self.global, invocation.name, invocation.arguments).await {
                    Ok((payload, is_error)) => Ok(ToolInvocationResult { payload, is_error }),
                    Err(err) => Err(err.to_string()),
                }
            })
        }
    }

    let dispatcher: Arc<dyn ToolDispatcher> = Arc::new(ADriveDispatcher {
        global: global.clone(),
    });
    Ok(TosMcpServer::new(
        "adrive-uni-cli",
        env!("CARGO_PKG_VERSION"),
        entries,
        dispatcher,
    ))
}

async fn mcp_invoke_tool(
    global: &GlobalArgs,
    name: String,
    arguments: Value,
) -> Result<(Value, bool), CliError> {
    let skill = skill_definitions()
        .into_iter()
        .find(|skill| skill.name == name)
        .ok_or_else(|| CliError::ValidationError(format!("unknown MCP tool '{}'", name)))?;
    mcp_execute_typed_command(global, &skill, &arguments).await
}

async fn mcp_execute_typed_command(
    global: &GlobalArgs,
    skill: &SkillDefinition,
    arguments: &Value,
) -> Result<(Value, bool), CliError> {
    let object = arguments.as_object().ok_or_else(|| {
        CliError::ValidationError(format!("{} arguments must be a JSON object", skill.name))
    })?;
    let execute = bool_field(object, "execute").unwrap_or(false);
    let argv = build_mcp_typed_argv(global, &skill.internal_command, object)?;
    if !execute {
        return Ok((
            json!({
                "command": skill.command,
                "argv": argv,
                "execution_status": "planned_not_executed",
            }),
            false,
        ));
    }
    if skill.internal_command == "ve-adrive serve" {
        // [Review Fix #1] Do not allow a tool call to start another long-running MCP server inside the active MCP request.
        return Err(CliError::ValidationError(
            "ve_adrive_serve MCP tool only supports planning; omit execute=true and use dry_run/describe"
                .to_string(),
        ));
    }
    let result = run_mcp_typed_argv(&skill.command, argv).await?;
    let is_error = result.exit_code.unwrap_or(1) != 0;
    let payload = serde_json::to_value(result).map_err(CliError::Json)?;
    Ok((payload, is_error))
}

fn build_mcp_typed_argv(
    global: &GlobalArgs,
    command: &str,
    arguments: &serde_json::Map<String, Value>,
) -> Result<Vec<String>, CliError> {
    let row = find_capability(command).ok_or_else(|| {
        CliError::ValidationError(format!("unknown typed MCP command '{}'", command))
    })?;
    let mut argv = Vec::new();
    push_mcp_global_args(global, arguments, &mut argv)?;
    push_mcp_public_command_path(command, &mut argv);
    push_mcp_command_args(row, arguments, &mut argv)?;
    Ok(argv)
}

fn push_mcp_public_command_path(command: &str, argv: &mut Vec<String>) {
    let mut parts = command.split_whitespace();
    let Some(first_part) = parts.next() else {
        return;
    };
    // [Review Fix #26] MCP subprocess execution uses the canonical public
    // top-level command directly; old `adrive` command paths are unsupported.
    argv.push(first_part.to_string());
    argv.extend(parts.map(ToString::to_string));
}

fn push_mcp_global_args(
    global: &GlobalArgs,
    arguments: &serde_json::Map<String, Value>,
    argv: &mut Vec<String>,
) -> Result<(), CliError> {
    argv.push("--output".to_string());
    argv.push(
        string_field(arguments, "output")
            .unwrap_or("json")
            .to_string(),
    );
    argv.push("--profile".to_string());
    argv.push(
        string_field(arguments, "profile")
            .unwrap_or(&global.profile)
            .to_string(),
    );
    for (field, flag, fallback) in [
        ("region", "--region", global.region.as_deref()),
        ("endpoint", "--endpoint", global.endpoint.as_deref()),
    ] {
        if let Some(value) = string_field(arguments, field).or(fallback) {
            argv.push(flag.to_string());
            argv.push(value.to_string());
        }
    }
    for (field, flag, fallback) in [
        ("dry_run", "--dry-run", global.dry_run),
        ("describe", "--describe", global.describe),
        ("verbose", "--verbose", global.verbose),
        ("quiet", "--quiet", global.quiet),
    ] {
        if bool_field(arguments, field).unwrap_or(fallback) {
            argv.push(flag.to_string());
        }
    }
    Ok(())
}

fn push_mcp_command_args(
    row: &CapabilityRow,
    arguments: &serde_json::Map<String, Value>,
    argv: &mut Vec<String>,
) -> Result<(), CliError> {
    let reserved = [
        "execute", "output", "profile", "region", "endpoint", "dry_run", "describe", "verbose",
        "quiet",
    ];
    for key in arguments.keys() {
        if reserved.contains(&key.as_str()) {
            continue;
        }
        if !row.parameters.iter().any(|param| param.name == key) {
            return Err(CliError::ValidationError(format!(
                "unknown argument '{}' for MCP tool '{}'",
                key, row.command
            )));
        }
    }
    for parameter in row
        .parameters
        .iter()
        .filter(|parameter| is_positional_parameter(row.command, parameter.name))
    {
        if let Some(value) = arguments.get(parameter.name) {
            push_mcp_argument_value(argv, None, value)?;
        } else if parameter.required {
            return Err(CliError::ValidationError(format!(
                "missing required argument '{}' for MCP tool '{}'",
                parameter.name, row.command
            )));
        }
    }
    for parameter in row
        .parameters
        .iter()
        .filter(|parameter| !is_positional_parameter(row.command, parameter.name))
    {
        let Some(value) = arguments.get(parameter.name) else {
            continue;
        };
        let flag = format!("--{}", parameter.name.replace('_', "-"));
        if is_boolean_parameter(parameter.name) {
            if value.as_bool().unwrap_or(false) {
                argv.push(flag);
            }
            continue;
        }
        push_mcp_argument_value(argv, Some(&flag), value)?;
    }
    Ok(())
}

fn is_positional_parameter(command: &str, name: &str) -> bool {
    matches!(
        (command, name),
        (
            "ve-adrive cp" | "ve-adrive mv" | "ve-adrive sync",
            "source" | "destination"
        ) | (
            "ve-adrive ls" | "ve-adrive crt" | "ve-adrive del" | "ve-adrive rm",
            "path"
        ) | ("ve-adrive api", "group" | "action")
            | ("ve-adrive completion", "shell")
    )
}

fn push_mcp_argument_value(
    argv: &mut Vec<String>,
    flag: Option<&str>,
    value: &Value,
) -> Result<(), CliError> {
    match value {
        Value::Null => Ok(()),
        Value::Array(values) => {
            for item in values {
                push_mcp_argument_value(argv, flag, item)?;
            }
            Ok(())
        }
        Value::String(_) | Value::Bool(_) | Value::Number(_) => {
            if let Some(flag) = flag {
                argv.push(flag.to_string());
            }
            argv.push(value_to_cli_string(value)?);
            Ok(())
        }
        Value::Object(_) => Err(CliError::ValidationError(
            "MCP typed command arguments must be scalar values or arrays".to_string(),
        )),
    }
}

fn value_to_cli_string(value: &Value) -> Result<String, CliError> {
    match value {
        Value::String(text) => Ok(text.clone()),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Number(value) => Ok(value.to_string()),
        Value::Null => Ok(String::new()),
        Value::Array(_) | Value::Object(_) => Err(CliError::ValidationError(
            "MCP typed command argument cannot be converted to a CLI scalar".to_string(),
        )),
    }
}

async fn run_mcp_typed_argv(
    command: &str,
    argv: Vec<String>,
) -> Result<McpCommandExecution, CliError> {
    let exe = std::env::current_exe()?;
    let output = timeout(
        Duration::from_secs(300),
        TokioCommand::new(exe).args(&argv).output(),
    )
    .await
    .map_err(|_| {
        CliError::ValidationError(format!(
            "MCP typed command '{}' timed out after 300 seconds",
            command
        ))
    })??;
    Ok(McpCommandExecution {
        command: command.to_string(),
        argv,
        exit_code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

fn string_field<'a>(object: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a str> {
    object.get(key).and_then(Value::as_str)
}

fn bool_field(object: &serde_json::Map<String, Value>, key: &str) -> Option<bool> {
    object.get(key).and_then(Value::as_bool)
}

// ---------------------------------------------------------------------------
// Doctor types (local, aligned with TOS doctor)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct DoctorCheck {
    name: &'static str,
    status: &'static str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    fix_command: Option<String>,
    details: Value,
}

#[derive(Debug, Serialize)]
struct DoctorSummary {
    total: usize,
    passed: usize,
    warnings: usize,
    failed: usize,
}

#[derive(Debug, Serialize)]
struct DoctorReport {
    profile: String,
    checks: Vec<DoctorCheck>,
    summary: DoctorSummary,
}

/// Handle ADrive doctor.
pub async fn handle_doctor_command(
    global: &GlobalArgs,
    auth: &ADriveAuthArgs,
    args: &DoctorArgs,
) -> Result<i32, CliError> {
    let report = doctor_report(global, auth, args).await?;
    let envelope = Envelope::success("ve-adrive doctor", serde_json::to_value(&report).unwrap());
    output_envelope(global, &envelope)?;
    Ok(0)
}

async fn doctor_report(
    global: &GlobalArgs,
    auth: &ADriveAuthArgs,
    args: &DoctorArgs,
) -> Result<DoctorReport, CliError> {
    let checks = build_doctor_checks(global, auth, args).await?;
    let passed = checks.iter().filter(|c| c.status == "passed").count();
    let warnings = checks.iter().filter(|c| c.status == "warning").count();
    let failed = checks.iter().filter(|c| c.status == "failed").count();
    Ok(DoctorReport {
        profile: global.profile.clone(),
        summary: DoctorSummary {
            total: checks.len(),
            passed,
            warnings,
            failed,
        },
        checks,
    })
}

fn completion_script(shell: &str) -> Result<String, CliError> {
    let commands = completion_words().join(" ");
    match shell {
        "bash" => Ok(format!(
            "_adrive_complete() {{\n  local cur=\"${{COMP_WORDS[COMP_CWORD]}}\"\n  if [[ \"${{COMP_WORDS[0]}}\" == \"ve-storage-uni-cli\" ]]; then\n    if [[ \"$COMP_CWORD\" -eq 1 ]]; then\n      COMPREPLY=( $(compgen -W \"ve-adrive\" -- \"$cur\") )\n      return\n    fi\n    [[ \"${{COMP_WORDS[1]}}\" == \"ve-adrive\" ]] || return\n  fi\n  COMPREPLY=( $(compgen -W \"{commands}\" -- \"$cur\") )\n}}\ncomplete -F _adrive_complete ve-adrive\ncomplete -F _adrive_complete ve-adrive-cli\ncomplete -F _adrive_complete ve-storage-uni-cli"
        )),
        "zsh" => Ok(format!(
            "#compdef ve-adrive ve-adrive-cli ve-storage-uni-cli\n_arguments '1:command:(ve-adrive {commands})'"
        )),
        "fish" => Ok(commands
            .split_whitespace()
            .flat_map(|cmd| {
                [
                    format!("complete -c ve-adrive -f -a {cmd}"),
                    format!("complete -c ve-adrive-cli -f -a {cmd}"),
                    format!("complete -c ve-storage-uni-cli -n '__fish_seen_subcommand_from ve-adrive' -f -a {cmd}"),
                ]
            })
            .chain(["complete -c ve-storage-uni-cli -f -a ve-adrive".to_string()])
            .collect::<Vec<_>>()
            .join("\n")),
        "powershell" | "pwsh" => Ok(format!(
            "Register-ArgumentCompleter -Native -CommandName ve-adrive,ve-adrive-cli,ve-storage-uni-cli -ScriptBlock {{\n  param($wordToComplete, $commandAst, $cursorPosition)\n  @('ve-adrive',{cmds}) | Where-Object {{ $_ -like \"$wordToComplete*\" }} | ForEach-Object {{ [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_) }}\n}}\n",
            cmds = commands
                .split_whitespace()
                .map(|command| format!("'{}'", command.replace('\'', "''")))
                .collect::<Vec<_>>()
                .join(",")
        )),
        other => Err(CliError::ValidationError(format!(
            "unsupported completion shell '{}': expected bash, zsh, fish, or powershell",
            other
        ))),
    }
}

fn completion_words() -> Vec<&'static str> {
    let mut words = capabilities()
        .iter()
        .filter_map(|row| row.command.strip_prefix("ve-adrive "))
        .filter_map(|suffix| suffix.split_whitespace().next())
        .collect::<Vec<_>>();
    words.sort_unstable();
    words.dedup();
    words
}

async fn build_doctor_checks(
    global: &GlobalArgs,
    auth: &ADriveAuthArgs,
    args: &DoctorArgs,
) -> Result<Vec<DoctorCheck>, CliError> {
    let selected = args.check.as_deref();
    let auth_context = doctor_auth_context(global, auth, selected);
    let mut checks = Vec::new();
    if doctor_check_selected(selected, "config") {
        checks.push(config_doctor_check(global, &auth_context));
    }
    if doctor_check_selected(selected, "auth") {
        checks.push(auth_doctor_check(global, &auth_context).await);
    }
    maybe_push_check(&mut checks, selected, "registry", registry_check);
    // network_check is async because --live-network performs a real HTTPS probe.
    if doctor_check_selected(selected, "network") {
        checks.push(network_doctor_check(global, args, &auth_context).await);
    }
    maybe_push_check(&mut checks, selected, "mcp", mcp_check);
    maybe_push_check(&mut checks, selected, "completion", completion_check);
    if selected == Some("principles") {
        // [Review Fix #DoctorLazy] Keep ordinary doctor output fast; run the
        // cross-surface invariant check only when explicitly requested.
        checks.push(principles_check());
    }
    if checks.is_empty() {
        return Err(CliError::ValidationError(format!(
            "unknown doctor check '{}': expected auth, config, registry, network, mcp, principles, or completion",
            selected.unwrap_or_default()
        )));
    }
    Ok(checks)
}

enum DoctorAuthContext {
    Ready(ResolvedAuthMode),
    Failed(String),
    NotNeeded,
}

impl DoctorAuthContext {
    fn resolved(&self) -> Result<ResolvedAuthMode, &str> {
        match self {
            Self::Ready(resolved) => Ok(*resolved),
            Self::Failed(message) => Err(message),
            Self::NotNeeded => Err("doctor auth mode unavailable"),
        }
    }
}

fn doctor_auth_context(
    global: &GlobalArgs,
    auth: &ADriveAuthArgs,
    selected: Option<&str>,
) -> DoctorAuthContext {
    // [Review Fix #5] Independent diagnostics do not parse authentication
    // configuration; dependent checks retain resolution failures as rows.
    if selected.is_some() && !matches!(selected, Some("auth" | "config" | "network")) {
        return DoctorAuthContext::NotNeeded;
    }
    match crate::handler::common::resolve_auth_mode(global, auth.auth_mode) {
        Ok(resolved) => DoctorAuthContext::Ready(resolved),
        Err(error) => DoctorAuthContext::Failed(error.to_string()),
    }
}

fn config_doctor_check(global: &GlobalArgs, context: &DoctorAuthContext) -> DoctorCheck {
    match context.resolved() {
        Ok(resolved) => config_check(global, resolved.mode)
            .unwrap_or_else(|error| failed_doctor_check("config", error)),
        Err(error) => failed_doctor_check("config", error),
    }
}

async fn auth_doctor_check(global: &GlobalArgs, context: &DoctorAuthContext) -> DoctorCheck {
    match context.resolved() {
        Ok(resolved) => auth_check(global, resolved)
            .await
            .unwrap_or_else(|error| failed_doctor_check("auth", error)),
        Err(error) => failed_doctor_check("auth", error),
    }
}

async fn network_doctor_check(
    global: &GlobalArgs,
    args: &DoctorArgs,
    context: &DoctorAuthContext,
) -> DoctorCheck {
    match context.resolved() {
        Ok(resolved) => network_check(global, args, resolved.mode)
            .await
            .unwrap_or_else(|error| failed_doctor_check("network", error)),
        Err(error) => failed_doctor_check("network", error),
    }
}

fn failed_doctor_check(name: &'static str, error: impl std::fmt::Display) -> DoctorCheck {
    DoctorCheck {
        name,
        status: "failed",
        message: error.to_string(),
        fix_command: None,
        details: json!({ "recoverable": true }),
    }
}

fn doctor_check_selected(selected: Option<&str>, name: &str) -> bool {
    selected.map(|value| value == name).unwrap_or(true)
}

fn maybe_push_check<F>(
    checks: &mut Vec<DoctorCheck>,
    selected: Option<&str>,
    name: &'static str,
    build: F,
) where
    F: FnOnce() -> DoctorCheck,
{
    if selected
        .map(|selected_name| selected_name == name)
        .unwrap_or(true)
    {
        checks.push(build());
    }
}

fn config_check(global: &GlobalArgs, mode: AuthMode) -> Result<DoctorCheck, CliError> {
    let path = global.config_path();
    // [Review Fix #2] Once Doctor selects Unified, its non-auth checks must not
    // reload or decrypt unselected local AK/SK credentials.
    let profile = build_runtime_profile(global, mode)?;
    let resolved = resolve_endpoint_and_region(profile.endpoint.clone(), profile.region.clone());
    let is_resource_configured = resolved.is_ok();
    let has_explicit_endpoint = profile
        .endpoint
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    let fix_command = (!is_resource_configured)
        .then(|| resource_config_fix_command(has_explicit_endpoint).to_string());
    Ok(DoctorCheck {
        name: "config",
        status: if is_resource_configured {
            "passed"
        } else {
            "warning"
        },
        message: resolved
            .as_ref()
            .map(|_| "effective ADrive resource configuration loaded".to_string())
            .unwrap_or_else(|error| error.to_string()),
        fix_command,
        details: json!({
            "config_exists": path.exists(),
            "config_path": path.display().to_string(),
            "has_endpoint": profile.endpoint.is_some(),
            "has_region": profile.region.is_some(),
        }),
    })
}

async fn auth_check(
    global: &GlobalArgs,
    resolved: ResolvedAuthMode,
) -> Result<DoctorCheck, CliError> {
    if resolved.mode == AuthMode::Unified {
        let inspection = inspect_unified_credentials_for_profile(&global.profile).await;
        return Ok(unified_auth_doctor_check(
            &global.profile,
            resolved,
            &inspection,
        ));
    }
    let credentials = inspect_selected_credentials(global, resolved.mode)?;
    let is_ready = credentials.is_ready();
    let has_login_auth_endpoint = resolved.mode != AuthMode::Oauth
        || crate::handler::auth::has_configured_login_auth_endpoint(global)?;
    let client_id = (resolved.mode == AuthMode::Oauth).then(oauth_client_id_diagnostics);
    let uses_process_access_token =
        credentials.credential_source == "environment" && credentials.is_ready();
    let has_client_id_warning =
        client_id.is_some_and(|value| value.is_placeholder && !uses_process_access_token);
    let status = if is_ready && !has_client_id_warning {
        "passed"
    } else {
        "warning"
    };
    let credential_state = credential_state(resolved.mode, &credentials);
    let fix_command = if resolved.mode == AuthMode::Oauth && !is_ready && !has_login_auth_endpoint {
        Some("ve-adrive config set auth_endpoint <url>".to_string())
    } else {
        auth_fix_command(resolved.mode, &credentials, is_ready, has_client_id_warning)
    };
    let message = if resolved.mode == AuthMode::Oauth && !is_ready && !has_login_auth_endpoint {
        "ADrive OAuth Authorization Server is not configured".to_string()
    } else {
        auth_check_message(resolved.mode, &credentials, is_ready, has_client_id_warning)
    };
    let mut details = auth_check_details(resolved, &credentials, credential_state);
    if let Some(details) = details.as_object_mut() {
        details.insert(
            "has_login_auth_endpoint".to_string(),
            Value::Bool(has_login_auth_endpoint),
        );
    }
    add_oauth_client_id_details(&mut details, client_id);
    Ok(DoctorCheck {
        name: "auth",
        status,
        message,
        fix_command,
        details,
    })
}

fn unified_auth_doctor_check(
    profile_name: &str,
    resolved: crate::domain::auth::ResolvedAuthMode,
    inspection: &UnifiedCredentialInspection,
) -> DoctorCheck {
    let mut details = json!({
        "mode": resolved.mode.as_str(),
        "source": resolved.source.as_str(),
        "profile": profile_name,
        "provider_name": inspection.provider_name,
        "has_session_token": inspection.has_session_token,
        "ready": inspection.ready,
    });
    if let (Some(details), Some(sdk_code)) = (details.as_object_mut(), inspection.sdk_code.as_ref())
    {
        details.insert("sdk_code".to_string(), json!(sdk_code));
    }
    DoctorCheck {
        name: "auth",
        status: if inspection.ready {
            "passed"
        } else {
            "warning"
        },
        message: if inspection.ready {
            "ADrive Unified credentials are ready".to_string()
        } else {
            "ADrive Unified credentials are unavailable; login is managed externally".to_string()
        },
        fix_command: (!inspection.ready).then(|| "ve login".to_string()),
        details,
    }
}

fn auth_fix_command(
    mode: AuthMode,
    credentials: &crate::domain::auth::CredentialAvailability,
    is_ready: bool,
    has_client_id_warning: bool,
) -> Option<String> {
    if is_ready || has_client_id_warning {
        return None;
    }
    match mode {
        AuthMode::Oauth => Some("ve-adrive auth login".to_string()),
        AuthMode::Unified => Some("ve login".to_string()),
        AuthMode::Aksk => match (credentials.has_access_key, credentials.has_secret_key) {
            (Some(false), Some(true)) => {
                Some("ve-adrive config set access_key_id <access_key_id>".to_string())
            }
            (Some(true), Some(false)) => {
                Some("ve-adrive config set secret_access_key <secret_access_key>".to_string())
            }
            _ => Some("ve-adrive config init".to_string()),
        },
    }
}

fn auth_check_message(
    mode: AuthMode,
    credentials: &crate::domain::auth::CredentialAvailability,
    is_ready: bool,
    has_client_id_warning: bool,
) -> String {
    if has_client_id_warning {
        return "ADrive OAuth Client ID is still a placeholder; use a registered test override or a production build"
            .to_string();
    }
    if mode == AuthMode::Aksk && credentials.has_complete_aksk() {
        return "ADrive credentials are configured".to_string();
    }
    match (mode, is_ready, credentials.has_oauth_token()) {
        (AuthMode::Oauth, true, _) => "ADrive OAuth credentials are available".to_string(),
        (AuthMode::Oauth, false, true) => {
            "ADrive OAuth credentials are present but cannot authenticate; run auth login"
                .to_string()
        }
        (AuthMode::Oauth, false, false) => {
            "ADrive OAuth credentials are not configured".to_string()
        }
        (AuthMode::Unified, _, _) => {
            "ADrive unified credentials are managed externally".to_string()
        }
        (AuthMode::Aksk, false, _) => aksk_missing_credential_message(credentials),
        (AuthMode::Aksk, true, _) => "ADrive credentials are configured".to_string(),
    }
}

fn aksk_missing_credential_message(
    credentials: &crate::domain::auth::CredentialAvailability,
) -> String {
    match (credentials.has_access_key, credentials.has_secret_key) {
        (Some(false), Some(true)) => "ADrive Access Key is not configured".to_string(),
        (Some(true), Some(false)) => "ADrive Secret Key is not configured".to_string(),
        _ => "ADrive Access Key and Secret Key are not configured".to_string(),
    }
}

fn auth_check_details(
    resolved: crate::domain::auth::ResolvedAuthMode,
    credentials: &crate::domain::auth::CredentialAvailability,
    credential_state: &str,
) -> Value {
    let mut details = json!({
        "mode": resolved.mode.as_str(),
        "source": resolved.source.as_str(),
        "credential_state": credential_state,
        "has_access_key": credentials.has_access_key,
        "has_secret_key": credentials.has_secret_key,
        "has_security_token": credentials.has_security_token,
        "has_access_token": credentials.has_access_token,
        "has_refresh_token": credentials.has_refresh_token,
        "access_token_expiry": credentials.access_token_expiry,
        "expires_at": credentials.expires_at,
        "scope": credentials.scope,
        "instance_id": credentials.instance_id,
        "ready": credentials.is_ready(),
        "oauth_service_integration": credentials.oauth_service_integration,
        "credential_source": credentials.credential_source,
    });
    if resolved.mode == AuthMode::Aksk {
        add_aksk_source_details(&mut details, credentials);
    }
    details
}

fn add_aksk_source_details(
    details: &mut Value,
    credentials: &crate::domain::auth::CredentialAvailability,
) {
    let Some(details) = details.as_object_mut() else {
        return;
    };
    details.insert(
        "access_key_source".to_string(),
        Value::String(credentials.access_key_source.unwrap_or("none").to_string()),
    );
    details.insert(
        "secret_key_source".to_string(),
        Value::String(credentials.secret_key_source.unwrap_or("none").to_string()),
    );
    details.insert(
        "security_token_source".to_string(),
        Value::String(
            credentials
                .security_token_source
                .unwrap_or("none")
                .to_string(),
        ),
    );
}

fn add_oauth_client_id_details(details: &mut Value, client_id: Option<OAuthClientIdDiagnostics>) {
    let (Some(details), Some(client_id)) = (details.as_object_mut(), client_id) else {
        return;
    };
    details.insert(
        "oauth_client_id_source".to_string(),
        Value::String(client_id.source.to_string()),
    );
    details.insert(
        "oauth_client_id_placeholder".to_string(),
        Value::Bool(client_id.is_placeholder),
    );
}

fn credential_state(
    mode: AuthMode,
    credentials: &crate::domain::auth::CredentialAvailability,
) -> &'static str {
    if mode == AuthMode::Aksk {
        return if credentials.is_ready() {
            "configured"
        } else {
            "incomplete"
        };
    }
    if credentials.is_ready() {
        // [Review Fix #2] A malformed expiry is usable only when the stored
        // credential group can refresh it, so report that state explicitly.
        return if matches!(
            credentials.access_token_expiry.as_deref(),
            Some("expired" | "invalid")
        ) || credentials.has_access_token == Some(false)
        {
            "refreshable"
        } else {
            "access_available"
        };
    }
    if credentials.credential_source == "environment"
        && credentials.has_access_token == Some(false)
        && credentials.has_refresh_token == Some(true)
    {
        return "unsupported_refresh_source";
    }
    if credentials.has_oauth_token() {
        "login_required"
    } else {
        "logged_out"
    }
}

async fn network_check(
    global: &GlobalArgs,
    args: &DoctorArgs,
    mode: AuthMode,
) -> Result<DoctorCheck, CliError> {
    // [Review Fix #2] Network diagnostics share the already-selected mode and
    // therefore cannot switch to credential-bearing AK/SK profile loading.
    let profile = build_runtime_profile(global, mode)?;
    let has_explicit_endpoint = profile
        .endpoint
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    let resolved = resolve_endpoint_and_region(profile.endpoint.clone(), profile.region.clone());
    let endpoint = resolved.as_ref().ok().map(|(endpoint, _)| endpoint.clone());

    // Without --live-network, retain offline-safe behavior so `ve-adrive-cli doctor`
    // works in air-gapped environments.
    if !args.live_network {
        return Ok(DoctorCheck {
            name: "network",
            status: if resolved.is_ok() {
                "passed"
            } else {
                "warning"
            },
            message: resolved
                .as_ref()
                .map(|_| "network endpoint is explicitly configured".to_string())
                .unwrap_or_else(|err| err.to_string()),
            fix_command: resolved
                .is_err()
                .then(|| resource_config_fix_command(has_explicit_endpoint).to_string()),
            details: json!({
                "endpoint": endpoint,
                "has_explicit_endpoint": has_explicit_endpoint,
                "has_region": profile.region.is_some(),
                "live_check": false,
                "hint": "pass --live-network to perform a real probe",
            }),
        });
    }

    // Live probe: HTTPS HEAD against the configured endpoint with a tight
    // timeout. Even a 403 proves the host is reachable.
    let Some(target) = endpoint else {
        return Ok(DoctorCheck {
            name: "network",
            status: "warning",
            message: resolved
                .err()
                .map(|err| err.to_string())
                .unwrap_or_else(|| "no endpoint configured; cannot probe".to_string()),
            fix_command: Some(resource_config_fix_command(has_explicit_endpoint).to_string()),
            details: json!({ "live_check": true, "skipped": true }),
        });
    };

    let url = if target.starts_with("http://") || target.starts_with("https://") {
        target.clone()
    } else {
        format!("https://{}", target)
    };
    let timeout_dur = std::time::Duration::from_millis(args.network_timeout_ms);
    let client = match reqwest::Client::builder()
        .user_agent(storage_user_agent())
        .timeout(timeout_dur)
        .build()
    {
        Ok(c) => c,
        Err(err) => {
            return Ok(DoctorCheck {
                name: "network",
                status: "failed",
                message: format!("failed to build HTTP client: {err}"),
                fix_command: None,
                details: json!({ "live_check": true, "url": url }),
            });
        }
    };

    let started = std::time::Instant::now();
    let probe = client.head(&url).send().await;
    let latency_ms = started.elapsed().as_millis() as u64;

    match probe {
        Ok(resp) => {
            let status_code = resp.status();
            let outcome = if status_code.is_server_error() {
                "warning"
            } else {
                "passed"
            };
            Ok(DoctorCheck {
                name: "network",
                status: outcome,
                message: format!(
                    "reached {} in {}ms (HTTP {})",
                    url,
                    latency_ms,
                    status_code.as_u16()
                ),
                fix_command: None,
                details: json!({
                    "live_check": true,
                    "url": url,
                    "http_status": status_code.as_u16(),
                    "latency_ms": latency_ms,
                }),
            })
        }
        Err(err) => Ok(DoctorCheck {
            name: "network",
            status: "failed",
            message: format!("probe failed after {}ms: {}", latency_ms, err),
            fix_command: None,
            details: json!({
                "live_check": true,
                "url": url,
                "latency_ms": latency_ms,
                "error": err.to_string(),
                "is_timeout": err.is_timeout(),
                "is_connect": err.is_connect(),
            }),
        }),
    }
}

fn resource_config_fix_command(has_endpoint: bool) -> &'static str {
    if has_endpoint {
        "ve-adrive config set region <region>"
    } else {
        "ve-adrive config set endpoint <endpoint>"
    }
}

fn registry_check() -> DoctorCheck {
    DoctorCheck {
        name: "registry",
        status: "passed",
        message: "capability registry is available".to_string(),
        fix_command: None,
        details: json!({
            "capabilities": capabilities().len(),
            "domains": command_domains(),
        }),
    }
}

fn principles_check() -> DoctorCheck {
    let rows = capabilities();
    let missing_domain: Vec<&str> = rows
        .iter()
        .filter(|row| row.domain.is_empty())
        .map(|row| row.command)
        .collect();
    let destructive_without_force: Vec<&str> = rows
        .iter()
        .filter(|row| row.destructive && !row.supports_force)
        .map(|row| row.command)
        .collect();
    let skill_domains = skill_definitions()
        .iter()
        .map(|skill| skill.domain.clone())
        .collect::<std::collections::BTreeSet<_>>();
    // Skill domains use the business taxonomy (adrive-transfer/-shared/-admin),
    // so compare coverage against `business_domains()` rather than command roots.
    let uncovered_skill_domains: Vec<&str> = business_domains()
        .into_iter()
        .filter(|domain| !skill_domains.contains(*domain))
        .collect();
    let exposed_low_level: Vec<&str> = rows
        .iter()
        .filter(|row| normalize_facet(row.layer) == "low_level")
        .map(|row| row.command)
        .collect();
    let passed = missing_domain.is_empty()
        && destructive_without_force.is_empty()
        && uncovered_skill_domains.is_empty()
        && exposed_low_level.is_empty();
    DoctorCheck {
        name: "principles",
        status: if passed { "passed" } else { "failed" },
        message: if passed {
            "six-principle invariants are upheld by the ADrive registry".to_string()
        } else {
            "six-principle invariants failed".to_string()
        },
        fix_command: None,
        details: json!({
            "capabilities": rows.len(),
            "skill_definitions": skill_domains.len(),
            "missing_domain": missing_domain,
            "destructive_force_violations": destructive_without_force,
            "exposed_unimplemented_low_level": exposed_low_level,
            "skill_domains": skill_domains.into_iter().collect::<Vec<_>>(),
            "uncovered_skill_domains": uncovered_skill_domains,
            "principle_keys": [
                "discovery",
                "understanding",
                "safe_execution",
                "controlled_output",
                "deterministic_errors",
                "agent_ecosystem"
            ],
        }),
    }
}

fn mcp_check() -> DoctorCheck {
    DoctorCheck {
        name: "mcp",
        status: "passed",
        message: "MCP runtime is available for stdio and SSE transports".to_string(),
        fix_command: None,
        details: json!({
            "capabilities": capabilities().len(),
            "runtime": "available",
            "stdio_status": "available",
            "sse_status": "available",
            "default_bind": "127.0.0.1",
        }),
    }
}

fn completion_check() -> DoctorCheck {
    DoctorCheck {
        name: "completion",
        status: "passed",
        message: "completion generation is registry-backed".to_string(),
        fix_command: None,
        details: json!({ "shells": ["bash", "zsh", "fish", "powershell"] }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn missing_chinese(context: &str, source: &str) -> Option<String> {
        let chinese = adrive_metadata_translation_zh(source);
        let is_missing = chinese.is_none_or(|value| {
            value == source || value.contains("原始英文说明") || value.contains(source)
        });
        is_missing.then(|| format!("command/parameter={context}, source={source:?}"))
    }

    fn schema_descriptions(schema: &Value) -> Vec<(String, String)> {
        let mut descriptions = Vec::new();
        let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
            return descriptions;
        };
        for (name, property) in properties {
            if let Some(description) = property.get("description").and_then(Value::as_str) {
                descriptions.push((name.clone(), description.to_string()));
            }
        }
        descriptions
    }

    fn collect_human_metadata(
        value: &Value,
        path: &str,
        context: ADriveMetadataContext,
        prose: &mut Vec<(String, String)>,
    ) {
        match value {
            Value::String(source) if context == ADriveMetadataContext::HumanText => {
                prose.push((path.to_string(), source.clone()));
            }
            Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    let item_context = adrive_array_item_context(context, item);
                    collect_human_metadata(item, &format!("{path}[{index}]"), item_context, prose);
                }
            }
            Value::Object(map) => {
                for (key, child) in map {
                    let child_path = format!("{path}.{key}");
                    let child_context = adrive_child_metadata_context(context, key, child);
                    collect_human_metadata(child, &child_path, child_context, prose);
                }
            }
            _ => {}
        }
    }

    fn describe_human_prose(document: &Value) -> Vec<(String, String)> {
        let mut prose = Vec::new();
        collect_human_metadata(document, "$", ADriveMetadataContext::Structured, &mut prose);
        prose
    }

    fn missing_describe_translations(command: &str, document: &Value) -> BTreeSet<String> {
        describe_human_prose(document)
            .into_iter()
            .filter_map(|(field, source)| missing_chinese(&format!("{command} {field}"), &source))
            .collect()
    }

    fn external_describe_documents() -> Vec<(&'static str, Value)> {
        vec![
            (
                "ve-adrive",
                json!({
                    "description": "ADrive CLI high-level file operations and agent utilities",
                    "groups": [
                        {"description": "File management operations with adrive:// URI and flag targets"},
                        {"description": "Discovery, configuration, diagnostics, completion, skill, API passthrough, and MCP utilities"},
                    ],
                }),
            ),
            (
                "ve-adrive api <group> <action>",
                json!({"description": "Guarded ADrive utility API planning; direct raw execution is not implemented yet"}),
            ),
        ]
    }

    #[test]
    fn chinese_catalog_recursively_covers_every_adrive_describe_path() {
        let mut documents = external_describe_documents();
        documents.push(("ve-adrive config", describe_config_group()));
        for command in [
            "ve-adrive config init",
            "ve-adrive config show",
            "ve-adrive config set",
        ] {
            let document = describe_adrive_command_metadata(command)
                .unwrap_or_else(|| panic!("missing Describe metadata for {command}"));
            documents.push((command, document));
        }
        let mut missing = BTreeSet::new();
        for (command, document) in documents {
            missing.extend(missing_describe_translations(command, &document));
        }
        assert!(
            missing.is_empty(),
            "missing recursive Describe translations:\n{}",
            missing.into_iter().collect::<Vec<_>>().join("\n")
        );
    }

    #[test]
    fn chinese_owner_catalog_covers_every_capability_and_parameter() {
        let mut missing = BTreeSet::new();
        for row in capabilities() {
            missing.extend(missing_chinese(row.command, row.description));
            for parameter in row.parameters {
                let context = format!("{} --{}", row.command, parameter.name);
                missing.extend(missing_chinese(&context, parameter.description));
            }
        }
        assert!(
            missing.is_empty(),
            "missing Chinese metadata:\n{}",
            missing.into_iter().collect::<Vec<_>>().join("\n")
        );
    }

    #[test]
    fn chinese_owner_catalog_has_unique_english_sources() {
        let mut sources = BTreeSet::new();
        let duplicates = ADRIVE_METADATA_TRANSLATIONS_ZH
            .iter()
            .filter_map(|(english, _)| (!sources.insert(*english)).then_some(*english))
            .collect::<Vec<_>>();

        assert!(
            duplicates.is_empty(),
            "duplicate ADrive Chinese owner sources: {duplicates:?}"
        );
    }

    #[test]
    fn chinese_skill_definitions_localize_all_human_descriptions() {
        let english = skill_definitions_for_language(DocumentationLanguage::En);
        let chinese = skill_definitions_for_language(DocumentationLanguage::Zh);
        for source_skill in english {
            let localized = chinese
                .iter()
                .find(|skill| skill.command == source_skill.command)
                .expect("Chinese skill matches English command");
            assert_ne!(
                localized.description, source_skill.description,
                "command={}",
                source_skill.command
            );
            assert!(
                !localized.description.contains("原始英文说明"),
                "command={}",
                source_skill.command
            );
            let source_value =
                serde_json::to_value(&source_skill).expect("serialize English Skill");
            let missing = missing_describe_translations(&source_skill.command, &source_value);
            assert!(
                missing.is_empty(),
                "missing recursive Skill translations:\n{}",
                missing.into_iter().collect::<Vec<_>>().join("\n")
            );
            let localized_schema = schema_descriptions(&localized.input_schema);
            for (parameter, source) in schema_descriptions(&source_skill.input_schema) {
                let (_, translated) = localized_schema
                    .iter()
                    .find(|(name, _)| name == &parameter)
                    .expect("Chinese schema preserves parameter");
                assert_ne!(
                    translated, &source,
                    "command={}, parameter={parameter}, source={source:?}",
                    source_skill.command
                );
                assert!(
                    !translated.contains("原始英文说明"),
                    "command={}, parameter={parameter}",
                    source_skill.command
                );
            }
        }
    }

    #[test]
    fn chinese_catalog_localizes_every_high_level_describe_document() {
        let mut missing = BTreeSet::new();
        for row in capabilities()
            .iter()
            .filter(|row| row.layer == "high-level")
        {
            let description =
                crate::handler::high_level::describe_high_level_command_path(row.command)
                    .expect("high-level registry command has Describe metadata");
            let document = serde_json::to_value(description).expect("serialize Describe");
            missing.extend(missing_describe_translations(row.command, &document));
        }
        assert!(
            missing.is_empty(),
            "missing high-level Describe translations:\n{}",
            missing.into_iter().collect::<Vec<_>>().join("\n")
        );
    }

    #[test]
    fn chinese_catalog_localizes_every_utility_describe_document() {
        let mut missing = BTreeSet::new();
        for command in [
            "ve-adrive capabilities",
            "ve-adrive api",
            "ve-adrive config",
            "ve-adrive completion",
            "ve-adrive serve",
            "ve-adrive skill list",
            "ve-adrive skill export",
            "ve-adrive doctor",
            "ve-adrive auth",
        ] {
            let document =
                describe_adrive_command_metadata(command).expect("utility Describe metadata");
            missing.extend(missing_describe_translations(command, &document));
        }
        assert!(
            missing.is_empty(),
            "missing utility Describe translations:\n{}",
            missing.into_iter().collect::<Vec<_>>().join("\n")
        );
        let mut auth = describe_adrive_command_metadata("ve-adrive auth").expect("auth Describe");
        let precedence = auth["scenario_routing"]["mode_resolution"].clone();
        localize_adrive_auth_documentation_zh(&mut auth);
        assert_eq!(auth["scenario_routing"]["mode_resolution"], precedence);
    }

    #[test]
    fn chinese_localizer_preserves_catalog_collisions_in_machine_fields() {
        let mut document = json!({
            "description": "Configuration management",
            "command": "Configuration management",
            "name": "Sort field",
            "type": "Configuration management",
            "enum": ["Configuration management"],
            "examples": ["Configuration management"],
            "scenario_routing": {
                "operation": "Configuration management",
                "command": "Configuration management",
                "mode_resolution": "Configuration management",
            },
            "recovery": {
                "description": "Configuration management",
                "command": "Configuration management",
                "code": "Sort field",
            },
        });

        localize_adrive_auth_documentation_zh(&mut document);

        assert_eq!(document["description"], "配置管理");
        assert_eq!(document["scenario_routing"]["operation"], "配置管理");
        assert_eq!(document["recovery"]["description"], "配置管理");
        for (pointer, expected) in [
            ("/command", "Configuration management"),
            ("/name", "Sort field"),
            ("/type", "Configuration management"),
            ("/enum/0", "Configuration management"),
            ("/examples/0", "Configuration management"),
        ] {
            assert_eq!(document.pointer(pointer).unwrap(), expected);
        }
        assert_eq!(
            document["scenario_routing"]["mode_resolution"],
            "Configuration management"
        );
        assert_eq!(
            document["scenario_routing"]["command"],
            "Configuration management"
        );
        assert_eq!(document["recovery"]["command"], "Configuration management");
        assert_eq!(document["recovery"]["code"], "Sort field");
    }

    #[test]
    fn unified_auth_doctor_check_exposes_only_approved_metadata() {
        let resolved = crate::domain::auth::ResolvedAuthMode {
            mode: AuthMode::Unified,
            source: crate::domain::auth::AuthModeSource::Config,
        };
        let inspection = crate::handler::common::UnifiedCredentialInspection {
            provider_name: Some("safe-provider".to_string()),
            has_session_token: true,
            ready: true,
            sdk_code: None,
        };

        let check = unified_auth_doctor_check("selected", resolved, &inspection);
        let keys = check
            .details
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();

        assert_eq!(check.status, "passed");
        assert_eq!(check.fix_command, None);
        assert_eq!(
            keys,
            std::collections::BTreeSet::from([
                "has_session_token",
                "mode",
                "profile",
                "provider_name",
                "ready",
                "source",
            ])
        );
    }

    #[test]
    fn unified_auth_prompts_external_credential_management() {
        let credentials = crate::domain::auth::CredentialAvailability {
            has_access_key: None,
            has_secret_key: None,
            has_security_token: None,
            access_key_source: None,
            secret_key_source: None,
            security_token_source: None,
            has_access_token: None,
            has_refresh_token: None,
            access_token_expiry: None,
            expires_at: None,
            scope: None,
            instance_id: None,
            ready: false,
            oauth_service_integration: "not_applicable",
            credential_source: "external",
        };

        assert_eq!(
            auth_fix_command(AuthMode::Unified, &credentials, false, false).as_deref(),
            Some("ve login")
        );
        assert_eq!(
            auth_check_message(AuthMode::Unified, &credentials, false, false),
            "ADrive unified credentials are managed externally"
        );
    }

    #[tokio::test]
    async fn unified_full_doctor_checks_ignore_unselected_local_secrets() {
        let directory = std::env::temp_dir().join(format!(
            "ve-adrive-unified-doctor-isolation-{}-{}",
            std::process::id(),
            ulid::Ulid::new()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let config_path = directory.join("config.toml");
        let credentials_path = directory.join("credentials.toml");
        std::fs::write(
            &config_path,
            r#"[selected.adrive]
auth_mode = "unified"
endpoint = "https://resource.example.com"
region = "cn-test"
access_key_id = "ENC:not-valid"
secret_access_key = "ENC:not-valid"
"#,
        )
        .unwrap();
        std::fs::write(&credentials_path, "not valid toml = [").unwrap();
        let global = GlobalArgs {
            profile: "selected".to_string(),
            config_path: Some(config_path),
            credentials_path: Some(credentials_path),
            ..GlobalArgs::default()
        };
        let args = DoctorArgs {
            check: None,
            live_network: false,
            network_timeout_ms: 5_000,
        };

        let config = config_check(&global, AuthMode::Unified).unwrap();
        let network = network_check(&global, &args, AuthMode::Unified)
            .await
            .unwrap();

        assert_eq!(config.status, "passed");
        assert_eq!(network.status, "passed");
        assert!(!directory.join(".key").exists());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn registry_only_doctor_check_remains_independent_of_auth_config() {
        let directory = std::env::temp_dir().join(format!(
            "ve-adrive-registry-doctor-isolation-{}-{}",
            std::process::id(),
            ulid::Ulid::new()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let config_path = directory.join("config.toml");
        std::fs::write(&config_path, "not valid toml = [").unwrap();
        let global = GlobalArgs {
            config_path: Some(config_path),
            ..GlobalArgs::default()
        };
        let auth = ADriveAuthArgs { auth_mode: None };
        let args = DoctorArgs {
            check: Some("registry".to_string()),
            live_network: false,
            network_timeout_ms: 5_000,
        };

        let checks = build_doctor_checks(&global, &auth, &args)
            .await
            .expect("registry diagnostics must not depend on auth config");

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].name, "registry");
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn full_doctor_reports_auth_config_errors_without_aborting_other_checks() {
        let directory = std::env::temp_dir().join(format!(
            "ve-adrive-full-doctor-error-isolation-{}-{}",
            std::process::id(),
            ulid::Ulid::new()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let config_path = directory.join("config.toml");
        std::fs::write(&config_path, "not valid toml = [").unwrap();
        let global = GlobalArgs {
            config_path: Some(config_path),
            ..GlobalArgs::default()
        };
        let auth = ADriveAuthArgs { auth_mode: None };
        let args = DoctorArgs {
            check: None,
            live_network: false,
            network_timeout_ms: 5_000,
        };

        let checks = build_doctor_checks(&global, &auth, &args)
            .await
            .expect("full doctor must retain independent diagnostics");

        for name in ["config", "auth", "network"] {
            assert_eq!(
                checks
                    .iter()
                    .find(|check| check.name == name)
                    .unwrap()
                    .status,
                "failed"
            );
        }
        assert!(checks
            .iter()
            .any(|check| check.name == "registry" && check.status == "passed"));
        assert!(checks
            .iter()
            .any(|check| check.name == "mcp" && check.status == "passed"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn mcp_serve_execute_is_rejected() {
        let global = GlobalArgs::default();
        let result = mcp_invoke_tool(
            &global,
            "ve_adrive_serve".to_string(),
            json!({"execute": true, "mcp": true}),
        )
        .await;

        assert!(matches!(
            result,
            Err(CliError::ValidationError(message))
                if message.contains("only supports planning")
        ));
    }

    #[test]
    fn auth_describe_metadata_exposes_unified_contract() {
        let description = describe_adrive_command_metadata("ve-adrive auth")
            .expect("auth metadata must be registered");
        let rendered = serde_json::to_string(&description).unwrap();

        for expected in [
            "--auth-mode <MODE>",
            "ADRIVE_AUTH_MODE",
            "same-name external profile",
            "ve login",
            "ve logout",
        ] {
            assert!(rendered.contains(expected), "describe missing {expected}");
        }
        assert_eq!(description["supports_dry_run"], json!(true));
    }
}
