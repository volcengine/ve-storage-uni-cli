# tos ZTI 鉴权：依赖验证与分步提交计划

> 公开发行方案更新（2026-09-23）：内部 SDK 入口已由公开源码实现取代。
> 以下内部依赖与 Commit 4 设计仅保留为历史调查记录；当前实现及后续计划以
> `2026-09-23-open-zti.md` 为准。

> 执行约定：每完成一个 commit，完成验证及 Review 后停止，等待用户 review。
> 本约定优先于技能中的连续执行默认流程。后续实现使用 executing-plans 或
> subagent-driven-development，并遵守用户的代码审查要求。

**Goal:** 为 `tos` 接通 `--auth-mode aksk|zti`，同步配置、help、describe、skill、
MCP、诊断和发行能力；`ve-tos`、`ve-adrive` 不接受 ZTI。

**Architecture:** 保留现有 ByteTOS 请求协议和传输引擎，增加独立的 Token 请求
鉴权分支。官方 ZTI SDK 管理身份发现、缓存和刷新；公共依赖图只包含 Provider
接口，私有 SDK 由独立内部入口注入。

**Tech Stack:** Rust、Tokio、reqwest、clap、内部 `zti = 0.1.9`。

## 1. 第一笔提交的范围

本提交只记录依赖验证、设计边界和后续提交顺序，不修改生产代码或 Cargo 依赖。
不声称 ZTI 已可用，也不声称完成跨平台或服务端联调。

### 已验证的事实

- `tos-rust-sdk/src/zti.rs` 通过 `Zti<BytedanceComponent>::new_from_env()`、
  `enable_capability([Capability::JwtSvid])`、`get_jwt_svid()` 获取当前 Token。
  该适配层使用进程内 OnceCell，首次请求时初始化。
- `tos-rust-sdk/src/tos.rs::do_request_once` 在 ZTI 模式添加
  `x-tos-ztitoken-with-acp`，跳过请求签名；复制请求也跳过源签名。
  不存在 ZTI 换 AK/SK 的过程。原 SDK 的 ACP + AK/SK 模式不属于本次范围。
- 本地 `zti 0.1.9` 的来源选择为环境字符串、Agent、文件；每次请求读取 SDK
  当前缓存，Agent 和文件后台更新。显式字符串不会自动刷新。
  此顺序是来源选择，不是错误重试链：选中 Agent 后初始化/获取失败，不自动
  回退文件；来源诊断和错误测试必须保持这一行为。
- `packaging/scripts/release.py::CARGO_PUBLISH_STEPS` 仍公开发布 `tos-core`、
  `tos-cli-core`、统一入口及三个安装入口；不是只发布内部二进制。
- 本机 Cargo 附带的 `reference/registries.html` 明确说明 crates.io 不接受
  依赖其他 registry 的包。因此不能把私有 `zti`（即使 optional）直接添加到
  这些公共包的 manifest；feature 开关不等于包发布边界。
- `zti` 还依赖内部 `byted-spiffe`、`spiffe-id`、`zti-region`。单独复制
  TOS SDK 的 Provider 文件不能消除私有依赖。

### 编译探针结果

在仓库外的 `/private/tmp/tos-zti-probe` 创建独立、`publish = false` 的临时包，
以 `zti = 0.1.9`、`default-features = false` 和以下 feature 编译适配 API：

- `upstream-embedded`
- `upstream-file`
- `file-notify`
- `upstream-spiffe-workload-api`

探针仅定义调用上述三个 API 的异步函数，以 `cargo check` 编译而不运行。
这是一组已验证的精简 feature，并非对最小集合的证明。未启用 metrics、inspector、
downgrade、online-jwk；本次只获取 JWT SVID，不做客户端 JWT 验签或身份降级。

验证命令：

