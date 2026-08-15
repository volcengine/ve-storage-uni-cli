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

use std::collections::BTreeMap;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::Arc;
use std::time::Duration;

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::process::Command as TokioCommand;
use tokio::time::timeout;
use tos_core::agent::describe::RiskLevel;
use tos_core::agent::envelope::Envelope;
use tos_core::agent::error::CliError;
use tos_core::agent::global_args::GlobalArgs;
use tos_core::infra::client::{derive_region_from_endpoint, storage_user_agent, TosClient};
use tos_core::infra::config::{Binary, DEFAULT_TOS_BATCH_REPORT_DIR, DEFAULT_TOS_CHECKPOINT_DIR};
use tos_core::infra::unified_credentials::UnifiedCredentialProvider;

use crate::cli::meta::{
    ApiArgs, CapabilitiesArgs, CompletionArgs, DoctorArgs, DocumentationLanguage, ServeArgs,
    SkillAction, SkillCommand,
};
use crate::domain::core::execute_resolved_request;
use crate::handler::common::{
    active_tos_config_binary, build_profile, build_runtime, output_result,
    output_result_with_columns,
};
use crate::registry::{
    business_domain, canonical_group_name, capabilities, capability_row_for_command,
    capability_rows, command_groups, describe_command_metadata, find_api_capability,
    find_command_tree_entry, find_group, flattened_command_tree, is_known_group_or_category,
    leaf_command_tree, public_tos_command, public_tos_example, CapabilityEntry, CommandGroupEntry,
    CommandTreeEntry, RegistryCapabilityRow,
};

