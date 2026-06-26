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

use clap::Parser;
use std::path::PathBuf;

/// 所有 TOS CLI 工具共享的全局参数。
///
/// 这些参数可在任意命令上指定，用于控制输出格式、
/// 认证信息、服务端点选择等。
#[derive(Parser, Debug, Clone)]
pub struct GlobalArgs {
    /// 输出格式：json、table、yaml、csv、markdown
    #[arg(long, env = "TOS_FORMAT", global = true, default_value = "table")]
    pub format: String,

    /// 以 JSON 格式输出帮助信息（机器可读）
    #[arg(long, global = true, default_value_t = false)]
    pub help_json: bool,

    /// 预览模式（不实际执行，dry-run）
    #[arg(long, global = true, default_value_t = false)]
    pub dry_run: bool,

    /// 跳过确认提示，自动确认破坏性操作
    #[arg(long, short = 'y', global = true, default_value_t = false)]
    pub yes: bool,

    /// 禁用颜色输出
    #[arg(long, env = "NO_COLOR", global = true, default_value_t = false)]
    pub no_color: bool,

    /// TOS 服务端点 URL（覆盖配置文件和自动检测）
    #[arg(long, env = "TOS_ENDPOINT", global = true)]
    pub endpoint: Option<String>,

    /// 区域标识（如 cn-beijing）
    #[arg(long, env = "TOS_REGION", global = true)]
    pub region: Option<String>,

    /// 配置文件 profile 名称
    #[arg(long, env = "TOS_PROFILE", global = true, default_value = "default")]
    pub profile: String,

    /// 配置文件路径，默认 $HOME/.tos/config.toml
    #[arg(long, env = "TOS_CONFIG_PATH", value_name = "PATH", global = true)]
    pub config_path: Option<PathBuf>,

    /// Access Key ID
    #[arg(long, env = "TOS_ACCESS_KEY", global = true)]
    pub access_key: Option<String>,

    /// Secret Access Key
    #[arg(long, env = "TOS_SECRET_KEY", global = true)]
    pub secret_key: Option<String>,

    /// 临时凭证 Security Token（STS）
    #[arg(long, env = "TOS_SECURITY_TOKEN", global = true)]
    pub security_token: Option<String>,

    /// 详细日志模式
    #[arg(long, short = 'v', global = true, default_value_t = false)]
    pub verbose: bool,

    /// 静默模式（仅输出错误信息）
    #[arg(long, short = 'q', global = true, default_value_t = false)]
    pub quiet: bool,

    /// 每页返回条数
    #[arg(long, global = true)]
    pub page_size: Option<u32>,

    /// 最大返回总条数
    #[arg(long, global = true)]
    pub max_items: Option<u64>,

    /// 分页标记（续传令牌）
    #[arg(long, global = true)]
    pub page_token: Option<String>,

    /// 输出文件路径（默认输出到标准输出）
    #[arg(long, short = 'o', global = true)]
    pub output: Option<String>,
}