```sh
mkdir -p /private/tmp/tos-zti-probe/src
cat > /private/tmp/tos-zti-probe/Cargo.toml <<'EOF'
[package]
name = "tos-zti-dependency-probe"
version = "0.0.0"
edition = "2021"
publish = false

[workspace]

[dependencies]
zti = { version = "=0.1.9", registry = "crates-byted", default-features = false, features = ["upstream-embedded", "upstream-file", "file-notify", "upstream-spiffe-workload-api"] }
EOF
cat > /private/tmp/tos-zti-probe/src/lib.rs <<'EOF'
use zti::{spiffe_id::BytedanceComponent, Capability, Zti};

/// Compile-check the adapter API without running credential discovery.
/// Returns the current token when called; SDK errors propagate unchanged.
pub async fn compile_adapter_api() -> Result<String, zti::Error> {
    let identity = Zti::<BytedanceComponent>::new_from_env()?;
    identity.enable_capability([Capability::JwtSvid]).await?;
    Ok(identity.get_jwt_svid()?.token().to_owned())
}
EOF
cargo check --manifest-path /private/tmp/tos-zti-probe/Cargo.toml --offline \
  --config 'registries.crates-byted.index="sparse+https://rust.byted.org/repository/rust-byted/index/"'
cargo metadata --offline --no-deps --format-version 1
python3 packaging/scripts/release.py cargo --dry-run
```

结果：探针在 `aarch64-apple-darwin`、Cargo 1.96.0 上编译成功；workspace 元数据
读取成功；发行脚本成功输出 8 个发布步骤。最后一条命令未传 `--execute`，只展示
命令，不代表 `cargo publish --dry-run` 已执行，更未发布任何包。

探针文件是临时验证产物，不是正式项目依赖。没有读取用户 Token、连接 Agent 或
请求存储服务，也没有修改用户 Cargo 配置。

### 明确的风险和验证限制

1. `zti 0.1.9/src/upstream/jwt_svid.rs:46,104,167` 在 TRACE 输出原始 Token。
   当前 CLI 的 tracing 初始化接受环境过滤器。接入前必须消除此日志泄露路径，
   不能仅依靠默认日志级别；须验证 `RUST_LOG=trace` 和定向开启 ZTI trace。
2. `byted-spiffe 0.2.0/src/workload_api/client.rs:7` 无条件引用
   `tokio::net::UnixStream`。Windows 全功能 Agent 支持不能假定成立。
   Windows 和 Linux 的完整依赖编译尚未验证；macOS 编译通过不等于 Agent 可用。
   `byted-spiffe` 的 build.rs 会编译 protobuf，内部构建需要 protoc。它对
   workload 相关 optional 依赖的使用并非完全受 feature 控制，不能假定仅开启
   文件来源就能避开 Unix 依赖或获得可编译的 Windows 版本。
3. `zti` 的 inspector 在 Linux 才启用实现，不能将 memfd 本身误判为本次
   macOS 阻塞；精简探针已不启用 inspector。
4. 本次未运行整个 CLI 测试套件，原因是只修改计划文档。后续代码提交必须按各自
   范围跑测试，最终再完成跨入口回归和发布检查。

## 2. 待本次 Review 确认的构建边界

推荐新增独立内部入口 `internal/tos-zti-cli/`，拥有自己的 `[workspace]`、
lockfile 和 `publish = false`。它依赖公共统一入口以及内部 ZTI Provider，产出
内部发行的 `tos-cli`。私有 registry 配置仅属于该内部 workspace。

公共 `tos-core` 提供不依赖私有类型的异步 Token Provider 接口；统一入口提供
显式注入路径，Provider 随调用上下文传递至 ByteTOS 客户端及 MCP 调用。不得以
可变进程全局状态切换身份，也不得为方便接入把 `ve-tos` 的公开模式扩成 ZTI。

公开构建保留 AK/SK；选择 ZTI 时如没有注入 Provider，明确报能力未编入，不回退。
help/describe/capabilities 必须区分协议支持与当前构建的可用性。内部构建提供
环境字符串、Agent、文件全部来源；公共构建不另写一套文件刷新协议。