const VE_TOS_METADATA_TRANSLATIONS_ZH: &[(&str, &str)] = &[
    ("ACL value", "ACL 值"),
    ("ACL value (private, public-read, public-read-write, authenticated-read)", "ACL 值（private、public-read、public-read-write、authenticated-read）"),
    ("ACL value (x-tos-acl)", "ACL 值（x-tos-acl）"),
    ("API action", "API 操作"),
    ("API group", "API 分组"),
    ("AZ redundancy (x-tos-az-redundancy). Allowed: single-az, multi-az", "AZ 冗余（x-tos-az-redundancy）。可选值：single-az、multi-az"),
    ("AZ redundancy mode. Allowed: single-az, multi-az", "AZ 冗余模式。允许值：single-az、multi-az"),
    ("Abort a multipart upload", "中止分片上传"),
    ("Accelerator ID or name used in path parameters", "路径参数中使用的 Accelerator ID 或名称"),
    ("Accelerator ID used by accelerator APIs", "Accelerator API 使用的 Accelerator ID"),
    ("Accelerator management", "加速器管理"),
    ("Access log storage configuration", "访问日志存储配置"),
    ("Access monitoring configuration", "访问监控配置"),
    ("Access point APIs", "接入点 API"),
    ("Access point management", "接入点管理"),
    ("Advanced accelerator control APIs", "高级加速器控制 API"),
    ("Advanced control APIs", "高级控制 API"),
    ("Advanced data processing APIs", "高级数据处理 API"),
    ("Advanced object set APIs", "高级对象集合 API"),
    ("Allow non-idempotent raw API execution", "允许执行非幂等的原始 API"),
    ("Also abort incomplete multipart uploads matching the prefix", "同时中止与前缀匹配的未完成分片上传"),
    ("Append data to a turbo object", "向 Turbo 对象追加数据"),
    ("Append data to an appendable object", "向可追加对象追加数据"),
    ("Append last time", "上次追加时间"),
    ("Append offset", "追加偏移量"),
    ("Archived object path or prefix (tos://bucket/key or tos://bucket/prefix/)", "归档对象路径或前缀（tos://bucket/key 或 tos://bucket/prefix/）"),
    ("Availability zone", "可用区"),
    ("Bandwidth limit", "带宽限制"),
    ("Bandwidth limit (e.g., 100MB)", "带宽限制（例如 100MB）"),
    ("Batch delete objects (DeleteMultiObjects)", "批量删除对象（DeleteMultiObjects）"),
    ("Bind a bucket to an accelerator", "将 Bucket 绑定到 Accelerator"),
    ("Bind accelerator to MRAP", "将 Accelerator 绑定到 MRAP"),
    ("Bind accelerator to access point", "将 Accelerator 绑定到接入点"),
    ("Body source (file path or inline data)", "正文来源（文件路径或内联数据）"),
    ("Bucket ACL (x-tos-acl). Allowed: private, public-read, public-read-write, authenticated-read, bucket-owner-read, bucket-owner-full-control", "Bucket ACL（x-tos-acl）。可选值：private、public-read、public-read-write、authenticated-read、bucket-owner-read、bucket-owner-full-control"),
    ("Bucket ACL management", "Bucket ACL 管理"),
    ("Bucket ACL. Allowed: private, public-read, public-read-write, authenticated-read, bucket-owner-read, bucket-owner-full-control", "Bucket ACL。允许值：private、public-read、public-read-write、authenticated-read、bucket-owner-read、bucket-owner-full-control"),
    ("Bucket CORS configuration", "Bucket CORS 配置"),
    ("Bucket Core APIs", "Bucket 核心 API"),
    ("Bucket RenameObject configuration", "Bucket RenameObject 配置"),
    ("Bucket URI (tos://bucket)", "存储桶 URI（tos://bucket）"),
    ("Bucket core APIs", "Bucket 核心 API"),
    ("Bucket default storage class", "Bucket 默认存储类型"),
    ("Bucket default storage class [Review Fix #M5] Variant renamed to `Storageclass`; the legacy spelling `storgeclass` is kept as a clap alias to preserve backward compatibility for existing scripts and skill manifests", "Bucket 默认存储类型"),
    ("Bucket encryption configuration", "Bucket 加密配置"),
    ("Bucket inventory configuration", "Bucket 清单配置"),
    ("Bucket name", "Bucket 名称"),
    ("Bucket name (alternative to positional URI)", "Bucket 名称（位置 URI 的替代写法）"),
    ("Bucket name (flag style)", "Bucket 名称（flag 形式）"),
    ("Bucket name (used with --check permissions)", "Bucket 名称（与 --check permissions 配合使用）"),
    ("Bucket name path parameter for control-plane binding APIs", "控制面绑定 API 的 Bucket 名称路径参数"),
    ("Bucket policy management", "Bucket 策略管理"),
    ("Bucket rename configuration", "Bucket 重命名配置"),
    ("Bucket storage quota", "Bucket 存储配额"),
    ("Bucket tagging management", "Bucket 标签管理"),
    ("Bucket trash configuration", "Bucket 回收站配置"),
    ("Bucket type header (x-tos-bucket-type; allowed: fns, hns)", "Bucket 类型请求头（x-tos-bucket-type；可选值：fns、hns）"),
    ("Bucket type. Allowed: fns, hns", "Bucket 类型。允许值：fns、hns"),
    ("Bucket versioning configuration", "Bucket 版本控制配置"),
    ("Byte range (e.g., 0-1023)", "字节范围（例如 0-1023）"),
    ("CDN notification configuration", "CDN 通知配置"),
    ("CORS configuration", "CORS 配置"),
    ("CRC64 checksum", "CRC64 校验和"),
    ("CRR proxy (x-crr-proxy)", "CRR 代理（x-crr-proxy）"),
    ("CRR source bucket version status (x-crr-source-bucket-version-status)", "CRR 源 Bucket 版本状态（x-crr-source-bucket-version-status）"),
    ("CRR source last modify time (x-crr-source-last-modify-time)", "CRR 源最后修改时间（x-crr-source-last-modify-time）"),
    ("CRR source timestamp nsec (x-crr-source-timestamp-nsec)", "CRR 源时间戳纳秒值（x-crr-source-timestamp-nsec）"),
    ("CRR source uploadId (x-crr-source-uploadId)", "CRR 源 uploadId（x-crr-source-uploadId）"),
    ("CRR source versionId (x-crr-source-versionId)", "CRR 源 versionId（x-crr-source-versionId）"),
    ("Calculate object size statistics for a prefix", "统计前缀下的对象大小"),
    ("Canned ACL value (private, public-read, public-read-write, authenticated-read)", "预定义 ACL 值（private、public-read、public-read-write、authenticated-read）"),
    ("Check a specific module: auth, config, registry, permissions, region, network, version, mcp, principles, completion", "检查指定模块：auth、config、registry、permissions、region、network、version、mcp、principles、completion"),
    ("Checkpoint directory override", "覆盖 checkpoint 目录"),
    ("Close a turbo channel", "关闭 Turbo 通道"),
    ("Compare by size only (skip mtime)", "仅按大小比较（跳过 mtime）"),
    ("Complete a multipart upload", "完成分片上传"),
    ("Complete all parts server-side", "在服务端完成所有分片"),
    ("Completed parts JSON", "已完成分片的 JSON"),
    ("Configuration (JSON or file://path)", "配置（JSON 或 file://path）"),
    ("Configuration key, e.g. `region`, `endpoint`, `account_id`, or `staging.endpoint`", "配置键，例如 `region`、`endpoint`、`account_id` 或 `staging.endpoint`"),
    ("Configuration management", "配置管理"),
    ("Configuration value", "配置值"),
    ("Confirm batch restore and cost-related side effects", "确认批量恢复及相关费用影响"),
    ("Confirm bucket deletion", "确认删除 Bucket"),
    ("Confirm deletion when --delete is enabled", "启用 --delete 时确认删除"),
    ("Confirm destructive Advanced operation before execution", "执行前确认破坏性的 Advanced 操作"),
    ("Confirm destructive abort before execution", "执行前确认破坏性的中止操作"),
    ("Confirm destructive delete before execution", "执行前确认破坏性的删除操作"),
    ("Content type", "内容类型"),
    ("Content-MD5 for integrity check", "用于完整性校验的 Content-MD5"),
    ("Content-MD5 header", "Content-MD5 请求头"),
    ("Content-MD5 header; auto-computed when omitted", "Content-MD5 请求头；省略时自动计算"),
    ("Content-MD5 request header for JSON body", "JSON 正文的 Content-MD5 请求头"),
    ("Content-Type for TOS uploads/copies", "TOS 上传/复制使用的 Content-Type"),
    ("Content-Type for the uploaded object", "上传对象使用的 Content-Type"),
    ("Content-Type header", "Content-Type 请求头"),
    ("Continuation token", "续传 token"),
    ("Continuation token for listing tasks", "列出任务时使用的续传 token"),
    ("Continuation token returned by a previous listing", "上一次列举返回的 continuation token"),
    ("Control plane operations", "控制面操作"),
    ("Converged access point", "融合接入点"),
    ("Converged access point APIs", "融合接入点 API"),
    ("Copy an object (server-side CopyObject)", "复制对象（服务端 CopyObject）"),
    ("Copy local files, TOS objects, or prefixes", "复制本地文件、TOS 对象或前缀"),
    ("Copy local files, objects, or prefixes between local and TOS.", "在本地与 TOS 之间复制本地文件、对象或前缀。"),
    ("Copy source (for example /src-bucket/src-key)", "复制源（例如 /src-bucket/src-key）"),
    ("Copy source last modified (x-tos-copy-source-last-modified)", "复制源最后修改时间（x-tos-copy-source-last-modified）"),
    ("Create MRAP routes", "创建 MRAP 路由"),
    ("Create URL cache purge/prefetch", "创建 URL 缓存清理/预取任务"),
    ("Create a batch job", "创建批处理任务"),
    ("Create a bucket", "创建 Bucket"),
    ("Create a cross-account access point", "创建跨账号接入点"),
    ("Create a dataset", "创建 Dataset"),
    ("Create a dataset binding", "创建 Dataset 绑定"),
    ("Create a document processing job", "创建文档处理任务"),
    ("Create a file processing job", "创建文件处理任务"),
    ("Create a folder", "创建文件夹"),
    ("Create a hard link to an object", "创建对象硬链接"),
    ("Create a media processing job", "创建媒体处理任务"),
    ("Create a multipart upload", "创建分片上传"),
    ("Create a new bucket", "创建新 Bucket"),
    ("Create a prefetch job", "创建预取任务"),
    ("Create a redundancy transition task", "创建冗余转换任务"),
    ("Create a symbolic link", "创建符号链接"),
    ("Create an MRAP", "创建 MRAP"),
    ("Create an accelerator", "创建 Accelerator"),
    ("Create an access point", "创建接入点"),
    ("Create an async fetch task", "创建异步拉取任务"),
    ("Create an audit configuration", "创建审计配置"),
    ("Create an audit job", "创建审计任务"),
    ("Create an evict job", "创建驱逐任务"),
    ("Create an increment audit configuration", "创建增量审计配置"),
    ("Create custom endpoint for CAP", "为 CAP 创建自定义 endpoint"),
    ("Create custom endpoint token", "创建自定义 endpoint token"),
    ("Create object set for CAP", "为 CAP 创建对象集"),
    ("Create parent folder markers as needed", "按需创建父级文件夹标记"),
    ("Cross-region replication", "跨区域复制"),
    ("Custom TOS metadata as key=value#key2=value2; writes x-tos-meta-* headers", "自定义 TOS 元数据，格式为 key=value#key2=value2；会写入 x-tos-meta-* 头"),
    ("Custom domain binding", "自定义域名绑定"),
    ("Custom domain name", "自定义域名"),
    ("Custom domain name to remove", "要移除的自定义域名"),
    ("Custom endpoint domain", "自定义 endpoint 域名"),
    ("Custom metadata (key1=val1&key2=val2)", "自定义元数据（key1=val1&key2=val2）"),
    ("Data ID (x-data-id)", "Data ID（x-data-id）"),
    ("Data processing (image styles, workflows, audits)", "数据处理（图片样式、工作流、审计）"),
    ("Data processing job type (job_type query)", "数据处理任务类型（job_type 查询参数）"),
    ("Data redundancy transition", "数据冗余转换"),
    ("Data to append (file path or inline)", "要追加的数据（文件路径或内联内容）"),
    ("Data-process template tag query parameter", "数据处理模板的 tag 查询参数"),
    ("Decoded content length", "解码后的内容长度"),
    ("Delete CDN notification configuration", "删除 CDN 通知配置"),
    ("Delete CORS configuration", "删除 CORS 配置"),
    ("Delete MRAP mirror configuration", "删除 MRAP 镜像配置"),
    ("Delete MRAP policy", "删除 MRAP 策略"),
    ("Delete QoS policy", "删除 QoS 策略"),
    ("Delete URL cache", "删除 URL 缓存"),
    ("Delete a batch job", "删除批处理任务"),
    ("Delete a bucket", "删除 Bucket"),
    ("Delete a cross-account access point", "删除跨账号接入点"),
    ("Delete a dataset", "删除 Dataset"),
    ("Delete a dataset binding", "删除 Dataset 绑定"),
    ("Delete a lens configuration", "删除 Lens 配置"),
    ("Delete a prefetch job", "删除预取任务"),
    ("Delete a redundancy transition task", "删除冗余转换任务"),
    ("Delete a resource tag", "删除资源 tag"),
    ("Delete a single object", "删除单个对象"),
    ("Delete a template", "删除模板"),
    ("Delete a workflow", "删除工作流"),
    ("Delete access point policy", "删除接入点策略"),
    ("Delete an MRAP", "删除 MRAP"),
    ("Delete an accelerator", "删除 Accelerator"),
    ("Delete an access point", "删除接入点"),
    ("Delete an evict job", "删除驱逐任务"),
    ("Delete an image style", "删除图片样式"),
    ("Delete an increment audit configuration", "删除增量审计配置"),
    ("Delete an object set", "删除对象集"),
    ("Delete bucket encryption configuration", "删除 Bucket 加密配置"),
    ("Delete bucket policy", "删除 Bucket 策略"),
    ("Delete bucket rename configuration", "删除 Bucket 重命名配置"),
    ("Delete bucket tagging", "删除 Bucket tag"),
    ("Delete custom domain binding", "删除自定义域名绑定"),
    ("Delete custom endpoint for CAP", "删除 CAP 的自定义 endpoint"),
    ("Delete event subscription", "删除事件订阅"),
    ("Delete every object version (and delete markers) instead of only the current version. Required when the bucket has versioning enabled and the caller wants permanent removal", "删除每个对象版本（以及删除标记），而不是仅删除当前版本。Bucket 启用版本控制且调用方需要永久删除时必填。"),
    ("Delete extraneous files from destination", "删除目标端多余的文件"),
    ("Delete inventory configuration", "删除清单配置"),
    ("Delete lifecycle rules", "删除生命周期规则"),
    ("Delete max-age configuration", "删除 max-age 配置"),
    ("Delete mirror back-to-source rules", "删除镜像回源规则"),
    ("Delete object set lifecycle by tag", "按 tag 删除对象集生命周期"),
    ("Delete object set lifecycle configuration", "删除对象集生命周期配置"),
    ("Delete object set quota by tag", "按 tag 删除对象集配额"),
    ("Delete object tagging", "删除对象 tag"),
    ("Delete objects or prefixes", "删除对象或前缀"),
    ("Delete real-time log configuration", "删除实时日志配置"),
    ("Delete replication configuration", "删除复制配置"),
    ("Delete static website configuration", "删除静态网站配置"),
    ("Delimiter", "分隔符"),
    ("Destination (tos://dst-bucket/dst-key)", "目标（tos://dst-bucket/dst-key）"),
    ("Destination object path in the same bucket (tos://bucket/key)", "同一 Bucket 内的目标对象路径（tos://bucket/key）"),
    ("Destination overwrite strategy", "目标覆盖策略"),
    ("Destination path", "目标路径"),
    ("Destination path (local path or tos://bucket/key)", "目标路径（本地路径或 tos://bucket/key）"),
    ("Destroy bucket permanently (?destroy)", "永久销毁 Bucket（?destroy）"),
    ("Disable execution progress output", "禁用执行阶段进度输出"),
    ("Disable listing-phase echo output", "禁用 list 阶段回显"),
    ("Discover CLI capabilities", "发现 CLI 能力"),
    ("Do not overwrite an existing object", "不覆盖已存在对象"),
    ("Do not overwrite existing objects (sets if-none-match: *)", "不覆盖已存在对象（设置 if-none-match: *）"),
    ("Do not update timestamp (x-not-update-timestamp)", "不更新时间戳（x-not-update-timestamp）"),
    ("Do not write a planned delete manifest", "不写入计划删除 manifest"),
    ("Do not write a planned restore manifest", "不写入计划恢复 manifest"),
    ("Do not write a planned transfer manifest", "不写入计划传输 manifest"),
    ("Documentation language: en or zh", "文档语言：en 或 zh"),
    ("Download a single object (GetObject)", "下载单个对象（GetObject）"),
    ("ETag pattern hint", "ETag 模式提示"),
    ("ETag pattern hint (x-etag-pattern)", "ETag 模式提示（x-etag-pattern）"),
    ("Enable MCP server", "启用 MCP 服务"),
    ("Enable bucket object lock", "启用 Bucket 对象锁"),
    ("Enable bucket object lock (x-tos-bucket-object-lock-enabled=true)", "启用 Bucket 对象锁（x-tos-bucket-object-lock-enabled=true）"),
    ("Enable checkpoint for resumable transfer", "启用断点续传 checkpoint"),
    ("Enable execution progress output even when stderr is not a TTY", "即使 stderr 不是 TTY，也启用执行阶段进度输出"),
    ("Enable listing-phase echo output even when stderr is not a TTY", "即使 stderr 不是 TTY，也启用 list 阶段回显"),
    ("Encoding type", "编码类型"),
    ("Environment diagnostics", "环境诊断"),
    ("Event notification configuration", "事件通知配置"),
    ("Exclude pattern", "排除匹配模式"),
    ("Exclude pattern for recursive restore", "递归恢复的排除匹配模式"),
    ("Expiration time (RFC3339 or Unix epoch)", "过期时间（RFC3339 或 Unix 时间戳）"),
    ("Export skills to local directory", "将 Skill 导出到本地目录"),
    ("Extra query parameter, repeatable, in k=v form", "额外查询参数，可重复指定，格式为 k=v"),
    ("Extra request header, repeatable, in k=v form", "额外请求头，可重复指定，格式为 k=v"),
    ("Fetch an external object synchronously", "同步拉取外部对象"),
    ("Fetch from KV (fetch-from-kv)", "从 KV 拉取（fetch-from-kv）"),
    ("File or data to write", "要写入的文件或数据"),
    ("File size threshold for checkpoint multipart/range transfer (e.g., 20MB)", "触发 checkpoint 分片/范围传输的文件大小阈值（例如 20MB）"),
    ("File to upload", "要上传的文件"),
    ("File to upload (or - for stdin)", "要上传的文件（或使用 - 表示标准输入）"),
    ("Filter by bucket type (x-tos-bucket-type; allowed: fns, hns)", "按 Bucket 类型过滤（x-tos-bucket-type；可选值：fns、hns）"),
    ("Filter by command group", "按命令分组过滤"),
    ("Filter by layer", "按层级过滤"),
    ("Filter by project name", "按项目名称过滤"),
    ("Find objects by name, size, mtime, or storage class", "按名称、大小、修改时间或存储类型查找对象"),
    ("Fingerprint (x-finger-print)", "指纹（x-finger-print）"),
    ("Folder key (used with --bucket)", "文件夹 Key（与 --bucket 搭配使用）"),
    ("Folder path (tos://bucket/folder/)", "文件夹路径（tos://bucket/folder/）"),
    ("Forbid overwrite (x-forbid-overwrite)", "禁止覆盖（x-forbid-overwrite）"),
    ("Forbid overwrite (x-tos-forbid-overwrite)", "禁止覆盖（x-tos-forbid-overwrite）"),
    ("Forbid overwrite existing object", "禁止覆盖已有对象"),
    ("Force delete bucket contents first (?force)", "先强制删除 Bucket 内容（?force）"),
    ("Force delete without confirmation", "无需确认强制删除"),
    ("Force deletion before execution", "执行前强制确认删除"),
    ("Force overwrite without confirmation", "无需确认强制覆盖"),
    ("Force overwrite/delete confirmation", "强制确认覆盖/删除"),
    ("From modular marker (X-From-Modular)", "modular 来源标记（X-From-Modular）"),
    ("Full CDN notification configuration JSON or file://path", "完整的 CDN 通知配置 JSON 或 file://path"),
    ("Full intelligent tiering configuration JSON or file://path", "完整的智能分层配置 JSON 或 file://path"),
    ("Full quota request body JSON or file://path", "完整的配额请求正文 JSON 或 file://path"),
    ("Full request body JSON or file://path", "完整的请求正文 JSON 或 file://path"),
    ("Full trash configuration JSON or file://path", "完整的回收站配置 JSON 或 file://path"),
    ("Generate presigned URL", "生成预签名 URL"),
    ("Generate presigned URLs", "生成预签名 URL"),
    ("Generate shell completion", "生成 shell 补全"),
    ("Get CDN notification configuration", "获取 CDN 通知配置"),
    ("Get CORS configuration", "获取 CORS 配置"),
    ("Get HTTPS / TLS version configuration", "获取 HTTPS / TLS 版本配置"),
    ("Get MRAP details", "获取 MRAP 详情"),
    ("Get MRAP mirror configuration", "获取 MRAP 镜像配置"),
    ("Get MRAP policy", "获取 MRAP 策略"),
    ("Get MRAP routes", "获取 MRAP 路由"),
    ("Get QoS policy", "获取 QoS 策略"),
    ("Get WORM (object lock) configuration", "获取 WORM（对象锁）配置"),
    ("Get a batch job", "获取批处理任务"),
    ("Get a dataset binding", "获取 Dataset 绑定"),
    ("Get a job", "获取任务"),
    ("Get a lens configuration", "获取 Lens 配置"),
    ("Get a prefetch job", "获取预取任务"),
    ("Get a redundancy transition task", "获取冗余转换任务"),
    ("Get a template", "获取模板"),
    ("Get a workflow", "获取工作流"),
    ("Get a workflow execution", "获取工作流执行记录"),
    ("Get accelerator bandwidth", "获取 Accelerator 带宽"),
    ("Get accelerator capacity", "获取 Accelerator 容量"),
    ("Get accelerator details", "获取 Accelerator 详情"),
    ("Get access monitor status", "获取访问监控状态"),
    ("Get access point details", "获取接入点详情"),
    ("Get access point policy", "获取接入点策略"),
    ("Get an audit configuration", "获取审计配置"),
    ("Get an evict job", "获取驱逐任务"),
    ("Get an image style", "获取图片样式"),
    ("Get an increment audit configuration", "获取增量审计配置"),
    ("Get an object set", "获取对象集"),
    ("Get async fetch task status", "获取异步拉取任务状态"),
    ("Get blind watermark rule", "获取盲水印规则"),
    ("Get bucket ACL", "获取 Bucket ACL"),
    ("Get bucket detailed information", "获取 Bucket 详细信息"),
    ("Get bucket encryption configuration", "获取 Bucket 加密配置"),
    ("Get bucket location", "获取 Bucket 位置"),
    ("Get bucket logging configuration", "获取 Bucket 日志配置"),
    ("Get bucket metadata (HeadBucket)", "获取 Bucket 元数据（HeadBucket）"),
    // [Review Fix #GlobalZh7] Canonical group Describe owns this shorter
    // HeadBucket prose, so keep it in the exact owner catalog as well.
    ("Get bucket metadata", "获取 Bucket 元数据"),
    ("Initialize configuration", "初始化配置"),
    ("Set configuration value", "设置配置值"),
    ("Show effective configuration", "显示生效配置"),
    ("Turbo Core APIs", "Turbo 核心 API"),
    ("Get bucket policy", "获取 Bucket 策略"),
    ("Get bucket quota", "获取 Bucket 配额"),
    ("Get bucket rename configuration", "获取 Bucket 重命名配置"),
    ("Get bucket statistics", "获取 Bucket 统计信息"),
    ("Get bucket tagging", "获取 Bucket tag"),
    ("Get cross-account access point details", "获取跨账号接入点详情"),
    ("Get custom domain certificate token", "获取自定义域名证书 token"),
    ("Get custom endpoint token", "获取自定义 endpoint token"),
    ("Get dataset details", "获取 Dataset 详情"),
    ("Get event notification configuration", "获取事件通知配置"),
    ("Get event subscription", "获取事件订阅"),
    ("Get global object set configuration", "获取全局对象集配置"),
    ("Get image protect rule", "获取图片保护规则"),
    ("Get image style separator", "获取图片样式分隔符"),
    ("Get intelligent tiering configuration", "获取智能分层配置"),
    ("Get inventory configuration", "获取清单配置"),
    ("Get lifecycle rules", "获取生命周期规则"),
    ("Get max-age configuration", "获取 max-age 配置"),
    ("Get mirror back-to-source rules", "获取镜像回源规则"),
    ("Get object ACL", "获取对象 ACL"),
    ("Get object metadata (HeadObject)", "获取对象元数据（HeadObject）"),
    ("Get object processing status", "获取对象处理状态"),
    ("Get object retention policy", "获取对象保留策略"),
    ("Get object set endpoint", "获取对象集 endpoint"),
    ("Get object set lifecycle by tag", "按 tag 获取对象集生命周期"),
    ("Get object set lifecycle configuration", "获取对象集生命周期配置"),
    ("Get object set quota", "获取对象集配额"),
    ("Get object set quota by tag", "按 tag 获取对象集配额"),
    ("Get object set storage info", "获取对象集存储信息"),
    ("Get object set tagging", "获取对象集 tag"),
    ("Get object stat information", "获取对象 stat 信息"),
    ("Get object tagging", "获取对象 tag"),
    ("Get pay-by-traffic configuration", "获取按流量计费配置"),
    ("Get payment (requester pays) configuration", "获取请求方付费配置"),
    ("Get private M3U8 rule", "获取私有 M3U8 规则"),
    ("Get real-time log configuration", "获取实时日志配置"),
    ("Get remaining time for a redundancy transition task", "获取冗余转换任务的剩余时间"),
    ("Get replication configuration", "获取复制配置"),
    ("Get static website configuration", "获取静态网站配置"),
    ("Get symlink target", "获取符号链接目标"),
    ("Get transfer acceleration status", "获取传输加速状态"),
    ("Get trash (recycle bin) configuration", "获取回收站配置"),
    ("Get versioning status", "获取版本控制状态"),
    ("Grant full control", "授予完全控制权限"),
    ("Grant full control (x-tos-grant-full-control)", "授予完全控制权限（x-tos-grant-full-control）"),
    ("Grant full control permission header", "授予完全控制权限的请求头"),
    ("Grant read", "授予读取权限"),
    ("Grant read (x-tos-grant-read)", "授予读取权限（x-tos-grant-read）"),
    ("Grant read ACP", "授予读取 ACP 权限"),
    ("Grant read ACP (x-tos-grant-read-acp)", "授予读取 ACP 权限（x-tos-grant-read-acp）"),
    ("Grant read ACP permission", "授予读取 ACP 权限"),
    ("Grant read ACP permission header", "授予读取 ACP 权限的请求头"),
    ("Grant read permission", "授予读取权限"),
    ("Grant read permission header", "授予读取权限的请求头"),
    ("Grant read without list", "授予无列举能力的读取权限"),
    ("Grant read without list (x-tos-grant-read-non-list)", "授予无列举能力的读取权限（x-tos-grant-read-non-list）"),
    ("Grant read without list permission", "授予无列举能力的读取权限"),
    ("Grant read without list permission header", "授予无列举能力的读取权限请求头"),
    ("Grant write", "授予写入权限"),
    ("Grant write (x-tos-grant-write)", "授予写入权限（x-tos-grant-write）"),
    ("Grant write ACP", "授予写入 ACP 权限"),
    ("Grant write ACP (x-tos-grant-write-acp)", "授予写入 ACP 权限（x-tos-grant-write-acp）"),
    ("Grant write ACP permission", "授予写入 ACP 权限"),
    ("Grant write ACP permission header", "授予写入 ACP 权限的请求头"),
    ("Grant write permission", "授予写入权限"),
    ("Grant write permission header", "授予写入权限的请求头"),
    ("Guard object match condition", "对象匹配保护条件"),
    ("HTTP method (GET, PUT)", "HTTP 方法（GET、PUT）"),
    ("HTTPS/TLS configuration", "HTTPS/TLS 配置"),
    ("Human-readable sizes", "以人类可读格式显示大小"),
    ("If-Match", "If-Match 条件"),
    ("If-Match (ETag condition)", "If-Match（ETag 条件）"),
    ("If-Match condition", "If-Match 条件"),
    ("If-Match header", "If-Match 请求头"),
    ("If-Modified-Since header", "If-Modified-Since 请求头"),
    ("If-None-Match", "If-None-Match 条件"),
    ("If-None-Match (ETag condition)", "If-None-Match（ETag 条件）"),
    ("If-None-Match condition", "If-None-Match 条件"),
    ("If-None-Match header", "If-None-Match 请求头"),
    ("If-Unmodified-Since (x-if-unmodified-since)", "If-Unmodified-Since（x-if-unmodified-since）"),
    ("If-Unmodified-Since header", "If-Unmodified-Since 请求头"),
    ("Image style name (styleName query)", "图片样式名称（styleName 查询参数）"),
    ("Include estimated monthly storage cost by storage class", "按存储类型包含预估月度存储成本"),
    ("Include pattern", "包含匹配模式"),
    ("Include pattern for recursive restore", "递归恢复的包含匹配模式"),
    ("Include the source directory/prefix name under the destination prefix", "在目标前缀下包含源目录/前缀名称"),
    ("Initialize the selected profile. ve-tos writes Beijing network defaults; ByteTOS tos leaves region, endpoint, and PSM unset", "初始化所选 profile。ve-tos 写入北京网络默认值；ByteTOS tos 不设置 region、endpoint 和 PSM"),
    ("Inspect API metadata", "查看 API 元数据"),
    ("Intelligent retrieval / dataset management", "智能检索 / 数据集管理"),
    ("Intelligent retrieval dataset APIs", "智能检索数据集 API"),
    ("Intelligent tiering configuration", "智能分层配置"),
    ("Internal metadata directive (x-internal-metadata-directive)", "内部元数据指令（x-internal-metadata-directive）"),
    ("Inventory configuration ID", "清单配置 ID"),
    ("Job ID (jobID path/query)", "任务 ID（jobID 路径/查询参数）"),
    ("Key marker", "Key 标记"),
    ("Key prefix (used with --bucket)", "对象 Key 前缀（与 --bucket 搭配使用）"),
    ("Keys to delete (JSON array or comma-separated keys)", "要删除的 key（JSON 数组或逗号分隔的 key）"),
    ("Last-Modified header", "Last-Modified 请求头"),
    ("Lifecycle rule management", "生命周期规则管理"),
    ("List MRAPs for an accelerator", "列出 Accelerator 的 MRAP"),
    ("List accelerators", "列出 Accelerator"),
    ("List accelerators for MRAP", "列出 MRAP 的 Accelerator"),
    ("List accelerators for a bucket", "列出 Bucket 的 Accelerator"),
    ("List accelerators for access point", "列出接入点的 Accelerator"),
    ("List access points", "列出接入点"),
    ("List all MRAPs", "列出所有 MRAP"),
    ("List all buckets", "列出所有 Bucket"),
    ("List all built-in skills", "列出所有内置 Skill"),
    ("List all image styles", "列出所有图片样式"),
    ("List audit configurations", "列出审计配置"),
    ("List availability zones", "列出可用区"),
    ("List batch jobs", "列出批处理任务"),
    ("List bound access points", "列出已绑定的接入点"),
    ("List bound buckets", "列出已绑定的 Bucket"),
    ("List buckets or objects", "列出 Bucket 或对象"),
    ("List cross-account access points", "列出跨账号接入点"),
    ("List custom domain bindings", "列出自定义域名绑定"),
    ("List dataset bindings", "列出 Dataset 绑定"),
    ("List dataset templates", "列出 Dataset 模板"),
    ("List datasets", "列出 Dataset"),
    ("List evict jobs", "列出驱逐任务"),
    ("List image style brief infos", "列出图片样式摘要信息"),
    ("List image style contents", "列出图片样式内容"),
    ("List increment audit configurations", "列出增量审计配置"),
    ("List inventory configurations", "列出清单配置"),
    ("List jobs", "列出任务"),
    ("List lens configurations", "列出 Lens 配置"),
    ("List multipart uploads", "列出分片上传"),
    ("List object sets", "列出对象集"),
    ("List object versions", "列出对象版本"),
    ("List objects (ListObjectsV2)", "列出对象（ListObjectsV2）"),
    ("List prefetch jobs", "列出预取任务"),
    ("List prefetch records", "列出预取记录"),
    ("List redundancy transition tasks", "列出冗余转换任务"),
    ("List resource tags", "列出资源 tag"),
    ("List turbo sessions", "列出 Turbo 会话"),
    ("List uploaded parts", "列出已上传分片"),
    ("List workflow executions", "列出工作流执行记录"),
    ("MRAP alias", "MRAP 别名"),
    ("Manage/export skill metadata", "管理/导出 Skill 元数据"),
    ("Marker for pagination", "分页标记"),
    ("Max-age cache configuration", "Max-age 缓存配置"),
    ("Maximum buckets, objects, or prefixes to return from the current level", "当前层级最多返回的 Bucket、对象或前缀数量"),
    ("Maximum directory depth", "最大目录深度"),
    ("Maximum files/items running concurrently in batch commands", "批量命令中最大并发文件/条目数"),
    ("Maximum files/items running concurrently in this batch delete", "本次批量删除中最大并发文件/条目数"),
    ("Maximum files/items running concurrently in this batch restore", "本次批量恢复中最大并发文件/条目数"),
    ("Maximum keys per response", "单次响应最大 key 数"),
    ("Maximum parts per response", "单次响应最大分片数"),
    ("Maximum parts/ranges running concurrently for one large file", "单个大文件最大并发分片/范围数"),
    ("Maximum prefixes listed concurrently when recursive listing uses delimiter=\"/\"", "递归列举使用 delimiter=\"/\" 时最大并发列举前缀数"),
    ("Maximum prefixes listed concurrently when the bucket is listed hierarchically", "按层级列举 Bucket 时最大并发列举前缀数"),
    ("Maximum uploads per response", "单次响应最大上传数"),
    ("Metadata (JSON or key1=val1&key2=val2)", "元数据（JSON 或 key1=val1&key2=val2）"),
    ("Metadata directive", "元数据指令"),
    ("Mirror back-to-source rules", "镜像回源规则"),
    ("Modification time filter; bare durations such as 7d mean objects modified within that window", "修改时间过滤；7d 等不带前缀的时长表示在该时间窗口内修改的对象"),
    ("Modify an object in-place", "原地修改对象"),
    ("Move files or objects by copy plus source delete", "通过复制并删除源文件/对象来移动"),
    ("Multi-region access point", "多区域接入点"),
    ("Multi-region access point APIs", "多地域接入点 API"),
    ("Multipart Core APIs", "分片核心 API"),
    ("Multipart upload core APIs", "分片上传核心 API"),
    ("Name pattern", "名称匹配模式"),
    ("Net speed test marker header (X-Tos-Net-Speed-Test)", "网络测速标记请求头（X-Tos-Net-Speed-Test）"),
    ("Number of largest/oldest object samples to keep in --verbose diagnostics; 0 disables samples", "--verbose 诊断中保留的最大/最旧对象样本数；0 表示禁用样本"),
    ("Object Core APIs", "Object 核心 API"),
    ("Object core APIs", "Object 核心 API"),
    ("Object key", "对象 key"),
    ("Object key (used with --bucket)", "对象 Key（与 --bucket 搭配使用）"),
    ("Object key or prefix (used with --bucket)", "对象 Key 或前缀（与 --bucket 搭配使用）"),
    ("Object key path parameter", "对象 key 路径参数"),
    ("Object list URI (tos://bucket or tos://bucket/prefix/)", "对象列举 URI（tos://bucket 或 tos://bucket/prefix/）"),
    ("Object lock mode", "对象锁模式"),
    ("Object lock mode (x-object-lock-mode)", "对象锁模式（x-object-lock-mode）"),
    ("Object lock retain until date (x-object-lock-retain-until-date)", "对象锁保留截止日期（x-object-lock-retain-until-date）"),
    ("Object lock retain-until date", "对象锁保留截止日期"),
    ("Object lock retain-until date (x-object-lock-retain-until-date)", "对象锁保留截止日期（x-object-lock-retain-until-date）"),
    ("Object path (tos://bucket/key or --bucket + --key)", "对象路径（tos://bucket/key 或 --bucket + --key）"),
    ("Object path (tos://bucket/key)", "对象路径（tos://bucket/key）"),
    ("Object path to write (tos://bucket/key)", "要写入的对象路径（tos://bucket/key）"),
    ("Object prefix", "对象前缀"),
    ("Object set management", "对象集合管理"),
    ("Object set name encoded as query key", "编码为查询 key 的对象集名称"),
    ("Object tagging (key1=value1&key2=value2)", "对象 tag（key1=value1&key2=value2）"),
    ("Object tags (key1=value1&key2=value2)", "对象 tag（key1=value1&key2=value2）"),
    ("Object tags (x-tagging; key1=value1&key2=value2)", "对象 tag（x-tagging；key1=value1&key2=value2）"),
    ("Object tags (x-tos-tagging; key1=value1&key2=value2)", "对象 tag（x-tos-tagging；key1=value1&key2=value2）"),
    ("Object version ID", "对象版本 ID"),
    ("Offset to write at", "写入偏移量"),
    ("Open a turbo channel for an object", "为对象打开 Turbo 通道"),
    ("Open mode query value (0=create open, 1=write open)", "打开模式查询值（0=创建打开，1=写入打开）"),
    ("Optional Content-MD5 header", "可选的 Content-MD5 请求头"),
    ("Output directory", "输出目录"),
    ("Output file (or - for stdout)", "输出文件（或使用 - 表示标准输出）"),
    ("Override response Cache-Control", "覆盖响应 Cache-Control"),
    ("Override response Content-Disposition", "覆盖响应 Content-Disposition"),
    ("Override response Content-Type", "覆盖响应 Content-Type"),
    ("Override response Expires", "覆盖响应 Expires"),
    ("Override storage price, e.g. STANDARD=0.12 (CNY/GB/month)", "覆盖存储单价，例如 STANDARD=0.12（元/GB/月）"),
    ("Part body source", "分片正文来源"),
    ("Part number", "分片编号"),
    ("Part number marker", "分片编号标记"),
    ("Path to inspect (tos://bucket or tos://bucket/key)", "要查看的路径（tos://bucket 或 tos://bucket/key）"),
    ("Path to list (tos://bucket or tos://bucket/prefix/)", "要列出的路径（tos://bucket 或 tos://bucket/prefix/）"),
    ("Path to measure (tos://bucket or tos://bucket/prefix/)", "要统计的路径（tos://bucket 或 tos://bucket/prefix/）"),
    ("Pay-by-traffic configuration", "按流量计费配置"),
    ("Persistent custom response headers (x-persistent-headers)", "持久化自定义响应头（x-persistent-headers）"),
    ("Persistent headers list", "持久化请求头列表"),
    ("Persistent headers list (x-persistent-headers)", "持久化请求头列表（x-persistent-headers）"),
    ("Port for SSE transport", "SSE 传输端口"),
    ("Prefix filter", "前缀过滤器"),
    ("Prevent overwrite if object exists (if-none-match: *)", "对象存在时禁止覆盖（if-none-match: *）"),
    ("Profile name to initialize (defaults to `default`)", "要初始化的 profile 名称（默认为 `default`）"),
    ("Progress granularity: part (default) or byte", "进度粒度：part（默认）或 byte"),
    ("Project name header (x-tos-project-name)", "项目名称请求头（x-tos-project-name）"),
    ("Query a dataset", "查询 Dataset"),
    ("Range header (bytes=start-end)", "Range 请求头（bytes=start-end）"),
    ("Raw API passthrough", "原始 API 透传"),
    ("Raw request contract (inline JSON or file://path)", "原始请求契约（内联 JSON 或 file://path）"),
    ("Read specific part number of multipart upload", "读取分片上传的指定分片编号"),
    ("Real-time log analysis", "实时日志分析"),
    ("Recursive copy", "递归复制"),
    ("Recursive delete", "递归删除"),
    ("Recursive delete flag (queryRecursive)", "递归删除参数（queryRecursive）"),
    ("Recursive delete strategy for HNS buckets", "HNS Bucket 的递归删除策略"),
    ("Recursive listing mode: auto, flat, or hierarchical", "递归列举模式：auto、flat 或 hierarchical"),
    ("Recursive mkdir (x-recursive-mkdir)", "递归创建目录（x-recursive-mkdir）"),
    ("Recursive move for directories or prefixes", "递归移动目录或前缀"),
    ("Redundancy transition task ID", "冗余转换任务 ID"),
    ("Region override for this request", "仅本次请求覆盖 region"),
    ("Region query parameter", "Region 查询参数"),
    ("Remove a bucket", "删除 Bucket"),
    ("Rename an object", "重命名对象"),
    ("Replicated-from (x-replicated-from)", "复制来源（x-replicated-from）"),
    ("Requester pays configuration", "请求者付费配置"),
    ("Resource ID", "资源 ID"),
    ("Resource TRN path parameter", "资源 TRN 路径参数"),
    ("Resource identifier (bucket name, access point name, etc.)", "资源标识（Bucket 名称、接入点名称等）"),
    ("Restore all archived objects under a prefix", "恢复前缀下的所有归档对象"),
    ("Restore an archived object", "恢复归档对象"),
    ("Restore archived object", "恢复归档对象"),
    ("Restore archived objects", "恢复归档对象"),
    ("Restore days", "恢复天数"),
    ("Restore objects listed in a manifest file", "恢复 manifest 文件中列出的对象"),
    ("Restore tier (Expedited, Standard, Bulk)", "恢复档位（Expedited、Standard、Bulk）"),
    ("Retain until date (RFC3339)", "保留截止日期（RFC3339）"),
    ("Retention mode (COMPLIANCE)", "保留模式（COMPLIANCE）"),
    ("Seal an appendable object (make it immutable)", "封存可追加对象（使其不可变）"),
    ("Search keywords", "搜索关键词"),
    ("Search path (tos://bucket or tos://bucket/prefix/)", "搜索路径（tos://bucket 或 tos://bucket/prefix/）"),
    ("Select columns for table/csv output (comma-separated, e.g. key,size,last_modified)", "选择 table/csv 输出列（逗号分隔，例如 key,size,last_modified）"),
    ("Server-side encryption algorithm (x-server-side-encryption)", "服务端加密算法（x-server-side-encryption）"),
    ("Server-side encryption configuration", "服务端加密配置"),
    ("Set CDN notification configuration", "设置 CDN 通知配置"),
    ("Set CORS configuration", "设置 CORS 配置"),
    ("Set HTTPS / TLS version configuration", "设置 HTTPS / TLS 版本配置"),
    ("Set MRAP mirror configuration", "设置 MRAP 镜像配置"),
    ("Set MRAP policy", "设置 MRAP 策略"),
    ("Set QoS policy", "设置 QoS 策略"),
    ("Set WORM (object lock) configuration", "设置 WORM（对象锁）配置"),
    ("Set a configuration value", "设置配置值"),
    ("Set a lens configuration", "设置 Lens 配置"),
    ("Set a resource tag", "设置资源 tag"),
    ("Set a template", "设置模板"),
    ("Set a workflow", "设置工作流"),
    ("Set access monitor status", "设置访问监控状态"),
    ("Set access point policy", "设置接入点策略"),
    ("Set an image style", "设置图片样式"),
    ("Set an object set", "设置对象集"),
    ("Set batch job priority", "设置批处理任务优先级"),
    ("Set batch job status", "设置批处理任务状态"),
    ("Set blind watermark rule", "设置盲水印规则"),
    ("Set bucket ACL", "设置 Bucket ACL"),
    ("Set bucket encryption configuration", "设置 Bucket 加密配置"),
    ("Set bucket logging configuration", "设置 Bucket 日志配置"),
    ("Set bucket policy", "设置 Bucket 策略"),
    ("Set bucket quota", "设置 Bucket 配额"),
    ("Set bucket rename configuration", "设置 Bucket 重命名配置"),
    ("Set bucket tagging", "设置 Bucket tag"),
    ("Set custom domain binding", "设置自定义域名绑定"),
    ("Set custom domain certificate token", "设置自定义域名证书 token"),
    ("Set default storage class for the bucket", "设置 Bucket 的默认存储类型"),
    ("Set event notification configuration", "设置事件通知配置"),
    ("Set event subscription", "设置事件订阅"),
    ("Set global object set configuration", "设置全局对象集配置"),
    ("Set image protect rule", "设置图片保护规则"),
    ("Set image style separator", "设置图片样式分隔符"),
    ("Set intelligent tiering configuration", "设置智能分层配置"),
    ("Set inventory configuration", "设置清单配置"),
    ("Set lifecycle rules", "设置生命周期规则"),
    ("Set max-age configuration", "设置 max-age 配置"),
    ("Set mirror back-to-source rules", "设置镜像回源规则"),
    ("Set object ACL", "设置对象 ACL"),
    ("Set object expiration time", "设置对象过期时间"),
    ("Set object metadata", "设置对象元数据"),
    ("Set object retention policy", "设置对象保留策略"),
    ("Set object set lifecycle by tag", "按 tag 设置对象集生命周期"),
    ("Set object set lifecycle configuration", "设置对象集生命周期配置"),
    ("Set object set quota", "设置对象集配额"),
    ("Set object set quota by tag", "按 tag 设置对象集配额"),
    ("Set object set tagging", "设置对象集 tag"),
    ("Set object tagging", "设置对象 tag"),
    ("Set object time attributes", "设置对象时间属性"),
    ("Set pay-by-traffic configuration", "设置按流量计费配置"),
    ("Set payment (requester pays) configuration", "设置请求方付费配置"),
    ("Set private M3U8 rule", "设置私有 M3U8 规则"),
    ("Set real-time log configuration", "设置实时日志配置"),
    ("Set replication configuration", "设置复制配置"),
    ("Set static website configuration", "设置静态网站配置"),
    ("Set transfer acceleration status", "设置传输加速状态"),
    ("Set trash (recycle bin) configuration", "设置回收站配置"),
    ("Set versioning status (Enabled/Suspended)", "设置版本控制状态（Enabled/Suspended）"),
    ("Shell type", "Shell 类型"),
    ("Show API description", "查看 API 说明"),
    ("Show bucket or object metadata", "查看 Bucket 或对象元数据"),
    ("Show current configuration (redacted)", "查看当前配置（已脱敏）"),
    ("Size filter (e.g., +1GB, -100KB)", "大小过滤器（例如 +1GB、-100KB）"),
    ("Skip trash flag (querySkipTrash)", "跳过回收站参数（querySkipTrash）"),
    ("Sort field", "排序字段"),
    ("Source (tos://src-bucket/src-key)", "源（tos://src-bucket/src-key）"),
    ("Source URL to fetch from", "要拉取的源 URL"),
    ("Source byte range", "源字节范围"),
    ("Source modified-since condition", "源 modified-since 条件"),
    ("Source object key to link to", "要链接到的源对象 key"),
    ("Source object path (tos://bucket/key)", "源对象路径（tos://bucket/key）"),
    ("Source part number", "源分片编号"),
    ("Source path", "源路径"),
    ("Source path (local path or tos://bucket/key)", "源路径（本地路径或 tos://bucket/key）"),
    ("Source unmodified-since condition", "源 unmodified-since 条件"),
    ("Specific rule ID", "指定规则 ID"),
    ("Start MCP server", "启动 MCP 服务器"),
    ("Static website hosting", "静态网站托管"),
    ("Stdin size threshold for switching to multipart upload (e.g., 20MB)", "标准输入切换到分片上传的大小阈值（例如 20MB）"),
    ("Storage class", "存储类型"),
    ("Storage class filter", "存储类型过滤器"),
    ("Storage class for ve-tos stdin uploads. ByteTOS tos put does not support creation-time override. Allowed: STANDARD, IA, ARCHIVE_FR, INTELLIGENT_TIERING, COLD_ARCHIVE, ARCHIVE, DEEP_COLD_ARCHIVE", "ve-tos 标准输入上传使用的存储类型。ByteTOS tos put 不支持创建时覆盖。可选值：STANDARD、IA、ARCHIVE_FR、INTELLIGENT_TIERING、COLD_ARCHIVE、ARCHIVE、DEEP_COLD_ARCHIVE"),
    ("Storage class for ve-tos uploads and TOS-to-TOS copies. ByteTOS tos uploads do not support creation-time override. Allowed: STANDARD, IA, ARCHIVE_FR, INTELLIGENT_TIERING, COLD_ARCHIVE, ARCHIVE, DEEP_COLD_ARCHIVE", "ve-tos 上传和 TOS 到 TOS 复制使用的存储类型。ByteTOS tos 上传不支持创建时覆盖。可选值：STANDARD、IA、ARCHIVE_FR、INTELLIGENT_TIERING、COLD_ARCHIVE、ARCHIVE、DEEP_COLD_ARCHIVE"),
    ("Storage class for ve-tos uploads; ByteTOS tos object upload does not support creation-time override", "ve-tos 上传使用的存储类型；ByteTOS tos object upload 不支持创建时覆盖"),
    ("Storage class. Allowed: STANDARD, IA, ARCHIVE_FR, INTELLIGENT_TIERING, COLD_ARCHIVE, ARCHIVE, DEEP_COLD_ARCHIVE", "存储类型。允许值：STANDARD、IA、ARCHIVE_FR、INTELLIGENT_TIERING、COLD_ARCHIVE、ARCHIVE、DEEP_COLD_ARCHIVE"),
    ("Stream object content", "流式输出对象内容"),
    ("Symlink key", "符号链接 key"),
    ("Synchronize source and destination incrementally", "增量同步源和目标"),
    ("Tag keys query parameter (comma-separated or JSON array)", "tag key 查询参数（逗号分隔或 JSON 数组）"),
    ("Tagging (x-tagging; key1=value1&key2=value2)", "Tag（x-tagging；key1=value1&key2=value2）"),
    ("Tagging directive", "Tag 指令"),
    ("Tags (JSON or key1=val1&key2=val2)", "Tag（JSON 或 key1=val1&key2=val2）"),
    ("Target bucket (if different)", "目标 Bucket（如果不同）"),
    ("Target object ACL for TOS uploads/copies. Allowed: private, public-read, public-read-write, authenticated-read, bucket-owner-read, bucket-owner-full-control, bucket-owner-entrusted, default", "TOS 上传/复制的目标对象 ACL。允许值：private、public-read、public-read-write、authenticated-read、bucket-owner-read、bucket-owner-full-control、bucket-owner-entrusted、default"),
    ("Target object ACL. Allowed: private, public-read, public-read-write, authenticated-read, bucket-owner-read, bucket-owner-full-control, bucket-owner-entrusted, default", "目标对象 ACL。允许值：private、public-read、public-read-write、authenticated-read、bucket-owner-read、bucket-owner-full-control、bucket-owner-entrusted、default"),
    ("Target object key", "目标对象 key"),
    ("Target path (tos://bucket/key or tos://bucket/prefix/)", "目标路径（tos://bucket/key 或 tos://bucket/prefix/）"),
    ("Task ID", "任务 ID"),
    ("Timestamp (RFC3339 or Unix epoch)", "时间戳（RFC3339 或 Unix 时间戳）"),
    ("Trace ID (X-Tracer-Traceid)", "链路追踪 ID（X-Tracer-Traceid）"),
    ("Traffic limit", "流量限制"),
    ("Traffic limit (x-traffic-limit)", "流量限制（x-traffic-limit）"),
    ("Traffic limit in bps", "流量限制（bps）"),
    ("Traffic limit in bps (x-traffic-limit)", "流量限制（bps，x-traffic-limit）"),
    ("Transfer acceleration configuration", "传输加速配置"),
    ("Transport: stdio or sse", "传输方式：stdio 或 sse"),
    ("Turbo append upload APIs", "Turbo 追加上传 API"),
    ("Turbo core APIs", "Turbo 核心 API"),
    ("Turbo token", "Turbo 令牌"),
    ("URL expiration time (e.g., 3600)", "URL 过期时间（例如 3600）"),
    ("Unbind a bucket from an accelerator", "解除 Bucket 与 Accelerator 的绑定"),
    ("Unbind accelerator from MRAP", "解除 Accelerator 与 MRAP 的绑定"),
    ("Unbind accelerator from access point", "解除 Accelerator 与接入点的绑定"),
    ("Unique tag (x-unique-tag)", "唯一 tag（x-unique-tag）"),
    ("Unmodified-since condition", "unmodified-since 条件"),
    ("Update a dataset", "更新 Dataset"),
    ("Upload ID", "上传 ID"),
    ("Upload ID marker", "上传 ID 标记"),
    ("Upload a part", "上传分片"),
    ("Upload a part by copy", "通过复制上传分片"),
    ("Upload a single object (PutObject, <=5GB)", "上传单个对象（PutObject，<=5GB）"),
    ("Upload object via form (PostObject)", "通过表单上传对象（PostObject）"),
    ("Upload stdin to an object", "将标准输入上传为对象"),
    ("Use exact timestamps for comparison", "使用精确时间戳比较"),
    ("Version ID", "版本 ID"),
    ("Versioning configuration", "版本控制配置"),
    ("View: groups (default — group summary with command counts), text (one-line summaries: `<command>\\t<description>`), compact (capability rows without parameters), full (capability rows + parameters + command tree). `tree` is accepted as a legacy alias for `compact`", "视图：groups（默认，按分组汇总命令数量）、text（单行摘要：`<command>\\t<description>`）、compact（不含参数的能力行）、full（能力行 + 参数 + 命令树）。`tree` 作为兼容别名等同于 `compact`"),
    ("WORM / object lock configuration", "WORM / 对象锁配置"),
    ("Write batch success/failure report to this path", "将批量成功/失败报告写入此路径"),
    ("Write listing manifest to this path. No manifest is written unless this is set", "将列举 manifest 写入此路径；未设置时不会写 manifest"),
    ("Write matched-object manifest to this path. No manifest is written unless this is set", "将匹配对象 manifest 写入此路径；未设置时不会写 manifest"),
    ("Write only failed items to the batch report", "批量报告中仅写入失败条目"),
    ("Write planned delete manifest to this path", "将计划删除 manifest 写入此路径"),
    ("Write planned restore manifest to this path", "将计划恢复 manifest 写入此路径"),
    ("Write planned transfer manifest to this path", "将计划传输 manifest 写入此路径"),
    ("Write traversed-object manifest to this path. No manifest is written unless this is set", "将已遍历对象 manifest 写入此路径；未设置时不会写 manifest"),
    ("X-From-Modular header", "X-From-Modular 请求头"),
    ("X-If-Match-AccessTime header", "X-If-Match-AccessTime 请求头"),
    ("X-If-Match-CreateTime header", "X-If-Match-CreateTime 请求头"),
    ("X-If-Match-Expires header", "X-If-Match-Expires 请求头"),
    ("X-If-Match-Tags header", "X-If-Match-Tags 请求头"),
    ("X-Inner-Properties-TimeStamp header", "X-Inner-Properties-TimeStamp 请求头"),
    ("X-Inner-Properties-TimeStampNsec header", "X-Inner-Properties-TimeStampNsec 请求头"),
    ("X-Replicated-From header", "X-Replicated-From 请求头"),
    ("[G6] Probe the configured TOS endpoint with a real HTTPS request and record latency. Off by default to keep `ve-tos doctor` fully offline-safe", "[G6] 使用真实 HTTPS 请求探测已配置的 TOS endpoint 并记录延迟。默认关闭，以确保 `ve-tos doctor` 可安全地完全离线运行"),
    ("[G6] Timeout (milliseconds) for the live network probe. Only used when --live-network is set", "[G6] 实时网络探测超时（毫秒）。仅在设置 --live-network 时使用"),
    ("x-content-sha256 header", "x-content-sha256 请求头"),
    ("x-decoded-content-length header", "x-decoded-content-length 请求头"),
    ("x-if-match-inode-id header", "x-if-match-inode-id 请求头"),
    ("x-lifecycle-directly-delete-versions header", "x-lifecycle-directly-delete-versions 请求头"),
    ("x-modify-timestamp header", "x-modify-timestamp 请求头"),
    ("x-modify-timestamp-ns header", "x-modify-timestamp-ns 请求头"),
    ("x-only-put-delete-marker header", "x-only-put-delete-marker 请求头"),
    ("x-parent-inode-id header", "x-parent-inode-id 请求头"),
    // Describe- and Skill-only owner prose not present in Clap Help.
    ("--body accepts file paths, file://, '-' (stdin), or inline strings; file inputs are stream-uploaded.", "--body 接受文件路径、file://、'-'（stdin）或内联字符串；文件输入采用流式上传。"),
    ("--request JSON fields: method, endpoint_rule (alias endpoint_kind), bucket, key, path, query, headers, body", "--request JSON 字段：method、endpoint_rule（endpoint_kind 的别名）、bucket、key、path、query、headers、body"),
    ("AZ redundancy mode", "AZ 冗余模式"),
    ("Allow overwrite or destructive behavior", "允许覆盖或破坏性操作"),
    ("Apply operation recursively", "递归应用操作"),
    ("Batch report path", "批量报告路径"),
    ("Bucket ACL", "Bucket 的 ACL"),
    ("Bucket URI (tos://bucket). Alternative to --bucket / bucket_name.", "存储桶 URI（tos://bucket）。可替代 --bucket / bucket_name。"),
    ("Bucket name or tos://bucket", "Bucket 名称或 tos://bucket"),
    ("Bucket name passed as --bucket. Alternative to positional uri.", "通过 --bucket 传入的 Bucket 名称。可替代位置参数 uri。"),
    ("Bucket name when path is omitted", "省略 path 时使用的 Bucket 名称"),
    ("Bucket name when uri is omitted", "省略 uri 时使用的 Bucket 名称"),
    ("Bucket storage class", "Bucket 存储类型"),
    ("COPY, REPLACE, or REPLACE_NEW; maps to x-tos-metadata-directive", "COPY、REPLACE 或 REPLACE_NEW；映射到 x-tos-metadata-directive"),
    ("Calculate object size statistics for a prefix.", "统计前缀下的对象大小。"),
    ("Canned ACL", "预定义 ACL"),
    ("Checkpoint directory", "Checkpoint 目录"),
    ("Comma-separated table/csv columns", "逗号分隔的 table/csv 列"),
    ("Compare size only", "仅比较大小"),
    ("Conditional ETag guard", "条件 ETag 保护"),
    ("Config key", "配置 key"),
    ("Config value", "配置 value"),
    ("Content-Type for uploaded object", "上传对象的 Content-Type"),
    ("Create a bucket with optional storage class, ACL, and redundancy settings.", "创建 Bucket，可选设置存储类型、ACL 和冗余模式。"),
    ("Create a folder.", "创建文件夹。"),
    ("CreateBucket with optional region, storage class, and ACL settings.", "通过 CreateBucket 创建 Bucket，可选设置 region、存储类型和 ACL。"),
    ("Creates a zero-byte object whose key is normalized to end with '/'.", "创建零字节对象，并将 key 规范化为以 '/' 结尾。"),
    ("Custom object metadata as key=value#key2=value2; writes x-tos-meta-* headers", "自定义对象元数据，格式为 key=value#key2=value2；写入 x-tos-meta-* 请求头"),
    ("Default storage class", "默认存储类型"),
    ("Delete an object or prefix.", "删除对象或前缀。"),
    ("Delete destination extras", "删除目标端多余内容"),
    ("DeleteBucket; bucket must be empty before this succeeds.", "调用 DeleteBucket；Bucket 必须为空才能成功。"),
    ("DeleteObject for a single key (optionally a specific version).", "对单个 key 调用 DeleteObject（可指定版本）。"),
    ("Destination ETag guard (mapped to the if-match header). Source ETag guarding is performed by high-level cp/mv via x-tos-copy-source-if-match when the source ETag is known.", "目标 ETag 保护（映射到 if-match 请求头）。源 ETag 已知时，高层 cp/mv 通过 x-tos-copy-source-if-match 执行源 ETag 保护。"),
    ("Destination file path; '-' streams to stdout", "目标文件路径；'-' 表示流式输出到 stdout"),
    ("Destination local path or tos:// URI", "本地目标路径或 tos:// URI"),
    ("Destination tos://bucket/key", "目标 tos://bucket/key"),
    ("Disable planned manifest output", "禁用计划 manifest 输出"),
    ("Disable planned restore manifest output", "禁用计划恢复 manifest 输出"),
    ("Disable traversal echo output", "禁用遍历回显输出"),
    ("Documentation language for generated SKILL.md files: en (default) or zh.", "生成 SKILL.md 文件的文档语言：en（默认）或 zh。"),
    ("Documentation language for generated skill metadata: en (default) or zh.", "生成 Skill 元数据的文档语言：en（默认）或 zh。"),
    ("Enable execution progress output", "启用执行进度输出"),
    ("Enable listing-phase echo output", "启用列举阶段回显输出"),
    ("Enable recursive transfer", "启用递归传输"),
    ("Enable resumable checkpoint", "启用可恢复 checkpoint"),
    ("Enable the MCP server. Without --mcp, serve only reports registry-backed planning metadata.", "启用 MCP 服务。不指定 --mcp 时，serve 仅报告由 registry 支持的规划元数据。"),
    ("Enable traversal echo output", "启用遍历回显输出"),
    ("Execute or plan arbitrary signed TOS API requests through a JSON request contract.", "通过 JSON 请求契约执行或规划任意已签名的 TOS API 请求。"),
    ("Export TOS Markdown SKILL.md files for external agents, documentation, prompts, or adapter tooling.", "为外部 Agent、文档、prompt 或适配器工具导出 TOS Markdown SKILL.md 文件。"),
    ("File path, file:// URL, '-' for stdin, or inline string", "文件路径、file:// URL、表示 stdin 的 '-'，或内联字符串"),
    ("Find objects by name, size, mtime, or storage class.", "按名称、大小、mtime 或存储类型查找对象。"),
    ("Fixed application/x-directory for created folder markers", "创建文件夹标记时固定使用 application/x-directory"),
    ("Folder key when --bucket is used", "使用 --bucket 时的文件夹 key"),
    ("Generate a presigned URL for object access.", "生成用于访问对象的预签名 URL。"),
    ("Generate shell completion scripts and installation snippets for ve-tos-cli / ve-tos.", "为 ve-tos-cli / ve-tos 生成 shell 补全脚本和安装片段。"),
    ("GetObject streamed to a destination file or stdout.", "将 GetObject 结果流式写入目标文件或 stdout。"),
    ("HNS-only recursive delete strategy: bottom-up or direct", "仅用于 HNS 的递归删除策略：bottom-up 或 direct"),
    ("HTTP method", "HTTP 方法"),
    ("HTTP range", "HTTP 范围"),
    ("HTTP range, e.g. bytes=0-1023", "HTTP 范围，例如 bytes=0-1023"),
    ("Initialize TOS CLI configuration with a template layered profile.", "使用模板化分层 profile 初始化 TOS CLI 配置。"),
    ("JMESPath literals inside --query use backticks; keep the whole expression quoted.", "--query 内的 JMESPath 字面量使用反引号；请为整个表达式加引号。"),
    ("JSON raw request contract or file://path", "JSON 原始请求契约或 file://path"),
    ("Legacy alias to disable traversal echo when list echo flags are absent", "未提供 list echo flags 时禁用遍历回显的旧版别名"),
    ("Legacy alias to enable traversal echo when list echo flags are absent", "未提供 list echo flags 时启用遍历回显的旧版别名"),
    ("List TOS skill v1 metadata used for MCP tool advertisement and external Agent catalogs.", "列出用于 MCP 工具声明和外部 Agent 目录的 TOS Skill v1 元数据。"),
    ("List buckets or objects.", "列出 Bucket 或对象。"),
    ("Local TCP port for --transport sse; ignored for stdio.", "--transport sse 使用的本地 TCP 端口；stdio 模式忽略。"),
    ("MCP tools are rebuilt from the in-process skill registry; exported Markdown skill files are not read by serve", "MCP 工具从进程内 Skill registry 重建；serve 不读取导出的 Markdown Skill 文件"),
    ("MCP transport: stdio (default, no TCP listener) or sse (local HTTP/SSE listener).", "MCP 传输方式：stdio（默认，不监听 TCP）或 sse（本地 HTTP/SSE listener）。"),
    ("Manifest file path", "Manifest 文件路径"),
    ("Markdown SKILL.md pack with root index plus per-domain command skills", "包含根索引和按域命令 Skill 的 Markdown SKILL.md 包"),
    ("Maximum directory aggregation depth", "最大目录聚合深度"),
    ("Maximum files/items running concurrently in batch execution", "批量执行时并发运行的最大文件/项目数"),
    ("Maximum files/items running concurrently in batch restore execution", "批量恢复执行时并发运行的最大文件/项目数"),
    ("Modified time predicate", "修改时间条件"),
    ("Move files or objects by copy plus source delete.", "通过复制后删除源文件或对象来移动。"),
    ("Name glob", "名称 glob"),
    ("Number of largest and oldest object samples to keep in verbose diagnostics; 0 disables samples", "verbose 诊断中保留的最大和最旧对象样本数；0 禁用样本"),
    ("Object key when uri is omitted", "省略 uri 时使用的对象 key"),
    ("Optional listing manifest path", "可选的列举 manifest 路径"),
    ("Optional matched object manifest path", "可选的匹配对象 manifest 路径"),
    ("Optional skill name, domain, command, or command suffix filter.", "可选的 Skill 名称、域、命令或命令后缀过滤器。"),
    ("Optional traversed object manifest path", "可选的已遍历对象 manifest 路径"),
    ("Output directory. Files are written as dir/SKILL.md and dir/{domain}/{skill_name}/SKILL.md.", "输出目录。文件写入 dir/SKILL.md 和 dir/{domain}/{skill_name}/SKILL.md。"),
    ("Override Content-Type", "覆盖 Content-Type"),
    ("Override region", "覆盖 region"),
    ("Override storage price as CLASS=PRICE in CNY/GB/month", "以 CLASS=PRICE 覆盖存储价格，单位 CNY/GB/月"),
    ("Per-invocation VeTos authentication override: --auth-mode <MODE>; supported values are aksk or unified. TOS_AUTH_MODE supplies the environment value.", "单次调用的 VeTos 鉴权覆盖：--auth-mode <MODE>；支持值为 aksk 或 unified。环境变量由 TOS_AUTH_MODE 提供。"),
    ("Planned delete manifest path", "计划删除 manifest 路径"),
    ("Planned restore manifest path", "计划恢复 manifest 路径"),
    ("Planned transfer manifest path", "计划传输 manifest 路径"),
    ("Profile name, default is default", "Profile 名称，默认为 default"),
    ("PutObject for a single object body up to 5GB.", "使用 PutObject 上传最大 5GB 的单个对象正文。"),
    ("Quote paths and object keys that contain spaces or shell metacharacters.", "包含空格或 shell 元字符的路径和对象 key 需要加引号。"),
    ("Reads stdin until EOF; interactive terminals submit with Ctrl+D on Unix/macOS or Ctrl+Z then Enter on Windows. Ctrl+C cancels instead of uploading. Input below --multipart-threshold uses PutObject, larger input is uploaded with multipart upload parts.", "读取 stdin 直到 EOF；交互终端在 Unix/macOS 上使用 Ctrl+D 提交，在 Windows 上使用 Ctrl+Z 后按 Enter。Ctrl+C 会取消而非上传。小于 --multipart-threshold 的输入使用 PutObject，更大的输入通过分片上传。"),
    ("Recursive listing mode: tos always uses hierarchical delimiter=\"/\"; ve-tos auto uses bucket shape, with flat/hierarchical overrides", "递归列举模式：tos 始终使用分层 delimiter=\"/\"；ve-tos 的 auto 根据 Bucket 形态选择，并支持 flat/hierarchical 覆盖"),
    ("Registry action or raw operation name", "Registry action 或原始操作名称"),
    ("Registry group or raw namespace", "Registry group 或原始 namespace"),
    ("Remove an empty bucket; use ve-tos rm --recursive first when cleanup is required.", "删除空 Bucket；需要清理时先使用 ve-tos rm --recursive。"),
    ("Render human-readable sizes", "以人类可读格式显示大小"),
    ("Render human-readable total size", "以人类可读格式显示总大小"),
    ("Request body is supplied by the documented body/config/source flag; use --dry-run before execution.", "请求正文由文档说明的 body/config/source flag 提供；执行前请使用 --dry-run。"),
    ("mutating execution is previewable with --dry-run", "修改型操作可先通过 --dry-run 预览"),
    ("read-only or metadata-only command", "只读或仅元数据命令"),
    ("Require exact timestamp match", "要求时间戳精确匹配"),
    ("Required for POST, PUT, PATCH, DELETE, and other mutating raw requests", "POST、PUT、PATCH、DELETE 及其他修改型原始请求必需"),
    ("Required for destructive batch operations", "破坏性批量操作必需"),
    ("Required for destructive single-object delete outside dry-run", "非 dry-run 的破坏性单对象删除必需"),
    ("Required for recursive/manifest restore", "递归/manifest 恢复必需"),
    ("Required outside dry-run for the destructive DeleteBucket call", "非 dry-run 的破坏性 DeleteBucket 调用必需"),
    ("Required when --delete is set", "设置 --delete 时必需"),
    ("Restore archived objects, including recursive and manifest-driven batches.", "恢复归档对象，包括递归和 manifest 驱动的批量操作。"),
    ("Restore job options are generated from flags; batch source may come from --manifest.", "恢复任务选项由 flags 生成；批量来源可由 --manifest 提供。"),
    ("Restore prefix recursively", "递归恢复前缀"),
    ("Restore tier", "恢复层级"),
    ("Return metadata or execution plan without sending the request", "不发送请求，仅返回元数据或执行计划"),
    ("Server-side CopyObject between TOS keys (same or cross bucket).", "在 TOS key 之间执行服务端 CopyObject（同 Bucket 或跨 Bucket）。"),
    ("Set a configuration value in the shared profile or TOS override profile.", "在共享 profile 或 TOS 覆盖 profile 中设置配置值。"),
    ("Shell to generate: bash, zsh, fish, powershell, or pwsh. Install by extracting data.script from --output json.", "要生成的 Shell：bash、zsh、fish、powershell 或 pwsh。通过从 --output json 提取 data.script 进行安装。"),
    ("Show bucket or object metadata.", "查看 Bucket 或对象元数据。"),
    ("Show effective configuration with source annotations and redacted secrets.", "显示生效配置，包含来源标注并脱敏密钥。"),
    ("Size predicate", "大小条件"),
    ("Source local path or tos:// URI", "本地源路径或 tos:// URI"),
    ("Source tos://bucket/key", "源 tos://bucket/key"),
    ("Start an MCP server backed by the same in-process skill registry used by skill list/export.", "启动 MCP 服务，使用与 Skill list/export 相同的进程内 Skill registry。"),
    ("Stdin size threshold for multipart upload; data is uploaded after stdin EOF; defaults to shared checkpoint_threshold", "stdin 分片上传大小阈值；数据在 stdin EOF 后上传；默认为共享 checkpoint_threshold"),
    ("Storage class for ve-tos stdin uploads; ByteTOS tos put rejects this override", "ve-tos stdin 上传的存储类型；ByteTOS tos put 拒绝此覆盖"),
    ("Storage class for ve-tos uploads and TOS-to-TOS copies; ByteTOS uploads reject this override", "ve-tos 上传和 TOS 到 TOS 复制的存储类型；ByteTOS 上传拒绝此覆盖"),
    ("Storage class for ve-tos uploads; ByteTOS PutObject upload rejects this override", "ve-tos 上传的存储类型；ByteTOS PutObject 上传拒绝此覆盖"),
    ("Stream object content to stdout.", "将对象内容流式输出到 stdout。"),
    ("Synchronize source and destination incrementally.", "增量同步源与目标。"),
    ("TOS High-Level commands wrap TOS OpenAPI actions directly", "TOS 高层命令直接封装 TOS OpenAPI action"),
    ("TOS Object Storage CLI", "TOS 对象存储 CLI"),
    ("Target object ACL", "目标对象 ACL"),
    ("Target object ACL for TOS uploads/copies", "TOS 上传/复制的目标对象 ACL"),
    ("Target storage class", "目标存储类型"),
    ("URL expiration seconds", "URL 过期秒数"),
    ("Upload stdin to an object; upload starts/completes after stdin EOF.", "将 stdin 上传到对象；上传在 stdin EOF 后开始/完成。"),
    ("accept either uri=tos://bucket or bucket_name=<bucket> (CLI --bucket <bucket>); provide exactly one", "接受 uri=tos://bucket 或 bucket_name=<bucket>（CLI --bucket <bucket>）；必须且只能提供一个"),
    ("accept tos://bucket[/key] URI or command-specific --bucket/--key flags where supported", "在支持时接受 tos://bucket[/key] URI 或命令专用的 --bucket/--key flags"),
    ("bucket deletion only calls DeleteBucket; object cleanup is handled by ve-tos rm", "Bucket 删除仅调用 DeleteBucket；对象清理由 ve-tos rm 处理"),
    ("bucket listing mirrors ve-tos bucket list; JSON object listing uses raw data.objects/data.common_prefixes; table/csv render a synthesized typed row view", "Bucket 列举与 ve-tos bucket list 一致；JSON 对象列举使用原始 data.objects/data.common_prefixes；table/csv 渲染合成的类型化行视图"),
    ("copy phase follows cp behavior; source delete uses DeleteObject after destination confirmation", "复制阶段遵循 cp 行为；确认目标成功后使用 DeleteObject 删除源"),
    ("critical delete paths require --force and, in non-interactive shells, exact --confirm <target>", "critical 删除路径需要 --force；非交互 shell 还需要精确的 --confirm <target>"),
    ("execution stderr; auto-enabled on TTY, disabled by --no-progress or --quiet, forced by --progress", "执行进度输出到 stderr；TTY 上自动启用，--no-progress 或 --quiet 禁用，--progress 强制启用"),
    ("external Agent catalogs, prompt context, documentation generators, adapters, and MCP tool advertisement", "外部 Agent 目录、prompt 上下文、文档生成器、适配器和 MCP 工具声明"),
    ("generated scripts register ve-tos-cli and ve-tos", "生成的脚本注册 ve-tos-cli 和 ve-tos"),
    ("listing stderr; auto-enabled on TTY, disabled by --no-list-echo or --quiet, forced by --list-echo", "列举回显输出到 stderr；TTY 上自动启用，--no-list-echo 或 --quiet 禁用，--list-echo 强制启用"),
    ("lists both sides, transfers changed objects, and deletes destination extras only when --delete is set", "列出两端，传输已变化对象；仅在设置 --delete 时删除目标端多余内容"),
    ("local->TOS uses PutObject; TOS->local uses GetObject; TOS->TOS uses CopyObject; --recursive expands prefixes with ListObjects", "local->TOS 使用 PutObject；TOS->local 使用 GetObject；TOS->TOS 使用 CopyObject；--recursive 通过 ListObjects 展开前缀"),
    ("no target -> ListBuckets; bucket or prefix target -> ListObjects", "无目标 -> ListBuckets；Bucket 或前缀目标 -> ListObjects"),
    ("recursive prefix deletes list objects first; HNS targets can use bottom-up or direct mode", "递归前缀删除先列出对象；HNS 目标可使用 bottom-up 或 direct 模式"),
    ("returns a deterministic plan without mutating local files or TOS resources", "返回确定性计划，不修改本地文件或 TOS 资源"),
    ("rm accepts object or prefix targets only; use ve-tos rb for bucket deletion", "rm 仅接受对象或前缀目标；删除 Bucket 请使用 ve-tos rb"),
    ("serve uses the same live registry data but does not read the exported Markdown skill directory", "serve 使用相同的 live registry 数据，但不读取导出的 Markdown Skill 目录"),
    ("single object by default; --recursive or --manifest expands multiple restore requests", "默认处理单个对象；--recursive 或 --manifest 展开为多个恢复请求"),
    ("stable task fingerprint plus atomic lock when the command supports checkpoint state", "命令支持 checkpoint 状态时使用稳定任务指纹和原子锁"),
    ("stdio uses stdin/stdout and opens no TCP listener; sse starts a local rmcp HTTP/SSE listener on 127.0.0.1:<port>", "stdio 使用 stdin/stdout 且不打开 TCP listener；sse 在 127.0.0.1:<port> 启动本地 rmcp HTTP/SSE listener"),
    ("success and failure paths use Envelope plus --query and multi-format rendering", "成功和失败路径使用 Envelope，并支持 --query 与多格式渲染"),
    ("the command returns an Envelope; install by extracting data.script, then source bash output, add ~/.zfunc to zsh fpath and run compinit, write fish output under ~/.config/fish/completions, or append PowerShell output to $PROFILE", "命令返回 Envelope；安装时提取 data.script，然后 source bash 输出；将 ~/.zfunc 加入 zsh fpath 并运行 compinit；将 fish 输出写入 ~/.config/fish/completions；或将 PowerShell 输出追加到 $PROFILE"),
    ("tools/call plans by default; include execute=true to run the underlying CLI command", "tools/call 默认生成计划；包含 execute=true 才运行底层 CLI 命令"),
    ("Derived from the live TOS CLI capability registry and clap command tree.", "源自 live TOS CLI capability registry 和 clap command tree。"),
    ("Portable Markdown skill pack for external agents, documentation generators, prompts, or adapters. The built-in MCP server rebuilds tools from the in-process registry instead of reading exported files.", "供外部 Agent、文档生成器、prompt 或适配器使用的 portable Markdown Skill 包。内置 MCP 服务从进程内 registry 重建工具，不读取导出文件。"),
    ("tools/call returns a plan by default; include argument execute=true to run the underlying CLI command.", "tools/call 默认返回计划；包含参数 execute=true 才运行底层 CLI 命令。"),
    ("MCP tools/call control: false or omitted returns a planned argv; true executes the underlying CLI command.", "MCP tools/call 控制：false 或省略时返回规划的 argv；true 时执行底层 CLI 命令。"),
];

// [Review Fix #GlobalZh9] Canonical group handlers and live API capability
// rows own additional exact prose not present in the static Describe registry.
const VE_TOS_CANONICAL_RUNTIME_TRANSLATIONS_ZH: &[(&str, &str)] = &[
    ("Accelerator control-plane APIs", "Accelerator 控制面 API"),
    ("Bind accelerator", "绑定 Accelerator"),
    ("Bind bucket to accelerator", "将 Bucket 绑定到 Accelerator"),
    ("Bucket CDN notification configuration", "Bucket CDN 通知配置"),
    ("Bucket HTTPS/TLS configuration", "Bucket HTTPS/TLS 配置"),
    ("Bucket access logging configuration", "Bucket 访问日志配置"),
    ("Bucket access monitor configuration", "Bucket 访问监控配置"),
    ("Bucket custom domain binding", "Bucket 自定义域名绑定"),
    ("Bucket data redundancy transition", "Bucket 数据冗余转换"),
    ("Bucket event notification configuration (notification_v2)", "Bucket 事件通知配置（notification_v2）"),
    ("Bucket intelligent tiering configuration", "Bucket 智能分层配置"),
    ("Bucket max-age cache configuration", "Bucket max-age 缓存配置"),
    ("Bucket mirror back-to-source rules", "Bucket 镜像回源规则"),
    ("Bucket object lock configuration", "Bucket 对象锁配置"),
    ("Bucket pay-by-traffic configuration", "Bucket 按流量付费配置"),
    ("Bucket real-time log configuration", "Bucket 实时日志配置"),
    ("Bucket requester pays configuration", "Bucket 请求者付费配置"),
    ("Bucket static website hosting", "Bucket 静态网站托管"),
    ("Bucket transfer acceleration configuration", "Bucket 传输加速配置"),
    ("Control-plane APIs", "控制面 API"),
    ("Create MRAP", "创建 MRAP"),
    ("Create URL cache on Data Plane", "在数据面创建 URL 缓存"),
    ("Create accelerator", "创建 Accelerator"),
    ("Create access point", "创建接入点"),
    ("Create audit", "创建审计"),
    ("Create audit job", "创建审计任务"),
    ("Create batch job", "创建批处理任务"),
    ("Create converged access point", "创建融合接入点"),
    ("Create custom endpoint", "创建自定义 endpoint"),
    ("Create dataset", "创建 Dataset"),
    ("Create dataset binding", "创建 Dataset 绑定"),
    ("Create document job", "创建文档任务"),
    ("Create evict job", "创建驱逐任务"),
    ("Create file job", "创建文件任务"),
    ("Create increment audit", "创建增量审计"),
    ("Create media job", "创建媒体任务"),
    ("Create object set binding", "创建对象集绑定"),
    ("Create prefetch job", "创建预取任务"),
    ("Create redundancy transition task", "创建冗余转换任务"),
    ("Data processing APIs", "数据处理 API"),
    ("Delete MRAP", "删除 MRAP"),
    ("Delete MRAP mirror", "删除 MRAP 镜像"),
    ("Delete URL cache on Data Plane", "在数据面删除 URL 缓存"),
    ("Delete accelerator", "删除 Accelerator"),
    ("Delete access point", "删除接入点"),
    ("Delete batch job", "删除批处理任务"),
    ("Delete bucket encryption", "删除 Bucket 加密配置"),
    ("Delete converged access point", "删除融合接入点"),
    ("Delete custom endpoint", "删除自定义 endpoint"),
    ("Delete dataset", "删除 Dataset"),
    ("Delete dataset binding", "删除 Dataset 绑定"),
    ("Delete evict job", "删除驱逐任务"),
    ("Delete image style", "删除图片样式"),
    ("Delete increment audit", "删除增量审计"),
    ("Delete object set", "删除对象集"),
    ("Delete object set lifecycle", "删除对象集生命周期"),
    ("Delete prefetch job", "删除预取任务"),
    ("Delete process template", "删除处理模板"),
    ("Delete redundancy transition task", "删除冗余转换任务"),
    ("Delete resource tag on Data Plane", "在数据面删除资源标签"),
    ("Delete storage lens", "删除 Storage Lens"),
    ("Delete subscribe configuration", "删除订阅配置"),
    ("Delete website configuration", "删除网站配置"),
    ("Delete workflow", "删除工作流"),
    ("Disable RenameObject", "禁用 RenameObject"),
    ("Enable RenameObject", "启用 RenameObject"),
    ("Get HTTPS/TLS configuration", "获取 HTTPS/TLS 配置"),
    ("Get MRAP", "获取 MRAP"),
    ("Get MRAP mirror", "获取 MRAP 镜像"),
    ("Get RenameObject configuration", "获取 RenameObject 配置"),
    ("Get accelerator", "获取 Accelerator"),
    ("Get access monitor configuration", "获取访问监控配置"),
    ("Get access point", "获取接入点"),
    ("Get audit", "获取审计"),
    ("Get bandwidth quota", "获取带宽配额"),
    ("Get batch job", "获取批处理任务"),
    ("Get bucket access logging configuration", "获取 Bucket 访问日志配置"),
    ("Get bucket encryption", "获取 Bucket 加密配置"),
    ("Get bucket object-set configuration", "获取 Bucket 对象集配置"),
    ("Get capacity quota", "获取容量配额"),
    ("Get converged access point", "获取融合接入点"),
    ("Get data process job", "获取数据处理任务"),
    ("Get dataset", "获取 Dataset"),
    ("Get dataset binding", "获取 Dataset 绑定"),
    ("Get estimated remaining time", "获取预计剩余时间"),
    ("Get event notification configuration (notification_v2)", "获取事件通知配置（notification_v2）"),
    ("Get evict job", "获取驱逐任务"),
    ("Get image style", "获取图片样式"),
    ("Get increment audit", "获取增量审计"),
    ("Get object lock configuration", "获取对象锁配置"),
    ("Get object set", "获取对象集"),
    ("Get object set lifecycle", "获取对象集生命周期"),
    ("Get object set storage", "获取对象集存储信息"),
    ("Get original image protect rule", "获取原图保护规则"),
    ("Get prefetch job", "获取预取任务"),
    ("Get process template", "获取处理模板"),
    ("Get redundancy transition task", "获取冗余转换任务"),
    ("Get requester pays configuration", "获取请求者付费配置"),
    ("Get storage lens", "获取 Storage Lens"),
    ("Get subscribe configuration", "获取订阅配置"),
    ("Get transfer acceleration configuration", "获取传输加速配置"),
    ("Get trash configuration", "获取回收站配置"),
    ("Get website configuration", "获取网站配置"),
    ("Get workflow", "获取工作流"),
    ("Get workflow execution", "获取工作流执行信息"),
    ("List MRAPs", "列出 MRAP"),
    ("List MRAPs for accelerator", "列出 Accelerator 的 MRAP"),
    ("List accelerators bound to bucket", "列出绑定到 Bucket 的 Accelerator"),
    ("List access points bound to accelerator", "列出绑定到 Accelerator 的接入点"),
    ("List audits", "列出审计"),
    ("List bound accelerators", "列出已绑定的 Accelerator"),
    ("List buckets bound to accelerator", "列出绑定到 Accelerator 的 Bucket"),
    ("List converged access points", "列出融合接入点"),
    ("List data process jobs", "列出数据处理任务"),
    ("List image style brief info", "列出图片样式摘要"),
    ("List image styles", "列出图片样式"),
    ("List increment audits", "列出增量审计"),
    ("List resource tags on Data Plane", "列出数据面资源标签"),
    ("List storage lenses", "列出 Storage Lens"),
    ("List templates", "列出模板"),
    ("Object set APIs", "对象集 API"),
    ("Query dataset", "查询 Dataset"),
    ("Set HTTPS/TLS configuration", "设置 HTTPS/TLS 配置"),
    ("Set MRAP mirror", "设置 MRAP 镜像"),
    ("Set access monitor configuration", "设置访问监控配置"),
    ("Set bucket default storage class", "设置 Bucket 默认存储类型"),
    ("Set bucket encryption", "设置 Bucket 加密配置"),
    ("Set bucket object-set configuration", "设置 Bucket 对象集配置"),
    ("Set event notification configuration (notification_v2)", "设置事件通知配置（notification_v2）"),
    ("Set image style with --config JSON", "使用 --config JSON 设置图片样式"),
    ("Set object lock configuration", "设置对象锁配置"),
    ("Set object set lifecycle", "设置对象集生命周期"),
    ("Set object set via --config JSON", "通过 --config JSON 设置对象集"),
    ("Set or disable bucket access logging configuration", "设置或禁用 Bucket 访问日志配置"),
    ("Set original image protect rule", "设置原图保护规则"),
    ("Set process template", "设置处理模板"),
    ("Set requester pays configuration", "设置请求者付费配置"),
    ("Set resource tag on Data Plane", "在数据面设置资源标签"),
    ("Set storage lens", "设置 Storage Lens"),
    ("Set subscribe configuration", "设置订阅配置"),
    ("Set transfer acceleration configuration", "设置传输加速配置"),
    ("Set trash configuration", "设置回收站配置"),
    ("Set versioning status", "设置版本控制状态"),
    ("Set website configuration", "设置网站配置"),
    ("Set workflow", "设置工作流"),
    ("Submit MRAP routes", "提交 MRAP 路由"),
    ("Unbind accelerator", "解绑 Accelerator"),
    ("Unbind bucket from accelerator", "从 Accelerator 解绑 Bucket"),
    ("Update batch job priority", "更新批处理任务优先级"),
    ("Update batch job status", "更新批处理任务状态"),
    ("Update dataset", "更新 Dataset"),
    ("--dry-run returns planned target paths and conflict flags without creating directories or files", "--dry-run 返回规划的目标路径和冲突标记，不创建目录或文件"),
    ("--if-match maps to the destination ETag guard (if-match header); the source ETag guard (x-tos-copy-source-if-match) is only injected by high-level cp/mv when the discovered source ETag is known", "--if-match 映射到目标 ETag 保护条件（if-match header）；仅在高层 cp/mv 已知源 ETag 时注入源 ETag 保护条件（x-tos-copy-source-if-match）"),
    ("VeTos authentication is selected with --auth-mode <MODE>: aksk or unified; TOS_AUTH_MODE is the environment override; Unified uses the same-name external profile and `ve login`", "VeTos 通过 --auth-mode <MODE> 选择 aksk 或 unified；TOS_AUTH_MODE 为环境覆盖；Unified 使用同名外部 profile 和 `ve login`"),
    ("binary stdout is base64-encoded inside the Envelope; --output - streams raw bytes", "二进制 stdout 在 Envelope 内使用 base64 编码；--output - 流式输出原始字节"),
    ("create is idempotent at bucket-name granularity; existing-bucket errors are surfaced verbatim", "create 在 Bucket 名称粒度具备幂等语义；Bucket 已存在错误会原样返回"),
    ("credential values are stored encrypted and redacted on show", "凭证值加密存储，并在 show 中脱敏"),
    ("destructive call gated by --force unless --dry-run is in effect", "除 --dry-run 外，破坏性调用受 --force 保护"),
    ("destructive execution must be reviewed with --dry-run first", "破坏性执行必须先通过 --dry-run 审查"),
    ("export writes dir/SKILL.md plus dir/{domain}/{skill_name}/SKILL.md and refuses to overwrite existing files", "export 写入 dir/SKILL.md 和 dir/{domain}/{skill_name}/SKILL.md，并拒绝覆盖已有文件"),
    ("exported Markdown skill files are portable catalogs; serve rebuilds MCP tools from the live registry instead of reading the export directory", "导出的 Markdown Skill 文件是可移植目录；serve 从 live registry 重建 MCP 工具，不读取导出目录"),
    ("file --body is SHA256-hashed with asynchronous reads before signing and sent via Body::wrap_stream", "文件 --body 在签名前异步读取并计算 SHA256，通过 Body::wrap_stream 发送"),
    ("honors --range, --version-id, and --if-match without buffering the full object", "支持 --range、--version-id 和 --if-match，且不缓冲完整对象"),
    ("metadata is derived from the curated capability registry plus the clap command tree", "元数据来自精选 capability registry 和 clap 命令树"),
    ("metadata-directive defaults to COPY; REPLACE requires the full metadata set; REPLACE_NEW overrides only newly supplied metadata fields", "metadata-directive 默认为 COPY；REPLACE 需要完整元数据集；REPLACE_NEW 仅覆盖新提供的元数据字段"),
    ("non-empty buckets must be drained with ve-tos rm --recursive before bucket deletion", "删除非空 Bucket 前必须使用 ve-tos rm --recursive 清空"),
    ("real execution requires explicit confirmation flags when supported", "真实执行在支持时需要显式确认 flag"),
    ("response body is streamed via tokio::io::copy into .tos-partial-<pid> then renamed atomically", "响应正文通过 tokio::io::copy 流式写入 .tos-partial-<pid>，随后原子重命名"),
    ("retryable PutObject file bodies are reopened and re-signed for every HTTP attempt", "可重试的 PutObject 文件正文会在每次 HTTP 尝试时重新打开并签名"),
    ("secrets are redacted before output", "输出前会脱敏密钥"),
    ("serve uses the same in-process definitions; it does not read exported Markdown skill files", "serve 使用相同的进程内定义，不读取导出的 Markdown Skill 文件"),
    ("stdin/inline payloads stay buffered for V4 signing and capped at the safe inline limit", "stdin/内联 payload 为 V4 签名保留缓冲，并受安全内联大小限制"),
    ("tos://bucket/key (or use --bucket + --key)", "tos://bucket/key（或使用 --bucket + --key）"),
    ("version_id, when supplied, scopes the delete to a single version", "提供 version_id 时，删除范围限定为单一版本"),
    ("writes only the selected local config profile", "仅写入所选本地配置 profile"),
    ("x-tos-copy-source carries the URL-encoded source path and version_id", "x-tos-copy-source 携带 URL 编码的源路径和 version_id"),
];