此边界会影响 ZTI 可用的安装渠道，须在本次 Review 中确认后再实施。Windows
不预先承诺 Agent 支持；若内部依赖不支持该平台，发行脚本应拒绝该 ZTI 构建，
而不是生成不完整功能包。原公共 Windows 发行不受影响。

不采用另外两种方案的原因：引入整个 TOS SDK 会扩大传输及协议迁移范围；在公共
manifest 中添加 optional 私有依赖不能维持现有 crates.io 发布契约。

## 3. 稳定的用户契约

| 入口 | 允许模式 | 默认 |
| --- | --- | --- |
| tos | aksk、zti | aksk |
| ve-tos | aksk、unified | aksk |
| ve-adrive | aksk、oauth、unified | aksk |

- `tos` 优先级：`--auth-mode` > `[profile.tos].auth_mode` >
  `BYTETOS_AUTH_MODE` > `aksk`。`TOS_AUTH_MODE` 继续只控制 `ve-tos`。
- ZTI 不读取、解密或要求 AK/SK，也不回退其他模式；有效共享配置中的 tos ZTI
  设置不得使 ve-tos/ve-adrive 加载失败。
- 不新增明文 Token 命令行参数、Token 持久化或 login/logout 命令。
- 每次 HTTP 尝试取当前 Token，跳过请求签名和复制源签名，保留 ByteTOS 请求格式。
- ZTI `presign` 明确不支持，不把 Token 放进 URL；元数据仍可离线说明该限制。
- help、describe、skill 导出、补全、dry-run 不获取 Token；doctor 默认离线，
  区分已配置、编入能力与经过网络验证，不将“存在来源”表述为远端鉴权成功。
- MCP 工具参数可覆盖 serve 启动时的模式，未指定则继承；Token 不进入工具参数。
  存储服务鉴权不改变 MCP SSE 的客户端 Bearer 鉴权。

## 4. Commit 顺序及每步验收

每笔实现提交先添加能复现缺失行为的测试，再实现、验证和完整 Review。
如发现 Critical/Major，修复并按用户要求标注理由，重新 Review 后才能提交。
用户 review 通过前不执行下一笔提交。

### Commit 1 — docs: record tos ZTI dependency findings and commit plan

- [x] 检查 SDK、私有依赖及公共发行链路。
- [x] 在仓库外完成精简依赖的 macOS 离线编译。
- [x] 记录协议、来源、日志风险、平台限制和安装渠道影响。
- [x] 完成文档 Review 和 diff 检查；本步提交后等待用户 review。

### Commit 2 — feat(auth): add injectable ZTI request authentication

文件：`crates/tos-core/src/infra/zti_credentials.rs`（新增）、`infra/mod.rs`、
`infra/client.rs` 及其中的测试。公共依赖不含私有 registry。

- [x] 定义可注入、可 mock 的异步 Provider，返回当前 Token，提供脱敏错误映射。
- [x] 扩展 RequestAuth 及客户端构造，覆盖缓冲和流式请求、每次重试、复制头。
- [x] 增加 Token 空值/换行拒绝、敏感 HeaderValue、ZTI presign 拒绝测试。
- [x] 验证正确 Token 头以及所有 AK/SK 签名头缺失；用计数 Provider 验证重试刷新。
- [x] 运行 `cargo test -p tos-core --lib` 并完成完整 Review；本步提交后停止。

该提交只提供底层能力，不提前在用户 help 中声称 ZTI 可用。

### Commit 3 — feat(tos): enable scoped auth-mode configuration and runtime

文件：`crates/toscli/src/cli/auth.rs`（新增）、`cli/mod.rs`、
`crates/tos-core/src/infra/config.rs`、`agent/global_args.rs`、
`crates/tos/src/handler/common.rs`、`src/lib.rs`、相关配置及入口测试。