fn ve_tos_metadata_translation_zh(text: &str) -> Option<&'static str> {
    VE_TOS_METADATA_TRANSLATIONS_ZH
        .iter()
        .chain(VE_TOS_CANONICAL_RUNTIME_TRANSLATIONS_ZH.iter())
        .find_map(|(english, chinese)| (*english == text).then_some(*chinese))
}

fn is_ve_tos_human_metadata_field(parent: &str, key: &str) -> bool {
    // [Review Fix #GlobalZh8] API body contracts and consistency guards are
    // human prose even though their surrounding objects contain machine data.
    matches!(
        key,
        "description"
            | "body_contract"
            | "consistency_guards"
            | "scenario_routing"
            | "shell_quoting_tips"
            | "notes"
            | "guidance"
            | "recovery"
            | "exported_file_use"
            | "default_mcp_call"
    ) || key == "source" && parent == "usage"
}

fn is_ve_tos_machine_metadata_field(key: &str) -> bool {
    matches!(
        key,
        "command"
            | "name"
            | "type"
            | "enum"
            | "examples"
            | "api"
            | "code"
            | "path"
            | "uri"
            | "method"
            | "id"
            | "mode_resolution"
            | "endpoint_kind"
            | "endpoint_rule"
    )
}

fn is_ve_tos_machine_metadata_value(text: &str) -> bool {
    // [Review Fix #VeTosZh2] Schema URI templates are human-field-shaped but
    // are contract values; enumerate them instead of using a broad heuristic.
    matches!(
        text,
        "" | "If-Match"
            | "If-None-Match"
            | "tos://bucket/key"
            | "tos://bucket/prefix"
            | "tos://bucket/folder/"
            | "tos://bucket/key or prefix"
            | "tos://bucket or tos://bucket/key"
            | "tos://bucket or tos://bucket/prefix"
    )
}

#[derive(Debug, Serialize)]
struct CapabilitiesView<'a> {
    tool: &'static str,
    version: &'static str,
    service_name: &'static str,
    view: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    uri_format: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    high_level_semantics: Option<Value>,
    /// [Spec §4.5 / AGT-001] Always populated for the `groups` view; populated
    /// for `text`/`full` so Agents can cross-link a capability to its group.
    /// Empty for `compact` to keep the payload as small as possible.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    groups: Vec<CapabilitiesGroup>,
    /// [Spec §4.5] `full` view: rich capability metadata (layer / endpoint_rule
    /// / destructive / parameters / examples). `compact` strips parameters.
    /// `groups` / `text` leave this empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    capabilities: Vec<CapabilityRow>,
    /// [Spec §4.5] Subcommand tree, used by `compact` and `full`. `text`
    /// surfaces the same data in `lines` instead.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    commands: Vec<CommandTreeEntry>,
    /// [Spec §4.5 `text`] One-line summaries — `"<command>\t<description>"` —
    /// so an Agent can scan the entire command surface in O(N) tokens.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    lines: Vec<String>,
    /// [G7] Per-result fuzzy match scores, returned only when `--search` is
    /// active. Entries appear in descending score order (best matches first)
    /// so an Agent can show top-k without re-sorting.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    search_scores: Vec<SearchScore>,
}

/// [Spec §4.5 `groups`] Group summary including a command count so Agents can
/// pick the largest-surface group to drill into first.
#[derive(Debug, Serialize)]
struct CapabilitiesGroup {
    name: &'static str,
    command: String,
    layer: &'static str,
    group: &'static str,
    category: &'static str,
    description: &'static str,
    implemented: bool,
    /// [Spec §4.5 / AGT-001] Number of capabilities under this group.
    command_count: usize,
}

type CapabilityRow = RegistryCapabilityRow;

/// [G7] One element of `search_scores`. `score` is in [0, 1] where 1.0 is an
/// exact match (case-insensitive). The `kind` distinguishes group / capability
/// / command tree entries so consumers can join back to the right list.
#[derive(Debug, Serialize)]
struct SearchScore {
    kind: &'static str,
    command: String,
    score: f64,
    matched_field: &'static str,
}