- [x] 接通 tos 独立枚举、参数、优先级和运行时 Provider 注入，保留 ve 模式枚举。
- [x] 去掉 `.tos.auth_mode` 的全局禁止，改为按命令面校验；更新配置写入和来源展示。
- [x] 同步基础 help、模式 schema 和能力未编入错误，确保提交后不会静默忽略 ZTI。
- [x] 覆盖无 AK/SK、有损坏本地 AK/SK、缺失命名 profile 及混合配置的 ZTI 行为。
- [x] 运行配置/入口测试及 `cargo test -p ve-tos-cli-core --lib`、
  `cargo test -p tos-cli-core --lib`；确认 ve-tos/ve-adrive 拒绝 zti 后 Review、提交。

### Commit 4 — feat(tos): add internal ZTI SDK adapter and build entry

文件：`internal/tos-zti-cli/Cargo.toml`、`src/main.rs`、`src/provider.rs`、
内部 workspace registry 配置及 lockfile；统一入口注入 API。

- [x] 使用第一步验证的 SDK API 和精简 feature；共享惰性初始化、限时错误处理。
- [x] 消除依赖的 Token trace 泄露，测试强制 trace 情况；仅过滤默认日志不算通过。
- [x] 用 mock Provider 验证入口注入到普通调用及 MCP；真实 SDK 测试不读取用户凭证。
- [x] 编译 macOS/Linux 内部入口，核对 Windows 明确限制；用含依赖的完整
  `cargo metadata --format-version 1`、`cargo tree` 和公共打包 manifest 检查私有
  包不可达。`--no-deps` 不能证明依赖闭包隔离；另以无内部 registry 配置的环境验证。
- [x] 完整 Review 后提交，报告具体构建结果及未验证的 Agent 环境，停止。

### Commit 5 — feat(tos): align ZTI discovery, skills, diagnostics and MCP

文件：`crates/toscli/src/registry.rs`、`handler/meta.rs`、
`crates/tos-core/src/agent/skill_markdown.rs`、`skill_guide.rs`、tos skill 指南、
`src/lib.rs` 中英文 help、README 及 CLI/skill 测试。

- [x] 一致展示枚举、默认、优先级、当前构建可用性、来源和 presign 限制。
- [x] 对共享指南按命令面分流，不将 tos ZTI 文案泄漏到 ve-tos/ve-adrive。
- [x] 更新 MCP schema、保留参数、argv 和继承规则；补全支持 aksk/zti。
- [x] 验证元命令及 dry-run 不调用 Provider；doctor 不强制要求 AK/SK。
- [x] 运行 core_help、skill_export、cli_basic、配置测试及 tos-cli-core 测试；
  对中英文 help、describe、capabilities、导出 SKILL.md 做一致性检查，Review、提交。

此提交使用公开仓库内建 ZTI Provider；实现方案见
`2026-09-23-open-zti.md`。未在真实 Agent/ByteTOS 服务环境验证远端授权。

### Commit 6 — build(tos): package public ZTI-enabled artifacts

原内部包方案已由公开实现取代。下一笔应审查公共发布包的依赖闭包、平台构建与
安装文档，并补充跨入口回归测试；不得恢复私有 Cargo 依赖。

- [x] 验证公共 Cargo/npm/pip/Homebrew 安装产物包含内建 ZTI，并明确平台支持；
  WinGet 复核清单模板，Windows 二进制留待 Windows runner 实编。
- [x] 验证依赖闭包不含私有包，且发布计划不改变 ve-tos/ve-adrive 鉴权行为。
- [x] 运行串行 `cargo test --workspace`、`cargo fmt --all --check`、packaging
  测试、三个 macOS 独立入口 release 构建及 Linux `tos-cli` 交叉编译检查。
- [ ] 真实 ZTI 读、写、分片、复制联调仍需授权 Agent/ByteTOS 环境；本笔仅验证
  本地协议、依赖和产物，不把 mock 通过当作远端服务验收通过。
- [x] 完整 Review、本地提交并停止；任何发布/推送不由本计划中的本地 commit 自动触发。

## 5. 全量验收矩阵