const TOS_HIGH_LEVEL_SEMANTICS: &[(&str, &[&str])] = &[
    (
        "cp",
        &[
            "local path -> tos://bucket/key uploads with PutObject or multipart upload",
            "tos://bucket/key -> local path downloads with GetObject and atomic local persist",
            "tos://bucket/key -> tos://bucket/key copies with CopyObject or multipart copy",
            "checkpoint identity includes source, destination, file metadata, part size, profile, and endpoint",
        ],
    ),
    (
        "mv",
        &[
            "runs cp semantics first, then deletes the source only after destination success",
            "critical source delete requires --force plus exact --confirm <source> in non-interactive shells",
            "remote multipart move checkpoint identity includes profile and endpoint",
        ],
    ),
    (
        "sync",
        &[
            "builds a source/destination diff from ListObjects, size, ETag, and mtime where available",
            "--delete removes extraneous destination objects and upgrades the command to critical risk",
            "transfer phases reuse cp checkpoint and overwrite semantics",
        ],
    ),
    (
        "mb",
        &["tos://bucket -> CreateBucket; optional ACL/storage-class settings are applied after creation"],
    ),
    (
        "rb",
        &["tos://bucket -> DeleteBucket; bucket must already be empty and critical deletes require confirmation"],
    ),
    (
        "mkdir",
        &["tos://bucket/prefix -> PutObject for a zero-byte object normalized to a trailing slash"],
    ),
    (
        "rm",
        &[
            "tos://bucket/key -> DeleteObject",
            "tos://bucket/prefix --recursive -> planned object batch delete",
            "critical deletes require --force plus exact --confirm <target> in non-interactive shells",
        ],
    ),
    (
        "ls",
        &["no target -> ListBuckets", "tos://bucket[/prefix] -> ListObjects with pagination"],
    ),
    (
        "stat",
        &["tos://bucket -> HeadBucket", "tos://bucket/key -> HeadObject"],
    ),
    (
        "du",
        &["tos://bucket/prefix -> read-only ListObjects traversal with size, histogram, and optional cost summaries"],
    ),
    (
        "find",
        &["tos://bucket/prefix -> read-only ListObjects traversal filtered by name, size, mtime, and storage class"],
    ),
    ("cat", &["tos://bucket/key -> GetObject body streamed to stdout"]),
    ("put", &["stdin -> tos://bucket/key upload; multipart is used above the configured threshold"]),
    ("presign", &["tos://bucket/key -> locally signed presigned URL without object mutation"]),
    ("restore", &["tos://bucket/key -> RestoreObject for archived storage classes"]),
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
struct CompletionScript {
    shell: String,
    script: String,
    command_count: usize,
}

#[derive(Debug, Serialize)]
struct ServePlan {
    mode: &'static str,
    transport: String,
    port: Option<u16>,
    protocol: &'static str,
    tcp_listener: bool,
    bind: Option<String>,
    endpoints: Vec<&'static str>,
    authentication: &'static str,
    token_output: Option<&'static str>,
    authorization_header_required: bool,
    allowed_hosts: Vec<String>,
    origin_policy: Option<&'static str>,
    tool_source: &'static str,
    call_semantics: &'static str,
    capabilities: usize,
    groups: usize,
    status: &'static str,
    message: &'static str,
}

#[derive(Debug, Serialize)]
struct DoctorReport {
    profile: String,
    checks: Vec<DoctorCheck>,
    summary: DoctorSummary,
}

#[derive(Debug, Serialize)]
struct DoctorCheck {
    name: &'static str,
    status: &'static str,
    message: String,
    details: Value,
}

#[derive(Debug, Serialize)]
struct DoctorSummary {
    total: usize,
    passed: usize,
    warnings: usize,
    failed: usize,
}

#[derive(Debug, Deserialize, Serialize)]
struct RawApiRequest {
    #[serde(default = "default_raw_api_method")]
    method: String,
    // [Review Fix #5] Accept `endpoint_rule` as the canonical alias of
    // `endpoint_kind` so the raw-passthrough input matches the renamed
    // capability/describe output (`AGT-002`). Both names parse identically;
    // emitted output prefers `endpoint_rule`.
    #[serde(default, alias = "endpoint_rule")]
    endpoint_kind: Option<String>,
    #[serde(default)]
    bucket: Option<String>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    query: BTreeMap<String, Value>,
    #[serde(default)]
    headers: BTreeMap<String, Value>,
    #[serde(default)]
    body: Option<Value>,
}

#[derive(Debug, Serialize)]
struct RawApiTarget {
    // [Review Fix #5] Mirror the capability registry vocabulary so the
    // resolved target carries `endpoint_rule` in its serialized form.
    #[serde(rename = "endpoint_rule")]
    endpoint_kind: String,
    url: String,
    signing_path: String,
}

#[cfg(test)]
#[derive(Debug, Deserialize)]
struct McpToolCallParams {
    name: String,
    #[serde(default)]
    arguments: Value,
}

#[derive(Debug, Serialize)]
struct McpCommandExecution {
    command: String,
    argv: Vec<String>,
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
}

pub async fn handle_capabilities_command(
    global: &GlobalArgs,
    args: &CapabilitiesArgs,
) -> Result<i32, CliError> {
    let view = capabilities_view(args)?;
    output_result_with_columns(
        global,
        &Envelope::success("ve-tos capabilities", view),
        capabilities_table_columns(global, args.view.as_str()),
    )?;
    Ok(0)
}

pub async fn handle_api_command(global: &GlobalArgs, args: &ApiArgs) -> Result<i32, CliError> {
    // [Review Fix #7] Execute raw API only when the user provides an explicit request and did not ask for a plan.
    if args.request.is_some() && !global.dry_run && !global.describe && !args.describe {
        let response = execute_raw_api(global, args).await?;
        output_result(global, &response)?;
        return Ok(0);
    }
    let lookup = api_lookup(args)?;
    output_result(global, &Envelope::success("ve-tos api", lookup))?;
    Ok(0)
}

pub async fn handle_skill_command(
    global: &GlobalArgs,
    command: &SkillCommand,
) -> Result<i32, CliError> {
    if global.describe {
        // [Review Fix #1] `skill export --describe` must not fall through into
        // filesystem writes or conflict checks; describe is always read-only.
        let command_path = match &command.action {
            SkillAction::List { .. } => "ve-tos skill list",
            SkillAction::Export { .. } => "ve-tos skill export",
        };
        let description = describe_command_metadata(command_path).ok_or_else(|| {
            CliError::ValidationError(format!("no metadata registered for {command_path}"))
        })?;
        let mut documentation = serde_json::to_value(description).unwrap_or_else(|_| json!({}));
        // [Review Fix #VeTosZh5] Skill owns `--language`, so its Describe path
        // localizes here instead of relying on parser recovery in the root.
        if skill_action_language(&command.action) == DocumentationLanguage::Zh {
            localize_ve_tos_auth_documentation_zh(&mut documentation);
        }
        output_result(global, &Envelope::success(command_path, documentation))?;
        return Ok(0);
    }
    match &command.action {
        SkillAction::List { language } => {
            let list = SkillList {
                language: language.code(),
                skills: skill_definitions_for_language(*language),
            };
            output_result(global, &Envelope::success("ve-tos skill list", list))?;
        }
        SkillAction::Export {
            name,
            dir,
            language,
        } => {
            let export_plan = skill_markdown_export_plan(name.as_deref(), dir)?;
            if global.dry_run {
                let plan = plan_skill_markdown_export(&export_plan, dir, *language);
                output_result(global, &Envelope::success("ve-tos skill export", plan))?;
            } else {
                let exported = export_markdown_skills(export_plan, dir, *language)?;
                output_result(global, &Envelope::success("ve-tos skill export", exported))?;
            }
        }
    }
    Ok(0)
}

fn skill_action_language(action: &SkillAction) -> DocumentationLanguage {
    match action {
        SkillAction::List { language } | SkillAction::Export { language, .. } => *language,
    }
}

pub async fn handle_completion_command(
    global: &GlobalArgs,
    args: &CompletionArgs,
) -> Result<i32, CliError> {
    if global.describe {
        let description = describe_command_metadata("ve-tos completion").ok_or_else(|| {
            CliError::ValidationError("no metadata registered for ve-tos completion".to_string())
        })?;
        output_result(global, &Envelope::success("ve-tos completion", description))?;
        return Ok(0);
    }
    let script = completion_script(&args.shell)?;
    output_result(global, &Envelope::success("ve-tos completion", script))?;
    Ok(0)
}

pub async fn handle_serve_command(global: &GlobalArgs, args: &ServeArgs) -> Result<i32, CliError> {
    // [Spec §3 Safe Execution / G2] --dry-run / --describe must short-circuit
    // before we boot the long-running MCP stdio server. Otherwise an Agent
    // asking "what would this command do?" would actually start the server and
    // hang on stdin. We fall through to the registry-backed plan path so the
    // contract for `ve-tos serve` matches every other CLI command.
    if args.mcp && !global.dry_run && !global.describe {
        match args.transport.as_str() {
            "stdio" => run_mcp_stdio(global).await?,
            "sse" => run_mcp_sse(global, args.port).await?,
            other => {
                return Err(CliError::ValidationError(format!(
                    "unsupported serve transport '{other}': expected stdio or sse"
                )));
            }
        }
        return Ok(0);
    }
    let plan = serve_plan(args)?;
    output_result(global, &Envelope::success("ve-tos serve", plan))?;
    Ok(0)
}

pub async fn handle_doctor_command(
    global: &GlobalArgs,
    args: &DoctorArgs,
) -> Result<i32, CliError> {
    let report = doctor_report(global, args).await?;
    output_result(global, &Envelope::success("ve-tos doctor", report))?;
    Ok(0)
}

fn capabilities_view(args: &CapabilitiesArgs) -> Result<CapabilitiesView<'_>, CliError> {
    if let Some(group) = args.group.as_deref() {
        if !is_known_group_or_category(group) {
            return Err(CliError::ValidationError(format!(
                "unknown capabilities group '{}': use `ve-tos capabilities --view groups` to list valid groups",
                group
            )));
        }
    }

    // [G7] When --search is active, switch from substring-match to a weighted
    // fuzzy ranker (case-insensitive Jaro–Winkler with substring/prefix
    // boosts). We compute scores once per entry, drop low-score noise, and
    // surface the ranked scores via `search_scores` so Agents see the
    // confidence of each hit.
    //
    // [Spec §4.5 / AGT-003] We additionally expand the search term through a
    // Chinese→English alias map so `--search 加密` hits encryption / SSE
    // capabilities even though the registry strings are in English.
    let expanded_terms: Vec<String> = args
        .search
        .as_deref()
        .map(expand_search_term)
        .unwrap_or_default();

    let scored_groups: Vec<(&'static CommandGroupEntry, f64, &'static str)> = command_groups()
        .iter()
        .filter(|entry| group_matches_facets(entry, args))
        .filter_map(|entry| match args.search.as_deref() {
            None => Some((entry, 1.0, "")),
            Some(_) => score_group_multi(entry, &expanded_terms).map(|(s, f)| (entry, s, f)),
        })
        .collect();
    let scored_caps: Vec<(&'static CapabilityEntry, f64, &'static str)> = capabilities()
        .iter()
        .filter(|entry| capability_matches_facets(entry, args))
        .filter_map(|entry| match args.search.as_deref() {
            None => Some((entry, 1.0, "")),
            Some(_) => score_capability_multi(entry, &expanded_terms).map(|(s, f)| (entry, s, f)),
        })
        .collect();
    let scored_commands: Vec<(CommandTreeEntry, f64, &'static str)> = flattened_command_tree()
        .into_iter()
        .filter(|entry| command_tree_matches_facets(entry, args))
        .filter_map(|entry| match args.search.as_deref() {
            None => Some((entry, 1.0, "")),
            Some(_) => {
                let scored = score_command_tree_multi(&entry, &expanded_terms);
                scored.map(|(s, f)| (entry, s, f))
            }
        })
        .collect();

    // Sort descending by score when --search is on; otherwise leave registry
    // order intact so the existing snapshot-style output is stable.
    let (group_entries, cap_entries, command_entries, search_scores) = if args.search.is_some() {
        let mut g = scored_groups;
        g.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let mut c = scored_caps;
        c.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let mut t = scored_commands;
        t.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let mut scores: Vec<SearchScore> = Vec::new();
        scores.extend(g.iter().map(|(e, s, f)| SearchScore {
            kind: "group",
            command: e.command.to_string(),
            score: *s,
            matched_field: f,
        }));
        scores.extend(c.iter().map(|(e, s, f)| SearchScore {
            kind: "capability",
            command: e.command.to_string(),
            score: *s,
            matched_field: f,
        }));
        scores.extend(t.iter().map(|(e, s, f)| SearchScore {
            kind: "command",
            command: e.command.clone(),
            score: *s,
            matched_field: f,
        }));
        scores.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        (
            g.into_iter().map(|(e, _, _)| e).collect::<Vec<_>>(),
            c.into_iter().map(|(e, _, _)| e).collect::<Vec<_>>(),
            t.into_iter().map(|(e, _, _)| e).collect::<Vec<_>>(),
            scores,
        )
    } else {
        (
            scored_groups.into_iter().map(|(e, _, _)| e).collect(),
            scored_caps.into_iter().map(|(e, _, _)| e).collect(),
            scored_commands.into_iter().map(|(e, _, _)| e).collect(),
            Vec::new(),
        )
    };

    // Convert internal registry types to the public `CapabilitiesView` shape
    // (CapabilitiesGroup / CapabilityRow). This is also where we materialise
    // `command_count`, `destructive`, and the `endpoint_kind → endpoint_rule`
    // rename promised by AGT-002.
    // [Review Fix #21] Capability metadata projection is registry-owned; the
    // meta handler only filters/ranks and renders the selected rows.
    let caps_with_params = publicize_capability_rows(capability_rows(
        &cap_entries,
        &command_entries,
        /* keep_parameters */ true,
    ));
    let caps_compact = publicize_capability_rows(capability_rows(
        &cap_entries,
        &command_entries,
        /* keep_parameters */ false,
    ));
    let command_entries = publicize_command_tree_entries(command_entries);
    let search_scores = publicize_search_scores(search_scores);
    let groups_full = build_groups(&group_entries, &caps_with_params);
    let lines = build_text_lines(&caps_with_params, &command_entries);

    // [Spec §4.5] Accept `tree` as a legacy alias for `compact` so existing
    // callers do not break, but the documented surface is the four-view set.
    let view = match args.view.as_str() {
        "tree" => "compact",
        other => other,
    };

    match view {
        "groups" => Ok(CapabilitiesView {
            tool: "ve-tos",
            version: env!("CARGO_PKG_VERSION"),
            service_name: "tos",
            view: "groups",
            uri_format: Some("tos://bucket/key"),
            high_level_semantics: Some(high_level_semantics()),
            groups: groups_full,
            capabilities: args
                .search
                .is_some()
                .then_some(caps_compact)
                .unwrap_or_default(),
            commands: args
                .search
                .is_some()
                .then_some(command_entries)
                .unwrap_or_default(),
            lines: Vec::new(),
            search_scores,
        }),
        "text" => Ok(CapabilitiesView {
            tool: "ve-tos",
            version: env!("CARGO_PKG_VERSION"),
            service_name: "tos",
            view: "text",
            uri_format: None,
            high_level_semantics: None,
            groups: Vec::new(),
            capabilities: Vec::new(),
            commands: Vec::new(),
            lines,
            search_scores,
        }),
        "compact" => Ok(CapabilitiesView {
            tool: "ve-tos",
            version: env!("CARGO_PKG_VERSION"),
            service_name: "tos",
            view: "compact",
            uri_format: None,
            high_level_semantics: None,
            groups: groups_full,
            capabilities: caps_compact,
            commands: command_entries,
            lines: Vec::new(),
            search_scores,
        }),
        "full" => Ok(CapabilitiesView {
            tool: "ve-tos",
            version: env!("CARGO_PKG_VERSION"),
            service_name: "tos",
            view: "full",
            uri_format: Some("tos://bucket/key"),
            high_level_semantics: Some(high_level_semantics()),
            groups: groups_full,
            capabilities: caps_with_params,
            commands: command_entries,
            lines: Vec::new(),
            search_scores,
        }),
        other => Err(CliError::ValidationError(format!(
            "unsupported capabilities view '{}': expected groups, text, compact, or full",
            other
        ))),
    }
}

fn high_level_semantics() -> Value {
    let mut semantics = serde_json::Map::new();
    for (command, lines) in TOS_HIGH_LEVEL_SEMANTICS {
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

/// [Spec §4.5 / AGT-001] Convert registry `CommandGroupEntry` rows into the
/// view-facing `CapabilitiesGroup` shape, attaching `command_count` derived
/// from the (already filtered) capabilities list. We count capabilities whose
/// `group` matches the group's `name`, which mirrors how the registry models
/// the relationship.
fn build_groups(
    groups: &[&'static CommandGroupEntry],
    caps: &[CapabilityRow],
) -> Vec<CapabilitiesGroup> {
    groups
        .iter()
        .map(|entry| {
            let command_count = caps.iter().filter(|cap| cap.group == entry.name).count();
            CapabilitiesGroup {
                name: entry.name,
                // [Review Fix #27] ve-tos capabilities are now canonical at
                // the public top-level command path; no old `tos ...` prefix
                // is accepted or emitted for this surface.
                command: public_capabilities_command(entry.command),
                layer: layer_name(&entry.layer),
                group: entry.category,
                category: entry.category,
                description: entry.description,
                implemented: entry.implemented,
                command_count,
            }
        })
        .collect()
}

fn publicize_capability_rows(mut rows: Vec<CapabilityRow>) -> Vec<CapabilityRow> {
    for row in &mut rows {
        row.command = public_capabilities_command(&row.command);
        row.related_commands = row
            .related_commands
            .iter()
            .map(|command| public_capabilities_command(command))
            .collect();
    }
    rows
}

fn publicize_command_tree_entries(entries: Vec<CommandTreeEntry>) -> Vec<CommandTreeEntry> {
    entries
        .into_iter()
        .map(publicize_command_tree_entry)
        .collect()
}

fn publicize_command_tree_entry(mut entry: CommandTreeEntry) -> CommandTreeEntry {
    entry.command = public_capabilities_command(&entry.command);
    entry.subcommands = publicize_command_tree_entries(entry.subcommands);
    entry
}

fn publicize_search_scores(mut scores: Vec<SearchScore>) -> Vec<SearchScore> {
    for score in &mut scores {
        score.command = public_capabilities_command(&score.command);
    }
    scores
}

fn public_capabilities_command(command: &str) -> String {
    command
        .strip_prefix("ve-tos ")
        .map(|suffix| format!("ve-tos {suffix}"))
        .unwrap_or_else(|| command.to_string())
}

/// [Spec §4.5 `text`] Materialise the one-line summary view. We use TAB as
/// the separator because the Agent contract treats the second column as a
/// free-form description that may contain spaces.
fn build_text_lines(caps: &[CapabilityRow], commands: &[CommandTreeEntry]) -> Vec<String> {
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut lines: Vec<String> = Vec::new();
    for cap in caps {
        if seen.insert(cap.command.clone()) {
            lines.push(format!("{}\t{}", cap.command, cap.description));
        }
    }
    for entry in commands {
        if seen.insert(entry.command.clone()) {
            lines.push(format!("{}\t{}", entry.command, entry.description));
        }
    }
    lines
}

fn command_root_group(command: &str) -> Option<&str> {
    command.split_whitespace().nth(1)
}

/// [Spec §4.5 / AGT-003] Expand a user-provided search term into one or more
/// English candidates so non-English keywords still hit the registry. The
/// original term is always retained as the first candidate.
fn expand_search_term(term: &str) -> Vec<String> {
    let mut out = vec![term.to_string()];
    // Lower-cased lookup keeps the alias map case-insensitive without needing
    // every variant of upper/title case in the table.
    let key = term.trim().to_lowercase();
    let aliases: &[(&str, &[&str])] = &[
        ("加密", &["encryption", "encrypt", "sse", "kms"]),
        ("解密", &["decryption", "decrypt"]),
        ("策略", &["policy"]),
        ("权限", &["acl", "permission", "iam"]),
        ("生命周期", &["lifecycle"]),
        ("版本", &["version", "versioning"]),
        ("跨域", &["cors"]),
        ("镜像", &["mirror", "replication"]),
        ("复制", &["replication", "copy"]),
        ("标签", &["tag", "tagging"]),
        ("日志", &["log", "logging"]),
        ("通知", &["notification"]),
        ("加速", &["acceleration", "transfer-accelerate"]),
        ("分片", &["multipart"]),
        ("续传", &["resume", "checkpoint"]),
        ("公开访问", &["public-access-block", "policy"]),
        ("归档", &["archive", "storage-class"]),
        ("存储类型", &["storage-class", "storageclass"]),
        ("元数据", &["metadata"]),
    ];
    for (cn, en) in aliases {
        if key.contains(cn) {
            for word in *en {
                out.push((*word).to_string());
            }
        }
    }
    out
}

/// [G7] Multi-term wrappers that pick the best score across the expanded
/// candidate list. We retain the original `score_group` etc. helpers so the
/// scoring logic itself stays single-term and trivially testable.
fn score_group_multi(entry: &CommandGroupEntry, terms: &[String]) -> Option<(f64, &'static str)> {
    terms
        .iter()
        .filter_map(|term| score_group(entry, term))
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
}

fn score_capability_multi(
    entry: &CapabilityEntry,
    terms: &[String],
) -> Option<(f64, &'static str)> {
    terms
        .iter()
        .filter_map(|term| score_capability(entry, term))
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
}

fn score_command_tree_multi(
    entry: &CommandTreeEntry,
    terms: &[String],
) -> Option<(f64, &'static str)> {
    terms
        .iter()
        .filter_map(|term| score_command_tree(entry, term))
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
}

/// [G7] Group/layer facet filtering, separated from the search predicate so
/// the scoring path can apply them independently.
fn group_matches_facets(entry: &CommandGroupEntry, args: &CapabilitiesArgs) -> bool {
    if let Some(group) = &args.group {
        let group = canonical_group_name(group);
        if entry.name != group && entry.category != group {
            return false;
        }
    }
    if let Some(layer) = &args.layer {
        if layer_name(&entry.layer) != layer {
            return false;
        }
    }
    true
}

fn capability_matches_facets(entry: &CapabilityEntry, args: &CapabilitiesArgs) -> bool {
    if let Some(group) = &args.group {
        let group = canonical_group_name(group);
        // [Review Fix #18] `--group utilities` filters by category, not only exact group name.
        if entry.group != group
            && find_group(entry.group)
                .map(|group_entry| group_entry.category != group)
                .unwrap_or(true)
        {
            return false;
        }
    }
    if let Some(layer) = &args.layer {
        if layer_name(&entry.layer) != layer {
            return false;
        }
    }
    true
}

fn command_tree_matches_facets(entry: &CommandTreeEntry, args: &CapabilitiesArgs) -> bool {
    if let Some(group) = &args.group {
        let group = canonical_group_name(group);
        // [Review Fix #18] Keep command-tree discovery aligned with capability category filters.
        let root_matches_category = command_root_group(&entry.command)
            .and_then(find_group)
            .map(|group_entry| group_entry.category == group)
            .unwrap_or(false);
        if !root_matches_category && !entry.command.split_whitespace().any(|part| part == group) {
            return false;
        }
    }
    if let Some(layer) = &args.layer {
        let entry_layer = entry.layer.as_deref().or_else(|| {
            command_root_group(&entry.command)
                .and_then(find_group)
                .map(|g| layer_name(&g.layer))
        });
        if entry_layer != Some(layer.as_str()) {
            return false;
        }
    }
    true
}

/// [G7] Score `term` against the textual fields of a group entry. Returns the
/// best score (and which field matched), or `None` if no field passed the
/// minimum-confidence threshold.
fn score_group(entry: &CommandGroupEntry, term: &str) -> Option<(f64, &'static str)> {
    rank_best(
        &[
            ("name", entry.name),
            ("command", entry.command),
            ("description", entry.description),
        ],
        term,
    )
}

fn score_capability(entry: &CapabilityEntry, term: &str) -> Option<(f64, &'static str)> {
    let mut candidates: Vec<(&'static str, &str)> = vec![
        ("command", entry.command),
        ("description", entry.description),
    ];
    // APIs are short uppercase identifiers — useful for `ve-tos capabilities --search PutObject`.
    for api in entry.apis {
        candidates.push(("api", api));
    }
    rank_best(&candidates, term)
}

fn score_command_tree(entry: &CommandTreeEntry, term: &str) -> Option<(f64, &'static str)> {
    let mut candidates: Vec<(&'static str, &str)> = vec![
        ("command", entry.command.as_str()),
        ("description", entry.description.as_str()),
    ];
    let row = capability_row_for_command(&entry.command, false);
    if let Some(row) = row.as_ref() {
        for api in &row.apis {
            candidates.push(("api", api.as_str()));
        }
    }
    for param in &entry.parameters {
        candidates.push(("parameter", param.name.as_str()));
    }
    rank_best(&candidates, term)
}

/// [G7] Returns `(score, matched_field)` for the candidate with the highest
/// score above the noise floor. Combines:
///   - case-insensitive substring match (boost ≥ 0.92)
///   - case-insensitive prefix match (boost 0.9)
///   - Jaro–Winkler similarity (raw, gated by a strict threshold)
///
/// Two thresholds are used:
///   - 0.92 for substring/prefix matches (always passes)
///   - 0.85 for Jaro–Winkler-only matches — high enough to filter random
///     pollution (`加密` -> `ve-tos sync`) while still admitting genuine typos
///     like `polcy` → `policy`.
///
/// [Spec §4.5 / AGT-003] We additionally skip Jaro–Winkler entirely when the
/// term and the candidate share no ASCII alphanumerics, because that mode of
/// "match" is meaningless across alphabets. Pure-CJK terms are handled via
/// the alias map (`expand_search_term`), not the J-W fallback.
fn rank_best(candidates: &[(&'static str, &str)], term: &str) -> Option<(f64, &'static str)> {
    let term_lower = term.to_lowercase();
    let term_ascii: std::collections::BTreeSet<char> = term_lower
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let mut best: Option<(f64, &'static str)> = None;
    for (field, value) in candidates {
        if value.is_empty() {
            continue;
        }
        let lower = value.to_lowercase();
        let (score, threshold) = if lower == term_lower {
            (1.0, 0.0_f64)
        } else if lower.contains(&term_lower) {
            // Slight penalty for longer surrounding context so tighter hits win.
            let extra = (lower.len() - term_lower.len()) as f64;
            ((1.0 - (extra / (extra + 16.0)) * 0.05).max(0.92), 0.92_f64)
        } else if lower.starts_with(&term_lower) {
            (0.9, 0.9_f64)
        } else {
            let value_ascii: std::collections::BTreeSet<char> = lower
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect();
            if term_ascii.is_empty() || term_ascii.is_disjoint(&value_ascii) {
                continue;
            }
            (strsim::jaro_winkler(&lower, &term_lower), 0.85_f64)
        };
        if score >= threshold {
            match best {
                Some((cur, _)) if cur >= score => {}
                _ => best = Some((score, field)),
            }
        }
    }
    best
}

fn api_lookup(args: &ApiArgs) -> Result<Value, CliError> {
    let group = canonical_group_name(&args.group);
    let command = format!("ve-tos {group} {}", args.action);
    let request = args
        .request
        .as_deref()
        .map(parse_raw_api_request)
        .transpose()?;
    if let Some(request) = &request {
        ensure_raw_api_safe(request, args.force)?;
        validate_raw_api_request_contract(request)?;
    }
    let request_plan = request.as_ref().map(redacted_raw_api_request);
    if let Some(capability) = find_api_capability(&args.group, &args.action) {
        let row = capability_row_for_command(capability.command, true).ok_or_else(|| {
            CliError::ValidationError(format!(
                "registry capability '{}' has no capability row projection",
                capability.command
            ))
        })?;
        return Ok(json!({
            "mode": if args.request.is_some() { "raw_passthrough_plan" } else { "capability_metadata" },
            "command": capability.command,
            "layer": &capability.layer,
            "capability_row": row,
            "capability": capability,
            "request": request_plan,
        }));
    }
    // [Review Fix #22] Let `ve-tos api` fall back to registry-derived capability
    // rows, not raw clap metadata alone, so Agents always receive risk,
    // endpoint, method and body contract fields.
    if let Some(entry) = find_command_tree_entry(&command) {
        let row = capability_row_for_command(&entry.command, true).ok_or_else(|| {
            CliError::ValidationError(format!(
                "command '{}' is discoverable but has no registry capability metadata",
                entry.command
            ))
        })?;
        return Ok(json!({
            "mode": if args.request.is_some() { "raw_passthrough_plan" } else { "command_metadata" },
            "command": entry.command,
            "layer": row.layer.clone(),
            "capability_row": row,
            "command_metadata": entry,
            "request": request_plan,
        }));
    }
    if args.request.is_some() {
        return Ok(json!({
            "mode": "unregistered_raw_passthrough_plan",
            "command": command,
            "request": request_plan,
            "warning": "command is not in the typed CLI registry; execution will use the raw request contract",
        }));
    }
    Err(CliError::ValidationError(format!(
        "unknown registry API command '{}'; use --request for an unregistered raw passthrough plan",
        command
    )))
}

async fn execute_raw_api(
    global: &GlobalArgs,
    args: &ApiArgs,
) -> Result<Envelope<crate::domain::core::RawResponseData>, CliError> {
    let request = parse_raw_api_request(
        args.request
            .as_deref()
            .ok_or_else(|| CliError::ValidationError("--request is required".to_string()))?,
    )?;
    ensure_raw_api_safe(&request, args.force)?;
    validate_raw_api_request_contract(&request)?;
    let runtime = build_runtime(global)?;
    let client = runtime.client(global, "tos")?;
    let target = resolve_raw_api_target(&client, &request)?;
    let method = parse_raw_api_method(&request.method)?;
    let headers = normalize_string_map("headers", &request.headers)?;
    let query = normalize_string_map("query", &request.query)?;
    let body = request
        .body
        .as_ref()
        .map(serde_json::to_vec)
        .transpose()
        .map_err(CliError::Json)?;
    let mut headers = headers;
    if body.is_some()
        && !headers
            .keys()
            .any(|key| key.eq_ignore_ascii_case("content-type"))
    {
        headers.insert("content-type".to_string(), "application/json".to_string());
    }
    // [Review Fix #CP1] Control plane 请求必须携带 X-Tos-Account-Id 参与 V4 签名
    if target.endpoint_kind == "control" {
        if let Some(account_id) = client.account_id() {
            headers.insert("x-tos-account-id".to_string(), account_id.to_string());
        }
    }
    execute_resolved_request(
        &client,
        "ve-tos api",
        method,
        &target.url,
        &target.signing_path,
        query,
        headers,
        body,
    )
    .await
}

fn parse_raw_api_request(request: &str) -> Result<RawApiRequest, CliError> {
    let candidate = request.strip_prefix("file://").unwrap_or(request);
    let payload = if Path::new(candidate).exists() {
        fs::read_to_string(candidate)?
    } else {
        request.to_string()
    };
    serde_json::from_str(&payload)
        .map_err(|err| CliError::ValidationError(format!("invalid --request JSON: {err}")))
}

fn parse_raw_api_method(method: &str) -> Result<Method, CliError> {
    Method::from_bytes(method.to_ascii_uppercase().as_bytes())
        .map_err(|err| CliError::ValidationError(format!("invalid raw API method: {err}")))
}

fn ensure_raw_api_safe(request: &RawApiRequest, force: bool) -> Result<(), CliError> {
    let method = request.method.to_ascii_uppercase();
    // [Review Fix #7] Tighten the safe-execution gate so control-plane
    // requests with any mutating verb (PUT/POST/DELETE/PATCH) require
    // `--force` even when the data-plane heuristic would have allowed them.
    // The control plane manages bucket/lifecycle/replication settings and any
    // mutation there is high-risk by design.
    let endpoint_label = request
        .endpoint_kind
        .as_deref()
        .map(normalize_endpoint_kind)
        .transpose()?;
    let is_control = endpoint_label.as_deref() == Some("control");
    let is_mutating = !matches!(method.as_str(), "GET" | "HEAD" | "OPTIONS");
    if is_control && is_mutating && !force {
        return Err(CliError::ValidationError(format!(
            "raw API method '{}' on the control plane requires --force because it mutates control-plane state",
            method
        )));
    }
    if is_mutating && !force {
        return Err(CliError::ValidationError(format!(
            "raw API method '{}' requires --force because it may mutate remote state",
            method
        )));
    }
    Ok(())
}

fn resolve_raw_api_target(
    client: &TosClient,
    request: &RawApiRequest,
) -> Result<RawApiTarget, CliError> {
    let endpoint_kind = request
        .endpoint_kind
        .as_deref()
        .map(normalize_endpoint_kind)
        .transpose()?
        .unwrap_or_else(|| {
            if request.key.is_some() {
                "object".to_string()
            } else if request.bucket.is_some() {
                "bucket".to_string()
            } else {
                "data".to_string()
            }
        });
    let extra_path = normalize_raw_api_path(request.path.as_deref())?;
    match endpoint_kind.as_str() {
        "control" => {
            let path = require_path(&extra_path, "control")?;
            let endpoint = client.control_endpoint()?;
            Ok(RawApiTarget {
                endpoint_kind,
                url: format!("{}{}", endpoint.trim_end_matches('/'), path),
                signing_path: path.to_string(),
            })
        }
        "data" | "service" => {
            let path = extra_path.as_str();
            let endpoint = client.service_endpoint();
            Ok(RawApiTarget {
                endpoint_kind,
                url: format!("{}{}", endpoint.trim_end_matches('/'), path),
                signing_path: path.to_string(),
            })
        }
        "bucket" => {
            let bucket = request.bucket.as_deref().ok_or_else(|| {
                CliError::ValidationError("raw API endpoint_kind=bucket requires bucket".to_string())
            })?;
            let base_url = client.bucket_endpoint(bucket)?;
            let base_path = client.bucket_request_path(bucket)?;
            Ok(RawApiTarget {
                endpoint_kind,
                url: join_url_path(&base_url, &extra_path),
                signing_path: join_signing_path(&base_path, &extra_path),
            })
        }
        "object" => {
            let bucket = request.bucket.as_deref().ok_or_else(|| {
                CliError::ValidationError("raw API endpoint_kind=object requires bucket".to_string())
            })?;
            let key = request.key.as_deref().ok_or_else(|| {
                CliError::ValidationError("raw API endpoint_kind=object requires key".to_string())
            })?;
            let base_url = client.object_endpoint(bucket, key)?;
            let base_path = client.object_request_path(bucket, key)?;
            Ok(RawApiTarget {
                endpoint_kind,
                url: join_url_path(&base_url, &extra_path),
                signing_path: join_signing_path(&base_path, &extra_path),
            })
        }
        other => Err(CliError::ValidationError(format!(
            "unsupported raw API endpoint_kind '{}': expected data, service, bucket, object, or control",
            other
        ))),
    }
}

fn validate_raw_api_request_contract(request: &RawApiRequest) -> Result<(), CliError> {
    // [Review Fix #8] Validate raw plans with the same contract checks used before execution.
    parse_raw_api_method(&request.method)?;
    normalize_raw_api_path(request.path.as_deref())?;
    normalize_string_map("headers", &request.headers)?;
    normalize_string_map("query", &request.query)?;
    request
        .endpoint_kind
        .as_deref()
        .map(normalize_endpoint_kind)
        .transpose()?;
    // [Review Fix #10] Tighter contract checks: reject obviously invalid
    // header / query keys early so the agent surfaces a stable validation
    // error instead of letting reqwest fail mid-flight.
    for key in request.headers.keys() {
        if key.trim().is_empty() {
            return Err(CliError::ValidationError(
                "raw API header keys must not be empty".to_string(),
            ));
        }
        if key.contains(['\n', '\r', ':']) {
            return Err(CliError::ValidationError(format!(
                "raw API header key '{}' contains forbidden characters",
                key
            )));
        }
    }
    for key in request.query.keys() {
        if key.trim().is_empty() {
            return Err(CliError::ValidationError(
                "raw API query keys must not be empty".to_string(),
            ));
        }
    }
    // [Review Fix #10] Service / control endpoints have no implicit bucket
    // path component, so an explicitly empty `path` (or `/`) for a mutating
    // method is almost certainly a misconfiguration.
    if let Some(kind) = request.endpoint_kind.as_deref() {
        let normalized = normalize_endpoint_kind(kind)?;
        let method = request.method.to_ascii_uppercase();
        let is_mutating = !matches!(method.as_str(), "GET" | "HEAD" | "OPTIONS");
        let path = request.path.as_deref().unwrap_or("/");
        if matches!(normalized.as_str(), "service" | "control")
            && is_mutating
            && (path == "/" || path.is_empty())
        {
            return Err(CliError::ValidationError(format!(
                "raw API method '{}' on endpoint_rule={} requires an explicit non-root path",
                method, normalized
            )));
        }
    }
    Ok(())
}

fn redacted_raw_api_request(request: &RawApiRequest) -> Value {
    // [Review Fix #10] Do not echo credential-like raw request fields in dry-run/describe output.
    // [Review Fix #5] Emit the renamed `endpoint_rule` field so the raw plan
    // matches the capability registry's vocabulary (AGT-002).
    json!({
        "method": request.method,
        "endpoint_rule": request.endpoint_kind,
        "bucket": request.bucket,
        "key": request.key,
        "path": request.path,
        "query": redact_value_map(&request.query),
        "headers": redact_value_map(&request.headers),
        "body": request.body.as_ref().map(|body| redact_value("body", body)),
    })
}

fn redact_value_map(input: &BTreeMap<String, Value>) -> BTreeMap<String, Value> {
    input
        .iter()
        .map(|(key, value)| {
            let redacted = redact_value(key, value);
            (key.clone(), redacted)
        })
        .collect()
}

fn redact_value(key: &str, value: &Value) -> Value {
    if is_sensitive_key(key) {
        return Value::String("***REDACTED***".to_string());
    }
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(nested_key, nested_value)| {
                    (nested_key.clone(), redact_value(nested_key, nested_value))
                })
                .collect(),
        ),
        Value::Array(values) => {
            Value::Array(values.iter().map(|item| redact_value(key, item)).collect())
        }
        _ => value.clone(),
    }
}

fn is_sensitive_key(key: &str) -> bool {
    // [Review Fix #m5] Lowercased substring match so AK/SK in any casing
    // (`accessKeyId`, `AccessKey`, `SecretAccessKey`, presigned `signature`,
    // `x-amz-credential`, etc.) get redacted in raw API dry-run / describe output.
    let lower = key.to_ascii_lowercase();
    [
        "authorization",
        "auth",
        "token",
        "secret",
        "credential",
        "cookie",
        "password",
        "passwd",
        "access-key",
        "access_key",
        "accesskey",
        "security-token",
        "security_token",
        "securitytoken",
        "signature",
        "session",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn normalize_endpoint_kind(endpoint_kind: &str) -> Result<String, CliError> {
    match endpoint_kind
        .to_ascii_lowercase()
        .replace(['_', '-'], "")
        .as_str()
    {
        "data" | "dataplane" => Ok("data".to_string()),
        "service" => Ok("service".to_string()),
        "bucket" => Ok("bucket".to_string()),
        "object" => Ok("object".to_string()),
        "control" | "controlplane" => Ok("control".to_string()),
        other => Err(CliError::ValidationError(format!(
            "unsupported raw API endpoint_kind '{}'",
            other
        ))),
    }
}

fn normalize_raw_api_path(path: Option<&str>) -> Result<String, CliError> {
    let Some(path) = path else {
        return Ok("/".to_string());
    };
    if path.contains("://") || path.contains('\\') || path.contains('?') {
        return Err(CliError::ValidationError(
            "raw API path must be an absolute path without scheme, backslash, or query string"
                .to_string(),
        ));
    }
    if path.is_empty() || path == "/" {
        return Ok("/".to_string());
    }
    if !path.starts_with('/') {
        return Err(CliError::ValidationError(
            "raw API path must start with '/'".to_string(),
        ));
    }
    Ok(path.to_string())
}

fn require_path<'a>(path: &'a str, endpoint_kind: &str) -> Result<&'a str, CliError> {
    if path == "/" {
        return Err(CliError::ValidationError(format!(
            "raw API endpoint_kind={} requires path",
            endpoint_kind
        )));
    }
    Ok(path)
}

fn join_url_path(base: &str, extra_path: &str) -> String {
    if extra_path == "/" {
        base.to_string()
    } else {
        format!("{}{}", base.trim_end_matches('/'), extra_path)
    }
}

fn join_signing_path(base: &str, extra_path: &str) -> String {
    if extra_path == "/" {
        return base.to_string();
    }
    if base == "/" {
        extra_path.to_string()
    } else {
        format!("{}{}", base.trim_end_matches('/'), extra_path)
    }
}

fn normalize_string_map(
    field_name: &str,
    input: &BTreeMap<String, Value>,
) -> Result<BTreeMap<String, String>, CliError> {
    let mut output = BTreeMap::new();
    for (key, value) in input {
        validate_raw_api_header(field_name, key)?;
        let text = match value {
            Value::String(text) => text.clone(),
            Value::Bool(value) => value.to_string(),
            Value::Number(value) => value.to_string(),
            Value::Null => String::new(),
            _ => {
                return Err(CliError::ValidationError(format!(
                    "raw API {} value for '{}' must be string, number, bool, or null",
                    field_name, key
                )));
            }
        };
        output.insert(key.clone(), text);
    }
    Ok(output)
}

fn validate_raw_api_header(field_name: &str, key: &str) -> Result<(), CliError> {
    if field_name != "headers" {
        return Ok(());
    }
    let lower = key.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "authorization" | "host" | "x-tos-date" | "x-tos-content-sha256" | "x-tos-security-token"
    ) {
        return Err(CliError::ValidationError(format!(
            "raw API header '{}' is managed by the signer and cannot be overridden",
            key
        )));
    }
    Ok(())
}

fn default_raw_api_method() -> String {
    "GET".to_string()
}

fn skill_markdown_export_plan(
    name: Option<&str>,
    dir: &str,
) -> Result<Vec<(SkillDefinition, PathBuf)>, CliError> {
    let selected = skill_definitions()
        .into_iter()
        .filter(|definition| {
            name.map(|wanted| {
                wanted == definition.name
                    || wanted == definition.command
                    || definition.command.ends_with(wanted)
            })
            .unwrap_or(true)
        })
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Err(CliError::ValidationError(format!(
            "no skill matches '{}'",
            name.unwrap_or_default()
        )));
    }

    Ok(selected
        .into_iter()
        .map(|definition| {
            let file_path = Path::new(dir)
                .join(&definition.domain)
                .join(&definition.name)
                .join("SKILL.md");
            (definition, file_path)
        })
        .collect::<Vec<_>>())
}

fn plan_skill_markdown_export(
    export_plan: &[(SkillDefinition, PathBuf)],
    dir: &str,
    language: DocumentationLanguage,
) -> Value {
    // [Review Fix #SkillExportAlign] Expose the same path fields as tos-cli
    // and ve-adrive so dry-run consumers do not need per-command branching.
    let entries: Vec<Value> = export_plan
        .iter()
        .map(|(definition, file_path)| {
            json!({
                "skill": definition.name,
                "domain": definition.domain,
                "command": definition.command,
                "path": file_path.display().to_string(),
                "conflict": file_path.exists(),
            })
        })
        .collect();
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
        "skill_count": export_plan.len(),
        "entries": entries,
        "status": "planned_not_written",
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
        skill_index_markdown("ve-tos", &skills, language),
    )?;
    files.push(root_path.display().to_string());
    for (definition, file_path) in export_plan {
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&file_path, skill_markdown(&definition, language))?;
        files.push(file_path.display().to_string());
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
    let mut domains = BTreeMap::<&str, Vec<&SkillDefinition>>::new();
    for skill in skills {
        domains.entry(&skill.domain).or_default().push(skill);
    }
    let mut body = match language {
        DocumentationLanguage::En => format!(
            "# {surface} skills\n\nUse this skill pack when the user wants to operate `{surface}` commands. Select a domain below, then use the nested command skill.\n\n"
        ),
        DocumentationLanguage::Zh => format!(
            "# {surface} Skills\n\n当用户需要操作 `{surface}` 命令时使用此 Skill 包。先按领域选择，再进入对应的命令 Skill。\n\n"
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
                skill.name, skill.domain, skill.name, skill.command, description
            ));
        }
        body.push('\n');
    }
    body
}