至少覆盖：正常读写；无 AK/SK；两种凭证共存但只使用所选模式；配置/环境/CLI
优先级；Token 空值及换行；Provider 初始化失败/超时；Token 轮换与重试；并发
初始化；流式/分片/复制；presign 拒绝；MCP 覆盖与继承；离线元命令；TRACE 脱敏；
另外两个入口的负向模式测试；旧 AK/SK、Unified、OAuth 回归；公共/内部发行隔离。

## 6. 第一笔提交 Review 记录

Review 覆盖正确性、安全性、性能、可维护性、健壮性、可测试性和可观测性。
本提交无生产代码修改，性能/资源释放仅审查设计，不代表已完成运行时验证。
第一轮独立 Review：Critical 0、Major 0、Minor 2。两项 Minor 均已修正：补充
Agent 失败不回退文件的语义，并要求完整依赖图和打包 manifest 验证发行隔离。
探针复现配方及 protoc 前提已补全。后续代码提交独立记录发现、修复和剩余项。
第二轮完整 Review：旧问题均修复，新增 0、回归 0；剩余 Critical/Major/Minor
均为 0。以上统计只针对本笔文档，不代表计划中的代码已实现或通过测试。

## 7. 第二笔提交验证记录

实现位置：`infra/zti_credentials.rs` 封装异步 resolver 及固定错误类别；
`infra/client.rs` 增加明确选择 ByteTOS 协议的 ZTI 构造入口和共享请求鉴权步骤；
`infra/client_zti_tests.rs` 使用本地 HTTP 服务验证请求行为。未修改 CLI、模式
配置或 Cargo 依赖，也未接入真实 ZTI SDK。

行为补充：ZTI 请求剥离调用方附加的旧鉴权头，Provider 是唯一身份来源；禁止
自动重定向以免自定义 Token 头被转发。Provider 返回的 HeaderValue 始终标记
sensitive；错误仅使用固定类别，不传播原始 SDK 错误。超时和 SDK 缓存/刷新由
未来注入的适配器负责，本次不创建后台任务。

测试先行证据：Provider 测试从 5 失败变为 6 通过；客户端先以既有 AK/SK 构造
作为占位行为，7 个新测试因凭证要求或错误请求头失败，再实现 ZTI 分支。
中途发现分片测试错误地预期斜杠被编码，核对既有 ByteTOS 查询实现后修正为
保留斜杠，没有改变原协议。随后补充错误、敏感性和追踪测试。

自审发现并修复 1 项 Major：重试中 Token 获取失败仍保留旧响应 ID。先以回归
测试确认失败，再通过 `Review Fix #1` 清除 terminal response 状态，保留历史
request IDs。修复作用于缓冲和流式共用的鉴权步骤。

已执行的验证：

- `cargo test -p tos-core --lib --offline -- --test-threads=1`：181 通过，0 失败。
  其中新增 Provider 测试 7 个、客户端测试 14 个。
- `cargo check --workspace --offline`：全部 workspace 包编译检查通过。
- 修改的 Rust 文件通过 rustfmt 检查；`git diff --check` 无空白错误。
- HTTP 测试使用合成 Token 和本机回环服务；未访问真实 Agent、账户或存储服务。

已覆盖正常路径、无 AK/SK/凭证共存、缓冲与流式重试、复制、非法 Token、Provider
失败、401 不重试、presign/form 拒绝、header/Debug 脱敏、重定向隔离及响应追踪。
规格审查通过。独立代码质量审查共三轮：

| 轮次 | 新发现 | 修复 | 回归/剩余 |
| --- | --- | --- | --- |
| 1 | Major 1：reqwest 从 URL userinfo 自动附加 Basic Authorization | Fix #2 在实际 Request 中清理隐式 Authorization | 进入第二轮复审 |
| 2 | Major 1：提前构建错误绕过旧响应 ID 清理 | Fix #3 对早期 builder 错误记录无响应状态 | 进入第三轮复审 |
| 3 | 0 | 确认前两轮问题均修复 | 新增 0；Critical/Major/Minor 均为 0 |