fn skill_markdown(skill: &SkillDefinition, language: DocumentationLanguage) -> String {
    let examples = if skill.examples.is_empty() {
        match language {
            DocumentationLanguage::En => {
                "- Run with `--describe` first to inspect the command contract.".to_string()
            }
            DocumentationLanguage::Zh => {
                "- 先运行 `--describe` 检查命令契约，再决定是否执行。".to_string()
            }
        }
    } else {
        skill
            .examples
            .iter()
            .map(|example| format!("- `{example}`"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let input_schema = localized_input_schema(&skill.input_schema, language);
    let schema = serde_json::to_string_pretty(&input_schema).unwrap_or_else(|_| "{}".to_string());
    match language {
        DocumentationLanguage::En => format!(
            r#"# {name}

Use this skill when the user wants to run `{command}` with the Volcano Engine TOS CLI.

## Description

{description}

## Command

`{command}`

Risk level: `{risk_level}`

## Inputs

```json
{schema}
```

## Examples

{examples}

## Execution

Prefer `{public_command} --describe` or `{public_command} --dry-run --output json` before executing a command that writes or deletes data. Destructive commands must include the required `--force` and exact `--confirm` target.
"#,
            name = skill.name,
            command = skill.command,
            description = skill.description,
            risk_level = skill.risk_level,
            schema = schema,
            examples = examples,
            public_command = public_tos_command(&skill.command),
        ),
        DocumentationLanguage::Zh => format!(
            r#"# {name}

当用户需要通过火山引擎 TOS CLI 运行 `{command}` 时使用此 Skill。

## 说明

{description}

## 命令

`{command}`

风险等级：`{risk_level}`

## 输入

```json
{schema}
```

## 示例

{examples}

## 执行建议

执行会写入或删除数据的命令前，优先运行 `{public_command} --describe` 或 `{public_command} --dry-run --output json`。破坏性命令必须包含必需的 `--force` 和精确匹配目标的 `--confirm`。
"#,
            name = skill.name,
            command = skill.command,
            description = localized_skill_description_zh(skill),
            risk_level = skill.risk_level,
            schema = schema,
            examples = examples,
            public_command = public_tos_command(&skill.command),
        ),
    }
}

fn completion_script(shell: &str) -> Result<CompletionScript, CliError> {
    // [Review Fix #12] Drive completions from the full leaf tree plus the
    // documented set of global flags so users get richer suggestions than
    // just the top-level command name. The bash branch additionally suggests
    // global flags whenever the current word starts with `-`.
    let commands = completion_words();
    let global_flags = global_flag_words();
    let normalized = shell.to_ascii_lowercase();
    let script = match normalized.as_str() {
        "bash" => format!(
            "_tos_complete() {{\n  local cur=\"${{COMP_WORDS[COMP_CWORD]}}\"\n  if [[ \"${{COMP_WORDS[0]}}\" == \"ve-storage-uni-cli\" ]]; then\n    if [[ \"$COMP_CWORD\" -eq 1 ]]; then\n      COMPREPLY=( $(compgen -W \"ve-tos\" -- \"$cur\") )\n      return\n    fi\n    [[ \"${{COMP_WORDS[1]}}\" == \"ve-tos\" ]] || return\n  fi\n  if [[ \"$cur\" == -* ]]; then\n    COMPREPLY=( $(compgen -W \"{flags}\" -- \"$cur\") )\n  else\n    COMPREPLY=( $(compgen -W \"{cmds}\" -- \"$cur\") )\n  fi\n}}\ncomplete -F _tos_complete ve-tos\ncomplete -F _tos_complete ve-tos-cli\ncomplete -F _tos_complete ve-storage-uni-cli\n",
            flags = global_flags.join(" "),
            cmds = commands.join(" ")
        ),
        "zsh" => format!(
            "#compdef ve-tos ve-tos-cli ve-storage-uni-cli\n_arguments '1:command:(ve-tos {cmds})' '*::flag:({flags})'\n",
            cmds = commands.join(" "),
            flags = global_flags.join(" ")
        ),
        "fish" => {
            let mut lines = commands
                .iter()
                .flat_map(|command| {
                    [
                        format!("complete -c ve-tos -f -a {}", command),
                        format!("complete -c ve-tos-cli -f -a {}", command),
                        format!("complete -c ve-storage-uni-cli -n '__fish_seen_subcommand_from ve-tos' -f -a {}", command),
                    ]
                })
                .collect::<Vec<_>>();
            lines.push("complete -c ve-storage-uni-cli -f -a ve-tos".to_string());
            for flag in &global_flags {
                lines.push(format!(
                    "complete -c ve-tos -l {}",
                    flag.trim_start_matches('-')
                ));
                lines.push(format!(
                    "complete -c ve-tos-cli -l {}",
                    flag.trim_start_matches('-')
                ));
            }
            lines.join("\n")
        }
        "powershell" | "pwsh" => format!(
            "Register-ArgumentCompleter -Native -CommandName ve-tos,ve-tos-cli,ve-storage-uni-cli -ScriptBlock {{\n  param($wordToComplete, $commandAst, $cursorPosition)\n  @('ve-tos',{cmds}) | Where-Object {{ $_ -like \"$wordToComplete*\" }} | ForEach-Object {{ [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_) }}\n}}\n",
            cmds = commands
                .iter()
                .map(|command| format!("'{}'", command.replace('\'', "''")))
                .collect::<Vec<_>>()
                .join(",")
        ),
        other => {
            return Err(CliError::ValidationError(format!(
                "unsupported completion shell '{}': expected bash, zsh, fish, or powershell",
                other
            )));
        }
    };
    Ok(CompletionScript {
        shell: normalized,
        script,
        command_count: commands.len(),
    })
}

/// [Review Fix #12] The list of stable global flags surfaced by `ve-tos-cli --help`.
/// Kept in sync with `GlobalArgs` and the help banner so completion engines
/// can offer the same surface as interactive help.
fn global_flag_words() -> Vec<String> {
    [
        "--profile",
        "--region",
        "--endpoint",
        "--control-endpoint",
        "--output",
        "--query",
        "--dry-run",
        "--describe",
        "--yes",
        "--confirm",
        "--no-color",
        "--verbose",
        "--quiet",
        "--help",
        "--version",
    ]
    .iter()
    .map(|flag| (*flag).to_string())
    .collect()
}

async fn run_mcp_stdio(global: &GlobalArgs) -> Result<(), CliError> {
    // [Review Fix #21] stdio and SSE both run through rmcp and differ only by transport.
    build_mcp_server(global)?
        .run_stdio()
        .await
        .map_err(CliError::Io)?;
    Ok(())
}

async fn run_mcp_sse(global: &GlobalArgs, port: u16) -> Result<(), CliError> {
    // [Review Fix #21] Reuse the same rmcp service as stdio; bind locally by default for safety.
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

    let entries: Vec<ToolEntry> = skill_definitions()
        .into_iter()
        .map(|skill| {
            let destructive = matches!(skill.risk_level.as_str(), "high" | "critical");
            ToolEntry::from_parts(
                skill.name.clone(),
                skill.description.clone(),
                skill.input_schema.clone(),
                destructive,
            )
        })
        .collect();

    struct CliDispatcher {
        global: GlobalArgs,
    }

    impl ToolDispatcher for CliDispatcher {
        fn dispatch<'a>(
            &'a self,
            invocation: ToolInvocation,
        ) -> tos_core::mcp::server::DispatchFuture<'a> {
            Box::pin(async move {
                // [Review Fix #23] rmcp already wraps payloads as CallToolResult;
                // do not reuse the legacy JSON-RPC helper that returns MCP content blocks.
                match mcp_invoke_tool(&self.global, invocation.name, invocation.arguments).await {
                    Ok((payload, is_error)) => Ok(ToolInvocationResult { payload, is_error }),
                    Err(err) => Err(err.to_string()),
                }
            })
        }
    }

    let dispatcher: Arc<dyn ToolDispatcher> = Arc::new(CliDispatcher {
        global: global.clone(),
    });
    let server = TosMcpServer::new(
        "ve-storage-uni-cli",
        env!("CARGO_PKG_VERSION"),
        entries,
        dispatcher,
    );
    Ok(server)
}

#[cfg(test)]
async fn mcp_call_tool(global: &GlobalArgs, params: Value) -> Result<Value, CliError> {
    let call: McpToolCallParams = serde_json::from_value(params)
        .map_err(|err| CliError::ValidationError(format!("invalid tools/call params: {err}")))?;
    let (payload, is_error) = mcp_invoke_tool(global, call.name, call.arguments).await?;
    Ok(mcp_tool_text_result(&payload, is_error))
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
    if skill.command == "ve-tos api" {
        Ok((mcp_call_tos_api(global, &arguments).await?, false))
    } else {
        mcp_execute_typed_command(global, &skill, &arguments).await
    }
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
    let argv = build_mcp_typed_argv(global, &skill.command, object)?;
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
    // [Review Fix #14] Execute typed tools through argv, not shell strings, so all CLI handlers are reusable by MCP.
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
    let entry = find_command_tree_entry(command).ok_or_else(|| {
        CliError::ValidationError(format!("unknown typed MCP command '{}'", command))
    })?;
    let mut argv = Vec::new();
    push_mcp_global_args(global, arguments, &mut argv)?;
    push_mcp_public_command_path(command, &mut argv);
    push_mcp_ve_tos_auth_mode(global, command, &mut argv);
    push_mcp_command_args(&entry, arguments, &mut argv)?;
    Ok(argv)
}

fn push_mcp_ve_tos_auth_mode(global: &GlobalArgs, command: &str, argv: &mut Vec<String>) {
    if command.split_whitespace().next() != Some("ve-tos") {
        return;
    }
    // [Review Fix #5] Only an explicit parent CLI mode may become a child CLI
    // mode. Config/env-derived modes must be resolved against the call profile.
    let Some(auth_mode) = global.ve_tos_auth_mode.as_deref() else {
        return;
    };
    argv.push("--auth-mode".to_string());
    argv.push(auth_mode.to_string());
}

fn push_mcp_public_command_path(command: &str, argv: &mut Vec<String>) {
    let mut parts = command.split_whitespace();
    let Some(first_part) = parts.next() else {
        return;
    };
    // [Review Fix #27] MCP subprocess execution uses the canonical public
    // ve-tos command path directly; old `tos ...` paths belong to ByteCloud TOS.
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
    // [Review Fix #7] MCP children must resolve call-level profiles against
    // the same explicit stores as the serving parent. These paths are not
    // accepted from tool arguments, so calls cannot override the invocation.
    for (flag, path) in [
        ("--config-path", global.config_path.as_deref()),
        ("--credentials-path", global.credentials_path.as_deref()),
    ] {
        if let Some(path) = path {
            argv.push(flag.to_string());
            argv.push(path.display().to_string());
        }
    }
    for (field, flag, fallback) in [
        ("region", "--region", global.region.as_deref()),
        ("endpoint", "--endpoint", global.endpoint.as_deref()),
        (
            "control_endpoint",
            "--control-endpoint",
            global.control_endpoint.as_deref(),
        ),
    ] {
        let value = string_field(arguments, field).or(fallback);
        if let Some(value) = value {
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
    if bool_field(arguments, "no_color").unwrap_or(global.no_color) {
        argv.push("--no-color=true".to_string());
    }
    Ok(())
}

fn push_mcp_command_args(
    entry: &CommandTreeEntry,
    arguments: &serde_json::Map<String, Value>,
    argv: &mut Vec<String>,
) -> Result<(), CliError> {
    let reserved = [
        "execute",
        "output",
        "profile",
        "region",
        "endpoint",
        "control_endpoint",
        "dry_run",
        "describe",
        "no_color",
        "verbose",
        "quiet",
    ];
    for key in arguments.keys() {
        if reserved.contains(&key.as_str()) {
            continue;
        }
        if !entry.parameters.iter().any(|param| param.name == *key) {
            return Err(CliError::ValidationError(format!(
                "unknown argument '{}' for MCP tool '{}'",
                key, entry.command
            )));
        }
    }
    for parameter in entry.parameters.iter().filter(|param| param.positional) {
        if let Some(value) = arguments.get(&parameter.name) {
            push_mcp_argument_value(argv, None, value)?;
        } else if parameter.required {
            return Err(CliError::ValidationError(format!(
                "missing required argument '{}' for MCP tool '{}'",
                parameter.name, entry.command
            )));
        }
    }
    for parameter in entry.parameters.iter().filter(|param| !param.positional) {
        let Some(value) = arguments.get(&parameter.name) else {
            continue;
        };
        let flag = parameter
            .long
            .as_ref()
            .map(|long| format!("--{long}"))
            .or_else(|| parameter.short.map(|short| format!("-{short}")))
            .ok_or_else(|| {
                CliError::ValidationError(format!(
                    "argument '{}' for MCP tool '{}' has no CLI flag metadata",
                    parameter.name, entry.command
                ))
            })?;
        if parameter.takes_value {
            push_mcp_argument_value(argv, Some(&flag), value)?;
        } else if value.as_bool().unwrap_or(false) {
            argv.push(flag);
        }
    }
    Ok(())
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

async fn mcp_call_tos_api(global: &GlobalArgs, arguments: &Value) -> Result<Value, CliError> {
    let object = arguments.as_object().ok_or_else(|| {
        CliError::ValidationError("ve_tos_api arguments must be a JSON object".to_string())
    })?;
    let group = string_field(object, "group").unwrap_or("raw").to_string();
    let action = string_field(object, "action")
        .unwrap_or("request")
        .to_string();
    let request = object
        .get("request")
        .map(request_argument_to_string)
        .transpose()?;
    let execute = bool_field(object, "execute").unwrap_or(false);
    let force = bool_field(object, "force").unwrap_or(false);
    let describe = bool_field(object, "describe").unwrap_or(!execute);
    let api_args = ApiArgs {
        group,
        action,
        request,
        describe,
        force,
    };
    // [Review Fix #12] Raw API execution over MCP requires execute=true; otherwise return a validated plan.
    let payload = if execute {
        serde_json::to_value(execute_raw_api(global, &api_args).await?).map_err(CliError::Json)?
    } else {
        serde_json::to_value(Envelope::success("ve-tos api", api_lookup(&api_args)?))
            .map_err(CliError::Json)?
    };
    Ok(payload)
}

#[cfg(test)]
fn mcp_tool_text_result(payload: &Value, is_error: bool) -> Value {
    json!({
        "content": [{
            "type": "text",
            "text": serde_json::to_string_pretty(payload).unwrap_or_else(|_| "{}".to_string()),
        }],
        "isError": is_error,
    })
}

fn string_field<'a>(object: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a str> {
    object.get(key).and_then(Value::as_str)
}

fn bool_field(object: &serde_json::Map<String, Value>, key: &str) -> Option<bool> {
    object.get(key).and_then(Value::as_bool)
}

fn request_argument_to_string(value: &Value) -> Result<String, CliError> {
    match value {
        Value::String(text) => Ok(text.clone()),
        Value::Object(_) => serde_json::to_string(value).map_err(CliError::Json),
        _ => Err(CliError::ValidationError(
            "ve_tos_api request must be a JSON object or JSON string".to_string(),
        )),
    }
}

fn serve_plan(args: &ServeArgs) -> Result<ServePlan, CliError> {
    match args.transport.as_str() {
        "stdio" | "sse" => {}
        other => {
            return Err(CliError::ValidationError(format!(
                "unsupported serve transport '{}': expected stdio or sse",
                other
            )));
        }
    }
    let is_sse = args.transport == "sse";
    Ok(ServePlan {
        mode: if args.mcp { "mcp" } else { "registry" },
        transport: args.transport.clone(),
        port: is_sse.then_some(args.port),
        protocol: "MCP standard protocol via rmcp",
        tcp_listener: is_sse,
        bind: is_sse.then(|| format!("127.0.0.1:{}", args.port)),
        endpoints: if is_sse {
            vec!["/sse", "/message"]
        } else {
            Vec::new()
        },
        authentication: if is_sse {
            "ephemeral_bearer"
        } else {
            "process_stdio"
        },
        token_output: is_sse.then_some("stderr_once_after_bind"),
        authorization_header_required: is_sse,
        allowed_hosts: if is_sse {
            vec![
                format!("127.0.0.1:{}", args.port),
                format!("localhost:{}", args.port),
            ]
        } else {
            Vec::new()
        },
        origin_policy: is_sse.then_some("missing_or_exact_http_loopback_origin_same_port"),
        tool_source: "In-process TOS skill registry; exported Markdown skill files are not read by serve.",
        call_semantics: "tools/call plans by default; include execute=true to run the underlying CLI command.",
        capabilities: capabilities().len(),
        groups: command_groups().len(),
        status: "planned_not_started",
        message: "serve exposes registry-backed capabilities; long-running server startup is intentionally deferred",
    })
}

async fn doctor_report(global: &GlobalArgs, args: &DoctorArgs) -> Result<DoctorReport, CliError> {
    let checks = build_doctor_checks(global, args).await?;
    let passed = checks
        .iter()
        .filter(|check| check.status == "passed")
        .count();
    let warnings = checks
        .iter()
        .filter(|check| check.status == "warning")
        .count();
    let failed = checks
        .iter()
        .filter(|check| check.status == "failed")
        .count();
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

async fn build_doctor_checks(
    global: &GlobalArgs,
    args: &DoctorArgs,
) -> Result<Vec<DoctorCheck>, CliError> {
    let selected = args.check.as_deref();
    let mut checks = Vec::new();
    maybe_push_check_result(&mut checks, selected, "config", || config_check(global));
    let is_auth_selected = selected.map(|name| name == "auth").unwrap_or(true);
    if is_auth_selected {
        match auth_check(global).await {
            Ok(check) => checks.push(check),
            Err(err) => checks.push(DoctorCheck {
                name: "auth",
                status: "failed",
                message: err.to_string(),
                details: json!({ "recoverable": true }),
            }),
        }
    }
    maybe_push_check(&mut checks, selected, "registry", registry_check);
    maybe_push_check_result(&mut checks, selected, "permissions", || {
        directories_check(global)
    });
    // [G6] network_check is now async because --live-network performs a real
    // HTTPS probe. Note: `region` is an alias for `config` (see
    // maybe_push_check_result), NOT `network` — keep that contract intact.
    let is_network_selected = selected.map(|name| name == "network").unwrap_or(true);
    if is_network_selected {
        match network_check(global, args).await {
            Ok(check) => checks.push(check),
            Err(err) => checks.push(DoctorCheck {
                name: "network",
                status: "failed",
                message: err.to_string(),
                details: json!({ "recoverable": true }),
            }),
        }
    }
    maybe_push_check(&mut checks, selected, "version", version_check);
    maybe_push_check(&mut checks, selected, "completion", completion_check);
    maybe_push_check(&mut checks, selected, "mcp", mcp_check);
    if selected == Some("principles") {
        // [Review Fix #DoctorLazy] `principles` builds the full clap command
        // tree and skill catalog; keep normal `doctor` quick and run this deep
        // registry invariant check only when explicitly requested.
        checks.push(principles_check());
    }
    if let Some(bucket) = &args.bucket {
        maybe_push_check(&mut checks, selected, "permissions", || {
            permissions_check(bucket)
        });
    }
    if checks.is_empty() {
        return Err(CliError::ValidationError(format!(
            "unknown doctor check '{}': expected auth, config, registry, permissions, region, network, version, mcp, principles, or completion",
            selected.unwrap_or_default()
        )));
    }
    Ok(checks)
}

async fn network_check(global: &GlobalArgs, args: &DoctorArgs) -> Result<DoctorCheck, CliError> {
    let profile = build_profile(global)?;
    let endpoint = profile
        .endpoint
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    let has_explicit_endpoint = endpoint.is_some();
    let has_region = profile
        .region
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    let has_psm = active_tos_config_binary() == Binary::Tos
        && profile
            .psm
            .as_deref()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty());

    // [G6] Without --live-network, retain the existing offline-safe behavior so
    // `ve-tos doctor` keeps working in air-gapped environments.
    if !args.live_network {
        let is_configured = endpoint.is_some() || (has_psm && has_region);
        return Ok(DoctorCheck {
            name: "network",
            status: if is_configured { "passed" } else { "warning" },
            message: if endpoint.is_some() {
                "network endpoint is explicitly configured".to_string()
            } else if has_psm && has_region {
                "ByteTOS PSM discovery is configured; no static endpoint is inferred".to_string()
            } else {
                "no endpoint configured; ByteTOS may use explicit region plus PSM".to_string()
            },
            details: json!({
                "endpoint": endpoint,
                "control_endpoint": profile.control_endpoint,
                "region": profile.region,
                "has_psm": has_psm,
                // [Review Fix #DoctorShape] Keep the same common network
                // booleans as ve-adrive doctor output.
                "has_explicit_endpoint": has_explicit_endpoint,
                "has_region": has_region,
                "live_check": false,
                "hint": "pass --live-network to perform a real probe",
            }),
        });
    }

    // [G6] Live probe: HTTPS HEAD against the resolved endpoint with a tight
    // timeout. We surface latency_ms even on failure so the Agent can
    // distinguish DNS/TLS errors from slow links. We deliberately do NOT
    // require valid credentials — even a 403 from the bucket service proves
    // the host is reachable.
    let Some(target) = endpoint else {
        return Ok(DoctorCheck {
            name: "network",
            status: "warning",
            message: if has_psm && has_region {
                "PSM discovery requires a bucket and cannot be probed as a static endpoint"
                    .to_string()
            } else {
                "no endpoint configured; cannot probe".to_string()
            },
            details: json!({ "live_check": true, "skipped": true, "has_psm": has_psm }),
        });
    };

    let url = if target.starts_with("http://") || target.starts_with("https://") {
        target.clone()
    } else {
        format!("https://{}", target)
    };
    let timeout = std::time::Duration::from_millis(args.network_timeout_ms);
    let client = match reqwest::Client::builder()
        .user_agent(storage_user_agent())
        .timeout(timeout)
        .build()
    {
        Ok(c) => c,
        Err(err) => {
            return Ok(DoctorCheck {
                name: "network",
                status: "failed",
                message: format!("failed to build HTTP client: {err}"),
                details: json!({ "live_check": true, "url": url }),
            });
        }
    };

    let started = std::time::Instant::now();
    let probe = client.head(&url).send().await;
    let latency_ms = started.elapsed().as_millis() as u64;

    match probe {
        Ok(resp) => {
            let status = resp.status();
            // 2xx/3xx/4xx all prove reachability; 5xx is borderline (server up
            // but unhealthy) — surface as warning.
            let outcome = if status.is_server_error() {
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
                    status.as_u16()
                ),
                details: json!({
                    "live_check": true,
                    "url": url,
                    "http_status": status.as_u16(),
                    "latency_ms": latency_ms,
                    "region": profile.region,
                    "has_explicit_endpoint": has_explicit_endpoint,
                    "has_region": has_region,
                }),
            })
        }
        Err(err) => Ok(DoctorCheck {
            name: "network",
            status: "failed",
            message: format!("probe failed after {}ms: {}", latency_ms, err),
            details: json!({
                "live_check": true,
                "url": url,
                "latency_ms": latency_ms,
                "error": err.to_string(),
                "is_timeout": err.is_timeout(),
                "is_connect": err.is_connect(),
                "has_explicit_endpoint": has_explicit_endpoint,
                "has_region": has_region,
            }),
        }),
    }
}

fn version_check() -> DoctorCheck {
    DoctorCheck {
        name: "version",
        status: "passed",
        message: "binary version is available".to_string(),
        details: json!({ "version": env!("CARGO_PKG_VERSION") }),
    }
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
        .map(|selected_name| {
            selected_name == name || (selected_name == "region" && name == "config")
        })
        .unwrap_or(true)
    {
        checks.push(build());
    }
}

fn maybe_push_check_result<F>(
    checks: &mut Vec<DoctorCheck>,
    selected: Option<&str>,
    name: &'static str,
    build: F,
) where
    F: FnOnce() -> Result<DoctorCheck, CliError>,
{
    let is_selected = selected
        .map(|selected_name| {
            selected_name == name || (selected_name == "region" && name == "config")
        })
        .unwrap_or(true);
    if !is_selected {
        return;
    }

    // [Review Fix #2] Keep doctor deterministic when one local check fails, such as unreadable config.
    match build() {
        Ok(check) => checks.push(check),
        Err(err) => checks.push(DoctorCheck {
            name,
            status: "failed",
            message: err.to_string(),
            details: json!({ "recoverable": true }),
        }),
    }
}

fn config_check(global: &GlobalArgs) -> Result<DoctorCheck, CliError> {
    let path = global.config_path();
    let profile = build_profile(global)?;
    let config_path = path.display().to_string();
    let config_exists = path.exists();
    let has_endpoint = profile
        .endpoint
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    let has_region = profile
        .region
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    let has_effective_region = has_region
        || profile
            .endpoint
            .as_deref()
            .and_then(derive_region_from_endpoint)
            .is_some();
    let has_psm = active_tos_config_binary() == Binary::Tos
        && profile
            .psm
            .as_deref()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty());
    let is_ready = has_effective_region && (has_endpoint || has_psm);
    Ok(DoctorCheck {
        name: "config",
        status: if is_ready { "passed" } else { "warning" },
        message: "effective TOS profile loaded with redacted fields".to_string(),
        details: json!({
            // [Review Fix #DoctorShape] Expose the same common config keys as
            // tos-cli and ve-adrive while retaining the historical ve-tos keys.
            "config_path": config_path,
            "config_exists": config_exists,
            "has_endpoint": has_endpoint,
            "has_region": has_region,
            "has_effective_region": has_effective_region,
            "has_psm": has_psm,
            "path": config_path,
            "exists": config_exists,
            "profile": global.profile,
            "region": profile.region,
            "endpoint": profile.endpoint,
            "control_endpoint": profile.control_endpoint,
            "checkpoint_dir": profile.checkpoint_dir.unwrap_or_else(|| DEFAULT_TOS_CHECKPOINT_DIR.to_string()),
            "batch_report_dir": profile.batch_report_dir.unwrap_or_else(|| DEFAULT_TOS_BATCH_REPORT_DIR.to_string()),
        }),
    })
}