两项独立 Review 修复均先加入失败回归再修复，保留 `Review Fix` 原因标注。
另采纳了两项非阻塞测试建议：响应体读取失败后重新获取 Token，以及多个 Provider
clone 并发调用互不覆盖。没有未修复的 Minor/Suggestion。既有低级
`send_form_post` 传输接口未改变；本步 form 限制指 `prepare_form_auth` 签名拒绝。

本步可供复核的测试至少包含：无 AK/SK 正常请求、两类请求重试、复制源保留、非法
Token 阻断、Provider 失败、URL 隐式 Basic 清理、重定向隔离、响应 ID 清理。
真实 SDK 获取、CLI 模式开放和真实服务端联调属于后续 commit，未在本步完成。


## 8. 第三笔提交验证记录

`tos` 增加独立 `ByteTosAuthMode` 与 `--auth-mode aksk|zti`，保留 ve 的模式类型。
运行时按 CLI、选中 profile 的 tos 配置、BYTETOS_AUTH_MODE、aksk 解析；模式及
Provider 随调用上下文传递。ZTI 不读取环境 AK/SK、credentials 文件或解密旧配置，
支持以非敏感网络参数建立缺失的命名 profile；实际构造客户端时才检查 Provider。
公开构建选择 ZTI 会报 `[ZtiProviderUnavailable]`，不会回退 AK/SK。

配置写入按命令面校验，show 展示模式和来源。基础中英文 help 与高层命令 describe
包含模式、默认和优先级。MCP 子进程保留 serve 的显式模式及配置/凭证路径，以免
重建调用时丢失鉴权选择；完整工具参数覆盖、skill、doctor 在第五笔提交统一处理。
沿用 ve 的配置校验规则：即使 CLI 覆盖，选中 profile 中的非法配置值仍会被拒绝。

测试先行：根入口参数测试先因不接受 tos 模式失败；runtime 测试先复现损坏凭证
被读取；describe schema 和 MCP 继承测试先失败，再完成接线。所有 HTTP 验证使用
mock Token 与回环服务，没有读取真实身份或访问存储服务。

验证结果：

- 根库 13、config 集成 108、credentials 集成 20、ZTI 入口集成 5，全部通过。
- tos-core 库 184、tos-cli-core 库 8，全部通过。
- ve-tos-cli-core 库 314 通过、2 失败，失败基线核对结果见下文。
- 新增/补充覆盖：参数作用域、CLI/config/env 优先级及来源、损坏凭证绕过、
  无 Provider 报错、命名 profile、mock 请求鉴权头、dry-run/describe 离线、MCP 继承。
- 配置/运行时与入口/元数据分别完成完整独立 Review，七类检查均通过：
  Critical 0、Major 0、Minor 0；没有新增待修复问题。

当前阶段限制：尚未接入真实 SDK；doctor 仍沿用旧 AK/SK 诊断，完整能力发现与
skill 文案在第五笔提交完成。本笔不代表 ZTI 内部发行包已可用。

基线复现：以 `git archive c659661` 在独立临时目录和独立 target 中，分别执行
以下两个定向库测试，均以相同原因退出 101；不是本笔回归：

- `handler::meta::tests::chinese_catalog_recursively_covers_all_describe_metadata`：
  已有 `Skill name, canonical command, or command suffix, e.g. ve_tos_cp or cp`
  描述缺少中文翻译。
- `registry::tests::unified_auth_metadata_is_synchronized_for_ve_tos_only`：
  已有 tos skill 文案 `For a unified CLI session` 触发禁止 `unified` 的断言。

这两项既有测试问题保留，在第五笔元数据/skill 工作中处理。`cargo check
--workspace --offline`、`cargo fmt --all -- --check` 和 `git diff --check` 均通过。
编译检查曾命中旧 tos-core 增量元数据；刷新源文件时间戳后重新编译通过，未为此
修改生产逻辑。以上测试统计来自实际执行，不将全库结果表述为全部通过。


## 9. 第四笔提交验证记录