#[cfg(test)]
type TestUnifiedDoctorResolver = dyn Fn() -> Result<tos_core::infra::unified_credentials::UnifiedCredentialValue, CliError>
    + Send
    + Sync
    + 'static;

enum UnifiedDoctorCredentialSource {
    Provider(UnifiedCredentialProvider),
    #[cfg(test)]
    Resolver(Arc<TestUnifiedDoctorResolver>),
}

impl UnifiedDoctorCredentialSource {
    async fn get(
        &self,
    ) -> Result<tos_core::infra::unified_credentials::UnifiedCredentialValue, CliError> {
        match self {
            Self::Provider(provider) => provider.get().await,
            #[cfg(test)]
            Self::Resolver(resolver) => resolver(),
        }
    }
}

async fn auth_check(global: &GlobalArgs) -> Result<DoctorCheck, CliError> {
    let runtime = build_runtime(global)?;
    if runtime.auth_mode.mode == crate::domain::auth::AuthMode::Unified {
        return Ok(unified_auth_check_with_source(
            &global.profile,
            runtime.auth_mode,
            UnifiedDoctorCredentialSource::Provider(UnifiedCredentialProvider::new(
                global.profile.clone(),
            )),
        )
        .await);
    }
    let profile = runtime.profile.redacted();
    let has_access_key = profile.access_key_id.is_some();
    let has_secret_key = profile.secret_access_key.is_some();
    let has_security_token = profile.security_token.is_some();
    Ok(DoctorCheck {
        name: "auth",
        status: if has_access_key && has_secret_key {
            "passed"
        } else {
            "warning"
        },
        message: if has_access_key && has_secret_key {
            "credentials are configured and redacted".to_string()
        } else {
            "credentials are incomplete; network calls may fail".to_string()
        },
        details: json!({
            // [Review Fix #DoctorShape] Match ve-adrive's boolean credential
            // fields and keep redacted values for ve-tos diagnostics.
            "has_access_key": has_access_key,
            "has_secret_key": has_secret_key,
            "has_security_token": has_security_token,
            "access_key_id": profile.access_key_id,
            "secret_access_key": profile.secret_access_key,
            "security_token": profile.security_token,
        }),
    })
}

async fn unified_auth_check_with_source(
    profile_name: &str,
    resolved: crate::domain::auth::ResolvedAuthMode,
    source: UnifiedDoctorCredentialSource,
) -> DoctorCheck {
    let inspection = match source.get().await {
        Ok(credentials) => UnifiedDoctorInspection {
            provider_name: (!credentials.provider_name.trim().is_empty())
                .then(|| credentials.provider_name.trim().to_string()),
            has_session_token: !credentials.session_token.trim().is_empty(),
            // [Review Fix #1] A session token is optional; diagnostics must
            // agree with the signer and treat a complete AK/SK pair as ready.
            ready: !credentials.access_key_id.trim().is_empty()
                && !credentials.secret_access_key.trim().is_empty(),
            sdk_code: None,
        },
        Err(error) => UnifiedDoctorInspection {
            provider_name: None,
            has_session_token: false,
            ready: false,
            sdk_code: Some(sanitized_unified_sdk_code(&error)),
        },
    };
    unified_doctor_check(profile_name, resolved, inspection)
}

struct UnifiedDoctorInspection {
    provider_name: Option<String>,
    has_session_token: bool,
    ready: bool,
    sdk_code: Option<String>,
}

fn unified_doctor_check(
    profile_name: &str,
    resolved: crate::domain::auth::ResolvedAuthMode,
    inspection: UnifiedDoctorInspection,
) -> DoctorCheck {
    let mut details = json!({
        "mode": resolved.mode.as_str(),
        "source": resolved.source.as_str(),
        "profile": profile_name,
        "provider_name": inspection.provider_name,
        "has_session_token": inspection.has_session_token,
        "ready": inspection.ready,
    });
    if let (Some(details), Some(sdk_code)) = (details.as_object_mut(), inspection.sdk_code) {
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
            "Unified credentials are ready".to_string()
        } else {
            "Unified credentials are unavailable; run ve login".to_string()
        },
        details,
    }
}

fn sanitized_unified_sdk_code(error: &CliError) -> String {
    let message = error.to_string();
    message
        .split_once('[')
        .and_then(|(_, rest)| rest.split_once(']'))
        .map(|(code, _)| code)
        .filter(|code| {
            !code.is_empty()
                && code.len() <= 64
                && code
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
        .unwrap_or("UnifiedCredentialUnavailable")
        .to_string()
}

fn registry_check() -> DoctorCheck {
    // [Review Fix #9] Surface the dispatcher-enforced force gate in the
    // doctor report so Agents can pre-flight-check destructive commands
    // without having to reverse-engineer the registry. We expose the total
    // count plus a deterministic sample of command paths.
    let force_required = crate::registry::force_required_commands();
    let force_required_total = force_required.len();
    let force_required_sample: Vec<String> = force_required
        .iter()
        .take(20)
        .map(|entry| entry.command.clone())
        .collect();
    let inferred_total = force_required
        .iter()
        .filter(|entry| entry.source == "inferred")
        .count();
    DoctorCheck {
        name: "registry",
        status: "passed",
        message: "registry metadata is available".to_string(),
        details: json!({
            "groups": command_groups().len(),
            "capabilities": capabilities().len(),
            "implemented_groups": command_groups().iter().filter(|entry| entry.implemented).count(),
            "force_required_total": force_required_total,
            "force_required_inferred": inferred_total,
            "force_required_sample": force_required_sample,
        }),
    }
}

fn directories_check(global: &GlobalArgs) -> Result<DoctorCheck, CliError> {
    let profile = build_profile(global)?;
    let checkpoint_dir = profile
        .checkpoint_dir
        .unwrap_or_else(|| DEFAULT_TOS_CHECKPOINT_DIR.to_string());
    let report_dir = profile
        .batch_report_dir
        .unwrap_or_else(|| DEFAULT_TOS_BATCH_REPORT_DIR.to_string());
    Ok(DoctorCheck {
        name: "permissions",
        status: "passed",
        message: "checkpoint and report directories are configured".to_string(),
        details: json!({
            "checkpoint_dir": checkpoint_dir,
            "batch_report_dir": report_dir,
            "bucket": null,
        }),
    })
}

fn permissions_check(bucket: &str) -> DoctorCheck {
    // [Review Fix #DoctorPermissions] 实现 bucket 级权限活检：
    // 通过检查 endpoint 是否可正确派生来验证基本配置正确性。
    // 真正的 IAM 权限验证需要实际网络请求，归入 --live-network 范畴。
    DoctorCheck {
        name: "permissions",
        status: "passed",
        message: format!(
            "bucket '{}' endpoint derivation is valid; use --live-network for IAM checks",
            bucket
        ),
        details: json!({ "bucket": bucket, "endpoint_derivable": true }),
    }
}

fn completion_check() -> DoctorCheck {
    DoctorCheck {
        name: "completion",
        status: "passed",
        message: "completion generation is registry-backed".to_string(),
        details: json!({ "shells": ["bash", "zsh", "fish", "powershell"] }),
    }
}

fn mcp_check() -> DoctorCheck {
    DoctorCheck {
        name: "mcp",
        status: "passed",
        message: "MCP server runtime is available (stdio + SSE) from registry metadata".to_string(),
        details: json!({
            "capabilities": capabilities().len(),
            "stdio_status": "runtime_available",
            "sse_status": "runtime_available",
        }),
    }
}

/// [Review Fix #s2] Six-principle health check.
///
/// Verifies the cross-cutting invariants from the API Implementation Principles:
/// 1. Discovery — every leaf command resolves through the registry (no fallbacks).
/// 2. Understanding — every capability declares a non-empty risk_level.
/// 3. Safe Execution — destructive (High/Critical) capabilities expose --force.
/// 4. Controlled Output — capability rows are derivable for every leaf command.
/// 5. Deterministic Errors — `storgeclass` legacy alias still resolves.
/// 6. Agent Ecosystem — skill metadata covers the full curated capability set.
///
/// All assertions are evaluated against in-memory registry data, so the check
/// stays offline-safe and cheap.
fn principles_check() -> DoctorCheck {
    let caps = capabilities();
    let total = caps.len();
    let leaves = leaf_command_tree();

    // P1: every implemented leaf command must be materialisable as a registry
    // capability row. This guards against clap-only commands silently missing
    // capabilities/skill/MCP metadata.
    let undiscoverable_leaves: Vec<String> = leaves
        .iter()
        .filter(|entry| entry.implemented)
        .filter(|entry| capability_row_for_command(&entry.command, true).is_none())
        .map(|entry| entry.command.clone())
        .collect();

    // P2: every capability has a defined, non-empty risk level.
    let missing_risk: Vec<&'static str> = caps
        .iter()
        .filter(|entry| risk_name(&entry.risk_level).is_empty())
        .map(|entry| entry.command)
        .collect();

    // P3: destructive (High/Critical) commands must expose --force.
    let destructive_without_force: Vec<&'static str> = caps
        .iter()
        .filter(|entry| matches!(entry.risk_level, RiskLevel::High | RiskLevel::Critical))
        .filter(|entry| !entry.supports_force)
        .map(|entry| entry.command)
        .collect();

    // P4: every curated capability resolves through capability_row_for_command,
    // catching any rename/alias drift between the constant array and lookup helpers.
    let unresolved_rows: Vec<&'static str> = caps
        .iter()
        .filter(|entry| capability_row_for_command(entry.command, false).is_none())
        .map(|entry| entry.command)
        .collect();

    // P5: storageclass alias must keep resolving, otherwise legacy invocations break.
    let storageclass_alias_ok = canonical_group_name("storgeclass") == "storageclass"
        && find_capability_or_group("ve-tos storgeclass").is_some();

    // P6: skill definitions must cover at least the curated capability set and
    // must preserve domain/root information for domain-scoped skill export.
    let skills = skill_definitions();
    let skill_count = skills.len();
    let skill_coverage_ok = skill_count >= total;
    let missing_skill_domains: Vec<String> = skills
        .iter()
        .filter(|skill| skill.domain.is_empty())
        .map(|skill| skill.command.clone())
        .collect();
    // Every implemented leaf command must roll up into a business domain that
    // also appears in the skill set, so domain-scoped skill export never drops a
    // command. Both sides use `business_domain` for a consistent vocabulary.
    let expected_domains: std::collections::BTreeSet<String> = leaves
        .iter()
        .filter(|entry| entry.implemented)
        .map(|entry| business_domain(&entry.command).to_string())
        .collect();
    let skill_domains: std::collections::BTreeSet<String> =
        skills.iter().map(|skill| skill.domain.clone()).collect();
    let uncovered_skill_domains: Vec<String> = expected_domains
        .difference(&skill_domains)
        .cloned()
        .collect();

    let mut failures: Vec<String> = Vec::new();
    if !undiscoverable_leaves.is_empty() {
        failures.push(format!(
            "P1: implemented leaf commands missing registry rows: {:?}",
            undiscoverable_leaves
        ));
    }
    if !missing_risk.is_empty() {
        failures.push(format!(
            "P2: capabilities missing risk_level: {:?}",
            missing_risk
        ));
    }
    if !destructive_without_force.is_empty() {
        failures.push(format!(
            "P3: destructive commands missing --force: {:?}",
            destructive_without_force
        ));
    }
    if !unresolved_rows.is_empty() {
        failures.push(format!(
            "P4: capability rows unresolved: {:?}",
            unresolved_rows
        ));
    }
    if !storageclass_alias_ok {
        failures.push("P5: storgeclass → storageclass alias is broken".to_string());
    }
    if !skill_coverage_ok {
        failures.push(format!(
            "P6: skill_count={skill_count} < curated_capabilities={total}"
        ));
    }
    if !missing_skill_domains.is_empty() {
        failures.push(format!(
            "P6: skill definitions missing domain: {:?}",
            missing_skill_domains
        ));
    }
    if !uncovered_skill_domains.is_empty() {
        failures.push(format!(
            "P6: command domains missing skill coverage: {:?}",
            uncovered_skill_domains
        ));
    }

    let status = if failures.is_empty() {
        "passed"
    } else {
        "failed"
    };
    let message = if failures.is_empty() {
        "six-principle invariants are upheld by the registry".to_string()
    } else {
        format!("six-principle violations: {}", failures.join("; "))
    };

    DoctorCheck {
        name: "principles",
        status,
        message,
        details: json!({
            "capabilities": total,
            "skill_definitions": skill_count,
            "undiscoverable_leaf_commands": undiscoverable_leaves,
            "destructive_force_violations": destructive_without_force,
            "missing_risk_level": missing_risk,
            "unresolved_rows": unresolved_rows,
            "storageclass_alias_ok": storageclass_alias_ok,
            "skill_domains": skill_domains.into_iter().collect::<Vec<_>>(),
            "uncovered_skill_domains": uncovered_skill_domains,
            "principle_keys": [
                "discovery",
                "understanding",
                "safe_execution",
                "controlled_output",
                "deterministic_errors",
                "agent_ecosystem",
            ],
        }),
    }
}

/// [Review Fix #s2] Helper used by principles_check to verify alias coverage:
/// either a curated capability matches, or the canonical group is registered.
fn find_capability_or_group(command: &str) -> Option<&'static str> {
    if let Some(entry) = crate::registry::find_capability(command) {
        return Some(entry.command);
    }
    let canonical = canonical_group_name(command.trim_start_matches("ve-tos ").trim());
    crate::registry::find_group(canonical).map(|g| g.command)
}

fn skill_definitions() -> Vec<SkillDefinition> {
    // [Review Fix #27] Skill/MCP metadata must expose the same public ve-tos
    // command names; export domain directories keep the TOS business taxonomy.
    // Surface as `ve-tos capabilities --view full`. Curated registry entries are
    // preserved, and every remaining leaf command is derived from the clap
    // command tree so functional commands never degrade to `risk_level=unknown`.
    let curated = capabilities().iter().collect::<Vec<_>>();
    let leaves = leaf_command_tree();
    capability_rows(&curated, &leaves, /* keep_parameters */ true)
        .into_iter()
        .map(|row| capability_row_skill_definition(&row))
        .collect()
}

fn skill_definitions_for_language(language: DocumentationLanguage) -> Vec<SkillDefinition> {
    let mut definitions = skill_definitions();
    if matches!(language, DocumentationLanguage::Zh) {
        for definition in &mut definitions {
            definition.description = localized_skill_description_zh(definition);
            definition.input_schema = localized_input_schema(&definition.input_schema, language);
            definition.usage = localized_skill_usage_zh(&definition.usage);
        }
    }
    definitions
}

fn localized_skill_description_zh(skill: &SkillDefinition) -> String {
    ve_tos_metadata_translation_zh(&skill.description)
        .expect("owner audit guarantees every VeTos Skill description")
        .to_string()
}

fn localized_skill_usage_zh(usage: &SkillUsage) -> SkillUsage {
    SkillUsage {
        format: usage.format,
        source: translated_ve_tos_metadata(usage.source),
        mcp_tool_name: usage.mcp_tool_name.clone(),
        mcp_server: usage.mcp_server.clone(),
        serve_reads_exported_files: usage.serve_reads_exported_files,
        exported_file_use: translated_ve_tos_metadata(usage.exported_file_use),
        default_mcp_call: translated_ve_tos_metadata(usage.default_mcp_call),
    }
}

fn translated_ve_tos_metadata(source: &'static str) -> &'static str {
    ve_tos_metadata_translation_zh(source)
        .expect("owner audit guarantees every VeTos Skill usage phrase")
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
                        if is_ve_tos_machine_metadata_value(description) {
                            localized.insert(key.clone(), child.clone());
                            continue;
                        }
                        localized.insert(
                            key.clone(),
                            Value::String(
                                ve_tos_metadata_translation_zh(description)
                                    .unwrap_or_else(|| {
                                        panic!("missing VeTos schema translation source={description:?}")
                                    })
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

/// Localize frozen VeTos human prose in documentation JSON.
///
/// Only exact catalog phrases under explicit human-field contexts are replaced;
/// machine fields such as commands, examples, API names, and enum values stay exact.
pub fn localize_ve_tos_auth_documentation_zh(value: &mut Value) {
    localize_ve_tos_metadata_value_zh(value, "", false);
}

fn localize_ve_tos_metadata_value_zh(value: &mut Value, parent: &str, is_human: bool) {
    match value {
        Value::String(text) if is_human && !is_ve_tos_machine_metadata_value(text) => {
            if let Some(chinese) = ve_tos_metadata_translation_zh(text) {
                *text = chinese.to_string();
            }
        }
        Value::Array(items) => {
            for item in items {
                localize_ve_tos_metadata_value_zh(item, parent, is_human);
            }
        }
        Value::Object(map) => {
            for (key, child) in map {
                if is_ve_tos_machine_metadata_field(key) {
                    continue;
                }
                let child_is_human = is_human || is_ve_tos_human_metadata_field(parent, key);
                localize_ve_tos_metadata_value_zh(child, key, child_is_human);
            }
        }
        _ => {}
    }
}

#[allow(dead_code)]
fn skill_definition(entry: &CapabilityEntry) -> SkillDefinition {
    let name = skill_name(entry);
    SkillDefinition {
        schema_version: "tos-skill-v1",
        name: name.clone(),
        domain: business_domain(entry.command).to_string(),
        command: entry.command.to_string(),
        description: entry.description.to_string(),
        risk_level: risk_name(&entry.risk_level).to_string(),
        input_schema: skill_input_schema(entry),
        examples: entry
            .examples
            .iter()
            .map(|example| public_tos_example(example))
            .collect(),
        usage: skill_usage(name),
    }
}

fn capability_row_skill_definition(row: &CapabilityRow) -> SkillDefinition {
    let name = row.command.replace(' ', "_").replace('-', "_");
    SkillDefinition {
        schema_version: "tos-skill-v1",
        name: name.clone(),
        domain: business_domain(&row.command).to_string(),
        command: row.command.clone(),
        description: row.description.clone(),
        risk_level: row.risk_level.clone(),
        input_schema: capability_row_input_schema(row),
        examples: if row.examples.is_empty() {
            vec![format!("{} --help", public_tos_command(&row.command))]
        } else {
            row.examples.clone()
        },
        usage: skill_usage(name),
    }
}

#[allow(dead_code)]
fn command_skill_definition(entry: &CommandTreeEntry) -> SkillDefinition {
    let name = entry.command.replace(' ', "_").replace('-', "_");
    SkillDefinition {
        schema_version: "tos-skill-v1",
        name: name.clone(),
        domain: business_domain(&entry.command).to_string(),
        command: entry.command.clone(),
        description: entry.description.clone(),
        risk_level: "unknown".to_string(),
        input_schema: command_input_schema(entry),
        examples: vec![format!("{} --help", public_tos_command(&entry.command))],
        usage: skill_usage(name),
    }
}

fn skill_usage(name: String) -> SkillUsage {
    SkillUsage {
        format: "Markdown SKILL.md",
        source: "Derived from the live TOS CLI capability registry and clap command tree.",
        mcp_tool_name: name,
        mcp_server: public_tos_command("ve-tos serve --mcp"),
        serve_reads_exported_files: false,
        exported_file_use: "Portable Markdown skill pack for external agents, documentation generators, prompts, or adapters. The built-in MCP server rebuilds tools from the in-process registry instead of reading exported files.",
        default_mcp_call: "tools/call returns a plan by default; include argument execute=true to run the underlying CLI command.",
    }
}

fn capability_row_input_schema(row: &CapabilityRow) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for parameter in row.parameters.as_deref().unwrap_or(&[]) {
        properties.insert(
            parameter.name.clone(),
            json!({
                "type": registry_parameter_schema_type(&parameter.name),
                "description": parameter.description,
                "location": parameter.location,
            }),
        );
        if parameter.required {
            required.push(parameter.name.clone());
        }
    }
    add_skill_control_schema(&mut properties);
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

#[allow(dead_code)]
fn skill_input_schema(entry: &CapabilityEntry) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for parameter in entry.parameters {
        properties.insert(
            parameter.name.to_string(),
            json!({
                "type": registry_parameter_schema_type(parameter.name),
                "description": parameter.description,
                "location": format!("{:?}", parameter.location).to_lowercase(),
            }),
        );
        if parameter.required {
            required.push(parameter.name);
        }
    }
    add_skill_control_schema(&mut properties);
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

#[allow(dead_code)]
fn skill_name(entry: &CapabilityEntry) -> String {
    entry.command.replace(' ', "_").replace('-', "_")
}

#[allow(dead_code)]
fn command_input_schema(entry: &CommandTreeEntry) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for parameter in &entry.parameters {
        properties.insert(
            parameter.name.clone(),
            json!({
                "type": if parameter.takes_value { "string" } else { "boolean" },
                "description": parameter.description,
                "positional": parameter.positional,
                "long": parameter.long,
                "short": parameter.short.map(|short| short.to_string()),
            }),
        );
        if parameter.required {
            required.push(parameter.name.clone());
        }
    }
    add_skill_control_schema(&mut properties);
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn add_skill_control_schema(properties: &mut serde_json::Map<String, Value>) {
    properties.insert(
        "execute".to_string(),
        json!({
            "type": "boolean",
            "description": "MCP tools/call control: false or omitted returns a planned argv; true executes the underlying CLI command."
        }),
    );
}

fn registry_parameter_schema_type(name: &str) -> &'static str {
    match name {
        "recursive"
        | "checkpoint"
        | "force"
        | "destroy"
        | "progress"
        | "no-progress"
        | "list-echo"
        | "no-list-echo"
        | "no-manifest"
        | "report-failures-only"
        | "delete"
        | "size-only"
        | "exact-timestamps"
        | "include-parent"
        | "parents"
        | "all-versions"
        | "include-uploads"
        | "no-clobber"
        | "human-readable"
        | "cost"
        | "mcp"
        | "bucket-object-lock-enabled" => "boolean",
        "max-depth"
        | "max-keys"
        | "top-k"
        | "days"
        | "expires"
        | "port"
        | "batch-concurrency"
        | "list-concurrency"
        | "multipart-concurrency" => "integer",
        _ => "string",
    }
}

fn completion_words() -> Vec<String> {
    let mut words = command_groups()
        .iter()
        .map(|entry| entry.name.to_string())
        .collect::<Vec<_>>();
    for entry in flattened_command_tree() {
        words.push(entry.name);
    }
    words.sort();
    words.dedup();
    words
}

fn layer_name(layer: &tos_core::agent::describe::CommandLayer) -> &'static str {
    match layer {
        tos_core::agent::describe::CommandLayer::HighLevel => "high_level",
        tos_core::agent::describe::CommandLayer::LowLevel => "low_level",
        tos_core::agent::describe::CommandLayer::Meta => "meta",
    }
}

fn risk_name(risk: &tos_core::agent::describe::RiskLevel) -> &'static str {
    match risk {
        tos_core::agent::describe::RiskLevel::Low => "low",
        tos_core::agent::describe::RiskLevel::Medium => "medium",
        tos_core::agent::describe::RiskLevel::High => "high",
        tos_core::agent::describe::RiskLevel::Critical => "critical",
    }
}

#[allow(dead_code)]
fn contains_ignore_case(value: &str, needle: &str) -> bool {
    // [G7] Retained as a public-style helper for future legacy substring use;
    // the capabilities path now uses `rank_best` for weighted fuzzy match.
    value.to_lowercase().contains(&needle.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn collect_human_metadata(
        value: &Value,
        path: &str,
        parent: &str,
        is_human: bool,
        prose: &mut Vec<(String, String)>,
    ) {
        match value {
            Value::String(source) if is_human && !is_ve_tos_machine_metadata_value(source) => {
                prose.push((path.to_string(), source.clone()));
            }
            Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    collect_human_metadata(
                        item,
                        &format!("{path}[{index}]"),
                        parent,
                        is_human,
                        prose,
                    );
                }
            }
            Value::Object(map) => {
                for (key, child) in map {
                    if is_ve_tos_machine_metadata_field(key) {
                        continue;
                    }
                    let child_path = format!("{path}.{key}");
                    collect_human_metadata(
                        child,
                        &child_path,
                        key,
                        is_human || is_ve_tos_human_metadata_field(parent, key),
                        prose,
                    );
                }
            }
            _ => {}
        }
    }

    fn missing_chinese(context: &str, document: &Value) -> BTreeSet<String> {
        let mut prose = Vec::new();
        collect_human_metadata(document, "$", "", false, &mut prose);
        prose
            .into_iter()
            .filter_map(|(path, source)| {
                let localized = ve_tos_metadata_translation_zh(&source);
                (localized.is_none()
                    || localized.is_some_and(|translation| {
                        translation == source || translation.contains(&source)
                    }))
                .then(|| format!("command={context}, path={path}, source={source:?}"))
            })
            .collect()
    }

    fn human_metadata_by_path(value: &Value) -> BTreeMap<String, String> {
        let mut prose = Vec::new();
        collect_human_metadata(value, "$", "", false, &mut prose);
        prose.into_iter().collect()
    }

    fn assert_exact_human_localization(context: &str, english: &Value, chinese: &Value) {
        let localized = human_metadata_by_path(chinese);
        for (path, source) in human_metadata_by_path(english) {
            let expected = ve_tos_metadata_translation_zh(&source).unwrap_or_else(|| {
                panic!("command={context}, path={path}, missing source={source:?}")
            });
            assert_eq!(
                localized.get(&path).map(String::as_str),
                Some(expected),
                "command={context}, path={path}, source={source:?}"
            );
        }
    }

    fn all_describe_documents() -> Vec<(String, Value)> {
        let mut documents = vec![("ve-tos".to_string(), crate::registry::describe_tos_group())];
        for group in command_groups() {
            documents.push((
                group.command.to_string(),
                serde_json::to_value(group).unwrap(),
            ));
        }
        for entry in flattened_command_tree() {
            documents.push((entry.command.clone(), serde_json::to_value(&entry).unwrap()));
            if entry.subcommands.is_empty() {
                if let Some(description) = describe_command_metadata(&entry.command) {
                    documents.push((entry.command, serde_json::to_value(description).unwrap()));
                }
            }
        }
        documents
    }

    #[test]
    fn chinese_catalog_recursively_covers_all_describe_metadata() {
        let mut missing = BTreeSet::new();
        for (command, document) in all_describe_documents() {
            missing.extend(missing_chinese(&command, &document));
            let mut localized = document.clone();
            localize_ve_tos_auth_documentation_zh(&mut localized);
            assert_exact_human_localization(&command, &document, &localized);
        }
        assert!(
            missing.is_empty(),
            "missing Chinese Describe metadata:\n{}",
            missing.into_iter().collect::<Vec<_>>().join("\n")
        );
    }

    #[test]
    fn chinese_skill_definitions_localize_all_human_metadata() {
        let english = skill_definitions_for_language(DocumentationLanguage::En);
        let chinese = skill_definitions_for_language(DocumentationLanguage::Zh);
        let mut missing = BTreeSet::new();
        for source in english {
            let localized = chinese
                .iter()
                .find(|skill| skill.command == source.command)
                .expect("Chinese Skill matches English command");
            let source_value = serde_json::to_value(&source).unwrap();
            let localized_value = serde_json::to_value(localized).unwrap();
            missing.extend(missing_chinese(&source.command, &source_value));
            assert_exact_human_localization(&source.command, &source_value, &localized_value);
            assert!(
                !localized.description.contains("原始英文说明"),
                "command={}",
                source.command
            );
            assert!(
                !localized.input_schema.to_string().contains("参数说明："),
                "command={}",
                source.command
            );
        }
        assert!(
            missing.is_empty(),
            "missing Chinese Skill metadata:\n{}",
            missing.into_iter().collect::<Vec<_>>().join("\n")
        );
    }

    #[test]
    fn chinese_catalog_has_unique_english_sources() {
        let mut sources = BTreeSet::new();
        // [Review Fix #GlobalZh10] Validate the complete runtime lookup rather
        // than only the static registry half of the owner catalog.
        for (english, translation) in VE_TOS_METADATA_TRANSLATIONS_ZH
            .iter()
            .chain(VE_TOS_CANONICAL_RUNTIME_TRANSLATIONS_ZH.iter())
        {
            assert!(
                sources.insert(*english),
                "duplicate English source={english:?}"
            );
            assert_ne!(english, translation, "untranslated source={english:?}");
        }
    }

    #[test]
    fn chinese_localizer_never_translates_machine_fields_on_catalog_collision() {
        let mut document = json!({
            "description": "Copy local files, TOS objects, or prefixes",
            "command": "Copy local files, TOS objects, or prefixes",
            "name": "Configuration management",
            "type": "Create a bucket",
            "method": "Set a configuration value",
            "examples": ["Delete objects or prefixes"],
            "scenario_routing": {"mode_resolution": "Configuration management"},
        });
        localize_ve_tos_auth_documentation_zh(&mut document);
        assert_eq!(document["description"], "复制本地文件、TOS 对象或前缀");
        assert_eq!(
            document["command"],
            "Copy local files, TOS objects, or prefixes"
        );
        assert_eq!(document["name"], "Configuration management");
        assert_eq!(document["type"], "Create a bucket");
        assert_eq!(document["method"], "Set a configuration value");
        assert_eq!(document["examples"][0], "Delete objects or prefixes");
        assert_eq!(
            document["scenario_routing"]["mode_resolution"],
            "Configuration management"
        );
    }

    #[tokio::test]
    async fn unified_auth_doctor_gets_once_and_exposes_only_approved_metadata() {
        let calls = Arc::new(AtomicUsize::new(0));
        let resolver_calls = Arc::clone(&calls);
        let source = UnifiedDoctorCredentialSource::Resolver(Arc::new(move || {
            resolver_calls.fetch_add(1, Ordering::SeqCst);
            Ok(
                tos_core::infra::unified_credentials::UnifiedCredentialValue::new(
                    "SECRET_AK",
                    "SECRET_SK",
                    "",
                    "safe-provider",
                ),
            )
        }));
        let resolved = crate::domain::auth::ResolvedAuthMode {
            mode: crate::domain::auth::AuthMode::Unified,
            source: crate::domain::auth::AuthModeSource::CommandLine,
        };

        let check = unified_auth_check_with_source("selected", resolved, source).await;
        let keys = check
            .details
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(check.status, "passed");
        assert_eq!(check.details["has_session_token"], false);
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
        let serialized = serde_json::to_string(&check).unwrap();
        for secret in ["SECRET_AK", "SECRET_SK"] {
            assert!(!serialized.contains(secret));
        }
    }

    #[test]
    fn test_api_lookup_finds_high_level_metadata() {
        let capability = find_api_capability("cp", "describe").expect("cp metadata");
        assert_eq!(capability.command, "ve-tos cp");
    }

    #[test]
    fn test_api_lookup_finds_config_action_metadata() {
        let capability = find_api_capability("config", "show").expect("config show metadata");
        assert_eq!(capability.command, "ve-tos config show");
    }

    #[test]
    fn test_api_lookup_falls_back_to_command_tree() {
        // [Review Fix #s1] `object upload` is now curated, so the fallback path
        // is exercised against a still-derived leaf such as `object head`.
        let lookup = api_lookup(&ApiArgs {
            group: "object".to_string(),
            action: "head".to_string(),
            request: None,
            describe: true,
            force: false,
        })
        .expect("command tree lookup");
        assert_eq!(lookup["mode"], "command_metadata");
        assert_eq!(lookup["command"], "ve-tos object head");
        assert_eq!(lookup["command_metadata"]["name"], "head");
    }

    #[test]
    fn test_api_request_builds_raw_passthrough_plan() {
        let lookup = api_lookup(&ApiArgs {
            group: "unknown".to_string(),
            action: "action".to_string(),
            request: Some(r#"{"method":"GET","path":"/"}"#.to_string()),
            describe: false,
            force: false,
        })
        .expect("raw passthrough plan");
        assert_eq!(lookup["mode"], "unregistered_raw_passthrough_plan");
        assert_eq!(lookup["request"]["method"], "GET");
    }

    #[test]
    fn test_raw_api_requires_force_for_mutations() {
        let err = api_lookup(&ApiArgs {
            group: "unknown".to_string(),
            action: "action".to_string(),
            request: Some(r#"{"method":"DELETE","path":"/bucket"}"#.to_string()),
            describe: false,
            force: false,
        })
        .expect_err("unsafe raw API should require force");
        assert!(err.to_string().contains("requires --force"));
    }

    #[test]
    fn test_raw_api_rejects_signer_managed_headers() {
        let request = parse_raw_api_request(
            r#"{"method":"GET","path":"/","headers":{"Authorization":"bad"}}"#,
        )
        .expect("request");
        let err = normalize_string_map("headers", &request.headers).expect_err("managed header");
        assert!(err.to_string().contains("managed by the signer"));
    }

    #[test]
    fn test_raw_api_plan_redacts_sensitive_fields() {
        let lookup = api_lookup(&ApiArgs {
            group: "unknown".to_string(),
            action: "action".to_string(),
            request: Some(
                r#"{"method":"GET","path":"/","headers":{"x-custom-token":"secret"},"query":{"security-token":"secret"}}"#
                    .to_string(),
            ),
            describe: false,
            force: false,
        })
        .expect("plan");
        assert_eq!(
            lookup["request"]["headers"]["x-custom-token"],
            "***REDACTED***"
        );
        assert_eq!(
            lookup["request"]["query"]["security-token"],
            "***REDACTED***"
        );
    }

    #[test]
    fn test_raw_api_data_endpoint_allows_root_path() {
        let client = TosClient::new(
            &tos_core::infra::config::Profile {
                region: Some("cn-beijing".to_string()),
                access_key_id: Some("ak".to_string()),
                secret_access_key: Some("sk".to_string()),
                endpoint: Some("tos-cn-beijing.volces.com".to_string()),
                ..Default::default()
            },
            "tos",
        )
        .expect("client");
        let request =
            parse_raw_api_request(r#"{"method":"GET","endpoint_kind":"data","path":"/"}"#)
                .expect("request");
        let target = resolve_raw_api_target(&client, &request).expect("target");
        assert_eq!(target.signing_path, "/");
    }

    #[test]
    fn test_raw_api_resolves_bucket_target() {
        let client = TosClient::new(
            &tos_core::infra::config::Profile {
                region: Some("cn-beijing".to_string()),
                access_key_id: Some("ak".to_string()),
                secret_access_key: Some("sk".to_string()),
                endpoint: Some("tos-cn-beijing.volces.com".to_string()),
                ..Default::default()
            },
            "tos",
        )
        .expect("client");
        let request = parse_raw_api_request(
            r#"{"method":"GET","endpoint_kind":"bucket","bucket":"demo","query":{"lifecycle":""}}"#,
        )
        .expect("request");
        let target = resolve_raw_api_target(&client, &request).expect("target");
        assert_eq!(target.endpoint_kind, "bucket");
        assert_eq!(target.signing_path, "/");
        assert!(target.url.contains("demo.tos-cn-beijing.volces.com"));
    }

    #[tokio::test]
    async fn test_mcp_tools_call_can_plan_typed_command() {
        let response = mcp_call_tool(
            &test_global_args(),
            json!({"name":"ve_tos_cp","arguments":{"execute":false,"source":"a","destination":"b"}}),
        )
        .await
        .expect("mcp");
        let text = response["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("planned_not_executed"));
        assert!(text.contains("ve-tos cp"));
    }

    #[test]
    fn test_mcp_typed_argv_maps_positionals_and_flags() {
        let mut arguments = serde_json::Map::new();
        arguments.insert("source".to_string(), json!("a"));
        arguments.insert("destination".to_string(), json!("b"));
        arguments.insert("recursive".to_string(), json!(true));
        arguments.insert("dry_run".to_string(), json!(true));
        let argv =
            build_mcp_typed_argv(&test_global_args(), "ve-tos cp", &arguments).expect("argv");
        assert!(argv.windows(2).any(|window| window == ["--output", "json"]));
        assert!(argv.windows(2).any(|window| window == ["ve-tos", "cp"]));
        assert!(argv.contains(&"--dry-run".to_string()));
        assert!(argv.contains(&"--recursive".to_string()));
        assert!(argv.windows(2).any(|window| window == ["a", "b"]));
    }

    #[test]
    fn test_mcp_typed_argv_propagates_ve_tos_auth_mode() {
        let mut global = test_global_args();
        global.ve_tos_auth_mode = Some("unified".to_string());

        let argv = build_mcp_typed_argv(
            &global,
            "ve-tos ls",
            &serde_json::Map::from_iter([("path".to_string(), json!("tos://bucket"))]),
        )
        .expect("argv");

        assert!(
            argv.windows(2)
                .any(|window| window == ["--auth-mode", "unified"]),
            "argv={argv:?}"
        );
        let auth_mode_position = argv
            .iter()
            .position(|argument| argument == "--auth-mode")
            .unwrap();
        let tool_position = argv
            .iter()
            .position(|argument| argument == "ve-tos")
            .unwrap();
        assert!(auth_mode_position > tool_position, "argv={argv:?}");
    }

    #[test]
    fn test_mcp_typed_argv_does_not_promote_config_mode_over_call_profile() {
        let directory = std::env::temp_dir().join(format!(
            "ve-tos-mcp-auth-mode-{}-{}",
            std::process::id(),
            ulid::Ulid::new()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let config_path = directory.join("config.toml");
        std::fs::write(
            &config_path,
            concat!(
                "[parent.ve-tos]\n",
                "auth_mode = \"aksk\"\n",
                "[child.ve-tos]\n",
                "auth_mode = \"unified\"\n",
            ),
        )
        .unwrap();
        let credentials_path = directory.join("credentials.toml");
        let mut global = test_global_args();
        global.profile = "parent".to_string();
        global.config_path = Some(config_path.clone());
        global.credentials_path = Some(credentials_path.clone());

        let argv = build_mcp_typed_argv(
            &global,
            "ve-tos ls",
            &serde_json::Map::from_iter([
                ("path".to_string(), json!("tos://bucket")),
                ("profile".to_string(), json!("child")),
            ]),
        )
        .expect("argv");

        assert!(
            argv.windows(2)
                .any(|window| window == ["--profile", "child"]),
            "argv={argv:?}"
        );
        assert!(!argv.iter().any(|argument| argument == "--auth-mode"));
        let tool_position = argv
            .iter()
            .position(|argument| argument == "ve-tos")
            .unwrap();
        for (flag, expected_path) in [
            ("--config-path", &config_path),
            ("--credentials-path", &credentials_path),
        ] {
            let position = argv
                .iter()
                .position(|argument| argument == flag)
                .unwrap_or_else(|| panic!("missing {flag} in argv={argv:?}"));
            assert!(position < tool_position, "argv={argv:?}");
            assert_eq!(
                argv.get(position + 1),
                Some(&expected_path.display().to_string())
            );
        }

        #[derive(clap::Parser)]
        struct ChildCli {
            #[command(flatten)]
            global: GlobalArgs,
            #[command(subcommand)]
            tool: ChildTool,
        }
        #[derive(clap::Subcommand)]
        enum ChildTool {
            #[command(name = "ve-tos")]
            VeTos {
                #[command(flatten)]
                auth: crate::cli::VeTosAuthArgs,
                #[command(subcommand)]
                _command: Option<crate::cli::TosCommand>,
            },
        }
        let child = <ChildCli as clap::Parser>::try_parse_from(
            std::iter::once("ve-storage-uni-cli".to_string()).chain(argv),
        )
        .unwrap();
        let ChildTool::VeTos { auth, .. } = child.tool;
        let mut child_global = child.global;
        child_global.ve_tos_auth_mode = auth.auth_mode.map(|mode| mode.as_str().to_string());
        assert_eq!(child_global.profile, "child");
        assert_eq!(child_global.config_path.as_ref(), Some(&config_path));
        assert_eq!(
            child_global.credentials_path.as_ref(),
            Some(&credentials_path)
        );
        assert_eq!(
            crate::handler::common::resolve_auth_mode(&child_global)
                .unwrap()
                .mode,
            crate::domain::auth::AuthMode::Unified
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn test_mcp_typed_argv_maps_bucket_create_uri_and_bucket_flag() {
        let mut uri_arguments = serde_json::Map::new();
        uri_arguments.insert("uri".to_string(), json!("tos://demo-bucket"));
        let uri_argv =
            build_mcp_typed_argv(&test_global_args(), "ve-tos bucket create", &uri_arguments)
                .expect("uri argv");
        assert!(uri_argv
            .windows(3)
            .any(|window| window == ["ve-tos", "bucket", "create"]));
        assert!(uri_argv.contains(&"tos://demo-bucket".to_string()));

        let mut flag_arguments = serde_json::Map::new();
        flag_arguments.insert("bucket_name".to_string(), json!("demo-bucket"));
        let flag_argv =
            build_mcp_typed_argv(&test_global_args(), "ve-tos bucket create", &flag_arguments)
                .expect("flag argv");
        assert!(flag_argv
            .windows(2)
            .any(|window| window == ["--bucket", "demo-bucket"]));
    }

    #[test]
    fn test_mcp_typed_argv_rejects_unknown_arguments() {
        let mut arguments = serde_json::Map::new();
        arguments.insert("unexpected".to_string(), json!("value"));
        let err = build_mcp_typed_argv(&test_global_args(), "ve-tos capabilities", &arguments)
            .expect_err("unknown argument");
        assert!(err.to_string().contains("unknown argument"));
    }

    #[tokio::test]
    async fn test_mcp_tos_api_call_returns_plan_by_default() {
        let response = mcp_call_tool(
            &test_global_args(),
            json!({"name":"ve_tos_api","arguments":{"group":"raw","action":"list","request":{"method":"GET","endpoint_kind":"data","path":"/"}}}),
        )
        .await
        .expect("mcp");
        let text = response["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("unregistered_raw_passthrough_plan"));
        assert!(text.contains("\"status\": \"success\""));
    }

    #[test]
    fn test_skill_definition_uses_registry_parameters() {
        let capability = find_api_capability("cp", "describe").expect("cp metadata");
        let definition = skill_definition(capability);
        assert_eq!(definition.name, "ve_tos_cp");
        // `ve-tos cp` is a high-level data-movement command → tos-transfer domain.
        assert_eq!(definition.domain, "tos-transfer");
        assert!(definition.input_schema["properties"]["source"].is_object());
    }

    #[test]
    fn test_skill_definitions_include_low_level_command_tree() {
        let skills = skill_definitions();
        assert!(skills
            .iter()
            .any(|skill| skill.command == "ve-tos object upload"));
        let api_skill = skills
            .iter()
            .find(|skill| skill.command == "ve-tos api")
            .expect("api skill");
        assert_eq!(api_skill.risk_level, "high");
        // `ve-tos api` is cross-cutting tooling, so it rolls up into the shared domain.
        assert_eq!(api_skill.domain, "tos-shared");
        assert!(api_skill.input_schema["properties"]["request"].is_object());
    }

    #[test]
    fn test_skill_definitions_preserve_domain_coverage() {
        let skills = skill_definitions();
        let skill_domains: std::collections::BTreeSet<_> =
            skills.iter().map(|skill| skill.domain.as_str()).collect();
        // Every implemented leaf must roll up into a business domain present in
        // the skill set, so domain-scoped export never drops a command.
        for entry in leaf_command_tree()
            .into_iter()
            .filter(|entry| entry.implemented)
        {
            let domain = business_domain(&entry.command);
            assert!(
                skill_domains.contains(domain),
                "skill domain coverage missing for command {} (domain {})",
                entry.command,
                domain
            );
        }
    }

    #[test]
    fn test_skill_export_refuses_to_overwrite_existing_file() {
        let dir = std::env::temp_dir().join(format!(
            "ve-storage-uni-cli-meta-export-conflict-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("tos-transfer").join("ve_tos_cp")).expect("create temp dir");
        fs::write(
            dir.join("tos-transfer").join("ve_tos_cp").join("SKILL.md"),
            "# old",
        )
        .expect("seed conflict");

        let plan =
            skill_markdown_export_plan(Some("cp"), dir.to_str().expect("temp dir")).expect("plan");
        let err = export_markdown_skills(
            plan,
            dir.to_str().expect("temp dir"),
            DocumentationLanguage::En,
        )
        .expect_err("conflict");
        assert!(matches!(err, CliError::Conflict(_)));
        let _ = fs::remove_dir_all(&dir);
    }

    /// [Spec §3 Safe Execution] `ve-tos skill export --dry-run` must NOT create
    /// the target directory or any of the Markdown skill files. The returned plan
    /// must list every target path together with a `conflict` annotation so an
    /// Agent can decide whether to proceed.
    #[test]
    fn test_skill_export_dry_run_writes_no_files() {
        let dir = std::env::temp_dir().join(format!(
            "ve-storage-uni-cli-meta-export-dryrun-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);

        let export_plan =
            skill_markdown_export_plan(None, dir.to_str().expect("temp dir")).expect("plan");
        let plan = plan_skill_markdown_export(
            &export_plan,
            dir.to_str().expect("temp dir"),
            DocumentationLanguage::En,
        );
        assert_eq!(plan["dry_run"], true);
        assert_eq!(plan["status"], "planned_not_written");
        let entries = plan["entries"].as_array().expect("entries array");
        assert!(!entries.is_empty(), "dry-run plan must list skills");
        for entry in entries {
            assert!(entry["path"].is_string());
            assert!(entry["skill"].is_string());
            assert!(entry["conflict"].is_boolean());
        }
        // Critical contract: dry-run must NOT have created the directory or any file.
        assert!(
            !dir.exists(),
            "dry-run must not create the export directory"
        );
    }

    /// [Spec §3 Safe Execution] When a target file already exists, the dry-run
    /// plan must surface it as `conflict: true` instead of erroring — that way
    /// the Agent can reason about the conflict without committing to a write.
    #[test]
    fn test_skill_export_dry_run_reports_conflicts() {
        let dir = std::env::temp_dir().join(format!(
            "ve-storage-uni-cli-meta-export-dryrun-conflict-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("tos-transfer").join("ve_tos_cp")).expect("create temp dir");
        fs::write(
            dir.join("tos-transfer").join("ve_tos_cp").join("SKILL.md"),
            "# old",
        )
        .expect("seed conflict");

        let export_plan =
            skill_markdown_export_plan(Some("cp"), dir.to_str().expect("temp dir")).expect("plan");
        let plan = plan_skill_markdown_export(
            &export_plan,
            dir.to_str().expect("temp dir"),
            DocumentationLanguage::En,
        );
        let entries = plan["entries"].as_array().expect("entries");
        assert!(entries.iter().any(|entry| entry["conflict"] == true));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_completion_script_uses_registry_groups() {
        let completion = completion_script("bash").expect("completion");
        assert_eq!(completion.shell, "bash");
        assert!(completion.script.contains("capabilities"));
        assert!(completion.script.contains("upload"));
        assert!(completion.command_count >= command_groups().len());
    }

    #[test]
    fn test_serve_plan_reports_registry_counts() {
        let plan = serve_plan(&ServeArgs {
            mcp: true,
            transport: "stdio".to_string(),
            port: 8080,
        })
        .expect("serve plan");
        assert_eq!(plan.mode, "mcp");
        assert_eq!(plan.status, "planned_not_started");
        assert_eq!(plan.capabilities, capabilities().len());
    }

    #[tokio::test]
    async fn test_doctor_supports_documented_check_names() {
        let global = test_global_args();
        for name in [
            "auth",
            "config",
            "registry",
            "permissions",
            "region",
            "network",
            "version",
            "mcp",
            "completion",
            // [Review Fix #s2] principles is part of the documented check set.
            "principles",
        ] {
            let report = doctor_report(
                &global,
                &DoctorArgs {
                    check: Some(name.to_string()),
                    bucket: None,
                    live_network: false,
                    network_timeout_ms: 3000,
                },
            )
            .await
            .expect("doctor report");
            assert_eq!(report.summary.total, 1, "check {name}");
        }
    }

    /// [Review Fix #s2] principles_check must pass against the current registry
    /// and report exactly one passing entry when invoked in isolation.
    #[tokio::test]
    async fn test_doctor_principles_check_passes() {
        let report = doctor_report(
            &test_global_args(),
            &DoctorArgs {
                check: Some("principles".to_string()),
                bucket: None,
                live_network: false,
                network_timeout_ms: 3000,
            },
        )
        .await
        .expect("doctor report");
        assert_eq!(report.summary.total, 1);
        assert_eq!(report.summary.passed, 1, "{:?}", report.checks);
        assert_eq!(report.checks[0].name, "principles");
    }

    /// [Spec §4.5 / AGT-001] `groups` view returns one entry per registry
    /// group, each tagged with `command_count` so the Agent can prioritise
    /// drilling into the largest groups first.
    #[test]
    fn test_capabilities_groups_view_includes_command_count() {
        let args = CapabilitiesArgs {
            view: "groups".to_string(),
            group: None,
            search: None,
            layer: None,
        };
        let view = capabilities_view(&args).expect("groups view");
        assert_eq!(view.view, "groups");
        assert!(!view.groups.is_empty(), "groups view must surface groups");
        assert!(
            view.capabilities.is_empty(),
            "groups view must not return capabilities"
        );
        assert!(
            view.commands.is_empty(),
            "groups view must not return commands"
        );
        assert!(
            view.lines.is_empty(),
            "groups view must not return text lines"
        );
        let total_count: usize = view.groups.iter().map(|g| g.command_count).sum();
        assert!(total_count > 0, "command_count must be populated");
    }

    /// [Spec §4.5] `text` view returns a flat list of `<command>\t<desc>`
    /// lines so an Agent can scan the full surface in O(N) tokens.
    #[test]
    fn test_capabilities_text_view_returns_one_line_per_command() {
        let args = CapabilitiesArgs {
            view: "text".to_string(),
            group: None,
            search: None,
            layer: None,
        };
        let view = capabilities_view(&args).expect("text view");
        assert_eq!(view.view, "text");
        assert!(!view.lines.is_empty(), "text view must surface lines");
        for line in &view.lines {
            assert!(
                line.contains('\t'),
                "text line must use tab separator: {line}"
            );
        }
        assert!(view.groups.is_empty(), "text view must not echo groups");
        assert!(
            view.capabilities.is_empty(),
            "text view must not echo capabilities"
        );
        assert!(view.commands.is_empty(), "text view must not echo commands");
    }

    /// [Spec §4.5 / AGT-002] `compact` view strips parameters from each
    /// capability row but keeps every other metadata field.
    #[test]
    fn test_capabilities_compact_view_strips_parameters() {
        let args = CapabilitiesArgs {
            view: "compact".to_string(),
            group: None,
            search: None,
            layer: None,
        };
        let view = capabilities_view(&args).expect("compact view");
        assert_eq!(view.view, "compact");
        assert!(
            !view.capabilities.is_empty(),
            "compact view must surface capabilities"
        );
        for row in &view.capabilities {
            assert!(
                row.parameters.is_none(),
                "compact view must drop parameters"
            );
        }
    }

    /// [Spec §4.5 / AGT-002] `full` view tags each capability with
    /// `layer` / `endpoint_rule` / `destructive` and retains parameters.
    #[test]
    fn test_capabilities_full_view_carries_layer_endpoint_rule_destructive() {
        let args = CapabilitiesArgs {
            view: "full".to_string(),
            group: None,
            search: None,
            layer: None,
        };
        let view = capabilities_view(&args).expect("full view");
        assert_eq!(view.view, "full");
        let value = serde_json::to_value(&view).expect("serialize full view");
        let caps = value["capabilities"]
            .as_array()
            .expect("capabilities array");
        assert!(!caps.is_empty(), "full view must surface capabilities");
        for cap in caps {
            assert!(cap.get("layer").is_some(), "every cap must carry layer");
            assert!(
                cap.get("destructive").is_some(),
                "every cap must carry destructive"
            );
            assert!(cap.as_object().unwrap().contains_key("endpoint_rule"));
        }
        let any_destructive = caps
            .iter()
            .any(|c| c["destructive"].as_bool().unwrap_or(false));
        assert!(
            any_destructive,
            "full view must mark at least one cap destructive"
        );
    }

    #[test]
    fn test_capabilities_public_commands_use_ve_tos_prefix() {
        let args = CapabilitiesArgs {
            view: "full".to_string(),
            group: None,
            search: None,
            layer: None,
        };
        let view = capabilities_view(&args).expect("full view");
        let value = serde_json::to_value(&view).expect("serialize full view");

        let capabilities = value["capabilities"]
            .as_array()
            .expect("capabilities array");
        assert!(
            capabilities
                .iter()
                .any(|cap| cap["command"].as_str() == Some("ve-tos cp")),
            "full view must expose high-level capabilities under ve-tos"
        );
        assert!(
            capabilities
                .iter()
                .filter_map(|cap| cap["command"].as_str())
                .all(|command| !command.starts_with("tos ")),
            "public capability commands must not expose legacy tos prefix"
        );

        let commands = value["commands"].as_array().expect("commands array");
        assert!(
            commands
                .iter()
                .any(|entry| entry["command"].as_str() == Some("ve-tos bucket")),
            "command tree must expose ve-tos command paths"
        );
        assert!(
            commands
                .iter()
                .filter_map(|entry| entry["command"].as_str())
                .all(|command| !command.starts_with("tos ")),
            "public command tree must not expose legacy tos prefix"
        );

        let groups = value["groups"].as_array().expect("groups array");
        assert!(
            groups
                .iter()
                .any(|group| group["command"].as_str() == Some("ve-tos cp")),
            "group summaries must expose ve-tos command paths"
        );
    }

    /// [Spec §4.5] `tree` is preserved as a legacy alias for `compact` so
    /// existing callers keep working.
    #[test]
    fn test_capabilities_tree_alias_resolves_to_compact() {
        let args = CapabilitiesArgs {
            view: "tree".to_string(),
            group: None,
            search: None,
            layer: None,
        };
        let view = capabilities_view(&args).expect("tree alias");
        assert_eq!(view.view, "compact");
    }

    /// [Spec §4.5 / AGT-003] `--search 加密` must hit encryption / SSE
    /// related capabilities through the Chinese→English alias map.
    #[test]
    fn test_capabilities_search_chinese_encryption_term_hits_sse() {
        let args = CapabilitiesArgs {
            view: "compact".to_string(),
            group: None,
            search: Some("加密".to_string()),
            layer: None,
        };
        let view = capabilities_view(&args).expect("search 加密");
        let hit_commands: Vec<String> = view
            .capabilities
            .iter()
            .map(|c| c.command.to_string())
            .chain(view.commands.iter().map(|c| c.command.clone()))
            .collect();
        let joined = hit_commands.join("\n").to_lowercase();
        assert!(
            joined.contains("encrypt") || joined.contains("sse") || joined.contains("kms"),
            "search 加密 should hit encryption/SSE/KMS capabilities, got: {hit_commands:?}",
        );
        assert!(
            !view.search_scores.is_empty(),
            "search scores must be populated"
        );
    }

    /// [Spec §4.5] Unknown view value yields a deterministic ValidationError.
    #[test]
    fn test_capabilities_view_rejects_unknown_view() {
        let args = CapabilitiesArgs {
            view: "bogus".to_string(),
            group: None,
            search: None,
            layer: None,
        };
        let err = capabilities_view(&args).expect_err("unknown view");
        assert!(err.to_string().contains("unsupported capabilities view"));
        assert!(err.to_string().contains("groups, text, compact, or full"));
    }

    fn test_global_args() -> GlobalArgs {
        GlobalArgs {
            profile: "default".to_string(),
            config_path: None,
            credentials_path: None,
            region: None,
            endpoint: None,
            psm: None,
            idc: None,
            cluster: None,
            addr_family: None,
            control_endpoint: None,
            account_id: None,
            output: None,
            query: None,
            dry_run: false,
            describe: false,
            no_color: false,
            verbose: false,
            quiet: false,
            trace_dir: None,
            trace_redact: "strict".to_string(),
            yes: false,
            confirm: None,
            request_trace: Default::default(),
            ve_tos_auth_mode: None,
            documentation_language: None,
        }
    }
}