新增独立 `internal/tos-zti-cli/` workspace，`publish = false`，包含自己的 registry
配置和 lockfile，产出内部 `tos-cli`。公开入口新增
`run_byted_tos_cli_with_zti_provider`，仅传递公共 Provider 类型；公共 manifest 和
依赖锁文件没有引入私有 SDK。SDK 仍为验证过的 `zti = 0.1.9` 精简 feature 集合。

Provider 使用 invocation 独立的 `Arc<OnceCell>`，首次真实请求才初始化 SDK。
每次解析从 SDK 当前缓存读取 JWT SVID，不另做 Token 缓存或 AK/SK 交换；失败映射
为固定 Unavailable/Expired/Timeout 类别，过期边界为 `now >= expires_at`。
十秒超时限制调用等待。SDK 的 LiveData 在安装 abort handle 前就生成后台任务，
因此采用独立的一线程 Tokio runtime 归属这些任务；取消初始化、初始化失败和
最后一个 Provider 释放时关闭运行时。已经开始的阻塞 SDK/OS 操作无法强制取消，
不将调用超时宣称为所有线程/I/O 已同步结束。

本次依赖审计补充并处理了两条安全路径：

1. `LiveData` 初始化中 `oneshot.send(...).unwrap()` 在取消竞态下可能 panic，
   而 `JwtSvid` 的 Debug 含 Token。内部 main 在创建 runtime 前永久替换 panic
   hook，仅输出固定脱敏消息，不链式调用旧 hook，不格式化 payload（Fix #1）。
2. tracing 在关闭自己的事件后仍可能走 log bridge。内部 manifest 同时启用
   tracing 与 log 的 `max_level_off`、`release_max_level_off`（Fix #2）。
   这会关闭内部二进制的全部 tracing/log 诊断，不仅是 SDK 日志；正常 CLI
   Envelope、退出码与 request ID 保留。公共构建日志行为不变。

注入入口由调用方负责日志策略，不再创建默认 EnvFilter；公开普通入口仍初始化
原有 tracing。新增测试先复现 `RUST_LOG=trace` 的过滤器警告，再通过 Fix #3
避免向用户输出“移除安全过滤 feature”的误导建议。

实际 SDK 测试补充了两项运行约束：

- SDK 自己需要识别部署区域；隔离测试明确使用 `ZTI_ENV=local`，真实部署应使用
  对应环境值。ByteTOS `--region` 不代替 SDK 区域发现。合成 SPIFFE subject 也
  必须包含合法 Bytedance identity component，避免用区域/fixture 错误冒充过期测试。
- SDK 0.1.9 notifier 只接受 CloseWrite 或 RenameTo，而 macOS FSEvents 返回
  RenameAny。文件来源首次 GET 已实测成功；macOS 文件快速轮换回归明确 ignored，
  文件更新需等 SDK 默认 600 秒轮询。保留 Linux 快速轮换测试但未在 Linux 执行。
  本笔未改写 SDK 的刷新协议，未将该项列为通过。

测试先行证据：Provider 的六个初始测试先失败后通过，再补两项生命周期回归；
入口 mock Provider 绑定测试先因缺少接线失败；panic hook 与日志警告测试分别
先失败再实现。真实 SDK 子进程统一清空环境、临时 HOME、合成 JWT、本机回环 HTTP，
没有读取真实 Token、访问真实存储服务或连接真实 Agent。全套测试曾发现 macOS
mock socket 继承 nonblocking 的偶发错误，测试服务显式设为 blocking 并保留
读写 deadline 后完成全套复核，不修改生产请求逻辑。

验证结果：

- `cd internal/tos-zti-cli && cargo test --locked --offline`：Provider 8、入口集成 6、
  日志安全 4，合计 **18 通过、1 ignored、0 失败**。包括普通请求、MCP 子进程、
  文件来源、环境来源优先级、Agent 失败不回退、过期/非法 Token、离线命令、
  TRACE 两种过滤配置及前台/后台 panic 脱敏。
- 根入口库与 `tos_auth_mode_test`：**14 + 5 通过**。
- macOS aarch64 debug 可执行文件实际编译并由集成测试执行。
- `CARGO_TARGET_DIR=/private/tmp/tos-zti-linux-target cargo zigbuild --offline --locked
  --target x86_64-unknown-linux-gnu`：实际编译并链接成功，`file` 确认为 Linux ELF。
  未在 Linux 执行，未验证真实 Agent 或远端鉴权。
- Windows 编译拒绝 guard 用安装的 `x86_64-pc-windows-msvc` target 对私有 lib
  直接执行 rustc 验证，按预期输出仅支持 Linux/macOS 的 compile_error；不是完整
  Windows 依赖构建，也不声称内部 Agent 支持 Windows。
- 公共完整 `cargo metadata --format-version 1`：321 个包，私有包 0；全平台
  `cargo tree --target all` 无私有包。以不含 registry 配置的临时 CARGO_HOME 和
  共享 registry 缓存重复完整 metadata/tree，同样通过。
- 公共 `cargo package -p ve-storage-uni-cli --list --allow-dirty --offline` 仅列出
  根包允许的文件，不包含 internal。未执行 publish。

独立完整 Review 共三轮：首轮无 Critical/Major、2 项 Minor（阻塞 I/O 清理边界和
panic hook 调用前提）均通过文档修正；后两轮复核无新增问题，最终
Critical/Major/Minor 均为 0。明确保留的限制是 macOS 文件快速轮换、真实 Agent
及远端联调；skill/doctor/完整能力发现仍按第五笔提交执行。

最终补充：内部 `cargo test --release --test log_safety --locked --offline` **4/4
通过**；公共 workspace `cargo check --locked --offline`、两套 workspace 的
rustfmt 检查及 `git diff --check` 通过。

## 10. 第六笔提交验证记录

三个独立入口的公开 Cargo.lock 已纳入版本控制，归档构建使用 `--locked`；
offline metadata 与 package-list 检查确认 ZTI 源文件、tonic/prost/tower、
crates.io 依赖来源及锁文件均进入公共包。三个入口的直接调用测试分别确认
`tos-cli` 包含 `aksk/zti`，`ve-tos-cli` 和 `ve-adrive-cli` 拒绝 `zti`。
平台 Review 修复了 Windows 将 Unix Agent 误报为可用来源、且存在 Agent 路径
时可能挡住文件 Token 的问题；来源发现、describe 参数及发布 guard 均按平台对齐。

本机 aarch64 macOS 三个独立入口的 release 二进制均已实际构建；`tos-cli`
原生 `--describe` 发布 guard 通过。归档、npm 包目录、pip 包目录、Homebrew
formula 已在本地生成，tar 包每个二进制仅有一份，pip 的两个 TOS 包目录与归档
使用同一个 `tos-cli`。本机 Python 缺少 build/setuptools，未生成 wheel。
Linux `tos-cli` 通过 `cargo zigbuild` 的 x86_64 交叉编译检查。Windows
交叉检查受本机 cargo-xwin 缓存写权限及缺失离线 MSVC sysroot 限制，没有把
模板检查当作 Windows 构建。
未执行公开发布，也未连接真实 Agent 或 ByteTOS 服务。

`python3 -m pytest packaging/tests -q` 共 77 项通过；三个 standalone Cargo
测试、根仓库 rustfmt 与三个 standalone rustfmt 检查通过。工作区默认并行测试
在现有 ve-tos-core 的三项环境/Envelope 测试上失败，相关运行时文件未在本笔
修改；单测独立执行和 `cargo test --workspace --offline --quiet --
--test-threads=1` 全量串行运行通过。并行测试稳定性留待后续单独修复。

两轮 Coder/Reviewer 复核覆盖正确性、安全性、性能、可维护性、健壮性、
可测试性及可观测性。首轮发现并修复 tar 重复成员、ADrive skill 文案漂移、
私有路径依赖测试缺口及 Windows Agent 误报/文件来源阻断；第二轮无新的
Critical/Major。保留并行测试稳定性、Windows 实编、pip wheel 及真实服务
联调为明确未验收项。
