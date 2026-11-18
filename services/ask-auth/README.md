# ask-auth 运维手册

`services/ask-auth` 是唯一认证服务包和 `ask-auth` 二进制。核心 OAuth、MySQL 存储及 TCE/TCC 宿主统一维护，只有仓库根 `Cargo.lock`，依赖全部来自公开 registry 或本仓库路径。原 `ask-auth-tce` 已合并，当前实施记录见 [tcc-merge-plan.md](tcc-merge-plan.md)。核心 v1.15 协议、接口和数据库约束见 [architecture.md](architecture.md)，冻结契约及 DDL 没有更改。

CLI 直接请求豆包业务接口；本服务只处理 OAuth、可信身份、准入及令牌生命周期，保护 OAuth AppSecret。BOE、M0 和生产实际验收、Linux 发布物验收以及双副本发布仍待部署环境验证。

## 根工作区构建与本地运行

所有 Cargo 命令在仓库根执行。缓存公开依赖后可增加 `--offline`。

```sh
cargo build -p ask-auth --locked
cargo test -p ask-auth --locked
cargo clippy -p ask-auth --locked --all-targets --no-deps -- -D warnings
cargo fmt --all -- --check
cargo run -p ask-auth --locked -- --help
cargo run -p ask-auth --locked
```

**无参数只启动本地禁用入口，部署必须显式使用 `serve`。** 本地入口仅接受 `ASK_AUTH_LISTEN`（默认数字地址 `127.0.0.1:8080`）和 `ASK_AUTH_LOG_LEVEL`（默认 `info`，支持 off/error/warn/info/debug/trace），其他 `ASK_AUTH_` 变量一律拒绝，包含 AppSecret。信号后整个连接排空最多五秒，包含未完成请求头的连接。此入口不读取 TCC、不连接数据库、不启用 OAuth。

同一个二进制提供以下命令，`--help` 不要求工作负载身份、AppID 或凭据：

```sh
cargo run -p ask-auth --locked -- probe --environment boe --namespace storage.tos.cli --space default
cargo run -p ask-auth --locked -- migrate --config /run/ask-auth/operator.json
cargo run -p ask-auth --locked -- serve --environment boe --listen 0.0.0.0:8080
```

环境必须为 `boe` 或 `prod`，namespace 默认 `storage.tos.cli`，space 默认 `default`，监听默认 `127.0.0.1:8080`。TLB 容器必须显式配置后端监听。未知/重复参数和非 UTF-8 参数返回固定错误；argv/env 不提供 OAuth AppSecret 或数据库 URL 回退。

`probe` 仅新鲜读取并验证配置，不连接数据库或请求 OAuth。身份/传输/超时等短暂错误以一秒间隔重读，总上限三十秒；无效配置立即失败。OAuth POST 和轮换 refresh token 不自动重试。本地开发和普通测试不需要真实 AppID、AppSecret、TCC、Consul 或 OAuth 服务。

## Linux SCM 发布物

在批准的 Linux SCM 构建环境，准备 Rust >=1.94、公开 crate 缓存及编译/TLS 依赖，从仓库根执行：

```sh
services/ask-auth/build.sh
services/ask-auth/output/run.sh --help
services/ask-auth/output/run.sh serve --environment boe --listen 0.0.0.0:8080
```

脚本自行解析仓库根目录，执行 `cargo build -p ask-auth --release --locked --bin ask-auth --target <Linux Rust host>`，输出 `services/ask-auth/output/bin/ask-auth` 和 `output/run.sh`；包装脚本以 `exec` 启动。非 Linux 系统或非 Linux Rust host 在构建前拒绝。macOS 本机构建不能作为 Linux 发布验收。

根 `.cargo/config.toml` 的 `--cfg polyval_force_soft` 是密钥状态擦除约束。发布脚本即使收到 `RUSTFLAGS` 或 `CARGO_ENCODED_RUSTFLAGS` 覆盖也追加该标志；手工 Cargo 覆盖同样必须保留它。无需第二套 Cargo 配置或锁文件。

## 平台身份、DPS 与发现输入

| 输入 | 优先级及约束 |
| --- | --- |
| 工作负载 PSM | `LOAD_SERVICE_PSM` > `PSM` > `TCE_PSM`；必需，安全的点分段标识 |
| 目标 IDC | 显式 `RUNTIME_IDC_NAME`；必需，最长 64 字节安全标识，无 shell/网卡探测回退 |
| 服务环境头 | `SERVICE_ENV` > `TCE_ENV` > `prod` |
| 发送方 cluster | `SERVICE_CLUSTER` > `TCE_CLUSTER` > `default` |
| DPS | 存在 `SEC_TOKEN_STRING` 时只使用它，否则绝对路径 `SEC_TOKEN_PATH` |
| Consul 地址 | 共享 tos-core 规则：`CONSUL_HTTP_ADDR` > `CONSUL_HTTP_HOST` > 平台主机变量；默认 `127.0.0.1:2280`，端口支持 `CONSUL_HTTP_PORT` |
| 可选平台元信息 | `TCE_STAGE`、`MY_POD_NAME`、`MY_HOST_IP`、`MY_HOST_IPV6`，IP 必须为数字地址，IPv6 优先 |

工作负载身份与 DPS 输入的高优先级值存在但无效时直接失败，不回退；Consul 地址继续使用既有 tos-core 解析优先级和规范化回退规则。BOE IDC 仅 `boe`、`boe2`、`boei18n`；生产要求非 BOE IDC，且存在 `IS_PROD_RUNTIME` 或 `SERVICE_ENV` 分类依据。命令期望环境必须匹配；`TCE_HOST_ENV=ppe` 和 `TESTING_PREFIX=offline` 拒绝。元信息头限 256 字节可见 ASCII；namespace 为 1–256 字节安全点分段标识，space 为 1–64 字节单段。

**平台 DPS 是访问 TCC 的工作负载凭据；OAuth client_secret 是豆包应用凭据，两者用途独立。** 不把 DPS 当作 OAuth AppSecret，也不把解码 JWT 声明当作可信身份。配置 TCC 工作负载 PSM ACL，并限制 secret key 给工作负载和授权操作员。禁用 CDN/公开分发。

DPS 必须是非空可见 ASCII，最多 16 KiB。静态无效值不会回退文件。文件使用绝对路径，每次请求前及响应体完成后重新读取、核对内容和设备/inode/大小/时间信息；轮换、消失或无效会丢弃当前响应。平台符号链接支持轮换，不额外要求平台文件 0400/0600。异步文件读取有 250 ms 期限、`O_NONBLOCK` 和普通文件校验，固定擦除缓冲区检测超限。文件凭据仅支持 Unix，其他平台编译可用但请求文件凭据时安全拒绝。没有令牌缓存或后台刷新线程。

`SERVICE_ENV` 为 prod/canary/空时通过共享 Consul 客户端查 `bytedance.bconfig.proxy`，其他环境查 `bytedance.bconfig.envs_proxy`。目标 cluster 恒为 `default`；发送方 cluster 仅写身份头。Consul 超时 500 ms，按权重选数字 socket 地址。无 BNS/CDN 或公开 endpoint 覆盖。

原生 HTTP GET 为 `http://<数字地址>/v1/keys/tcc/v2/meta/{namespace}` 与 `/v1/keys/tcc/v2/data/{namespace}/{space}`，带校验后的 `x-env/x-psm/x-cluster/x-dps-token`，客户端标识 `ask-auth/<version>`。禁止代理、重定向、referer、自动重试及解压；连接/读取/请求有界，单请求含响应体一秒，整个快照三秒。每次新读 meta 与 data；任何新失败不得返回旧成功值。

## 四个 TCC JSON key

每个 key 的值是 JSON **字符串**，不是 TCC 嵌套对象。结构必须为对象，拒绝未知/重复字段和超过 64 层 JSON。示例占位符必须替换为已批准部署输入。

`ask_auth_runtime` 最小禁用值：

```json
{"schema_version":1,"is_enabled":false}
```

禁用启动不解析 secret、不建库池/密钥环/provider；`/healthz` 为 200，`/readyz` 为 503，启用需滚动重启。完整启用值：

```json
{
  "schema_version":1,
  "is_enabled":true,
  "app_id":"<registered-app-id>",
  "environment":"boe",
  "deployment":"<stable-crypto-deployment>",
  "callback_url":"https://<tlb-domain>/v1/oauth/doubao/callback",
  "required_scopes":["offline_access"],
  "current_key_version":1,
  "database_ca_path":"/run/ask-auth/mysql-ca.pem",
  "api_pool_connections":8,
  "cleanup_pool_connections":2
}
```

环境匹配平台和命令；AppID 为 1–256 可见 ASCII，deployment 非空最长 256 字节。callback 必须规范 HTTPS 且路径精确匹配示例，不含 userinfo/query/fragment。scope 唯一、含 offline_access，1–100 项合计不超过 4096 字节。版本正数，CA 绝对路径，各池 1–32 连接。

`ask_auth_secrets` 必须使用加密配置：

```json
{
  "schema_version":1,
  "client_secret":"<secret-from-oauth-registration>",
  "database_url":"<runtime-writer-mysql-url>",
  "keys":[{
    "version":1,
    "hmac_key":"<43-character-unpadded-base64url-random-32-byte-key>",
    "aead_key":"<different-43-character-unpadded-base64url-random-32-byte-key>"
  }]
}
```

通过批准的密钥管理生成独立随机 32 字节 HMAC/AEAD 密钥。1–64 个唯一正版本，密钥采用规范无 padding base64url，整个环内不得重复，包含当前版本；持久化旧行仍需对应旧版本。自有密钥/字符串释放时擦除，第三方 HTTP/parser/环境变量分配不保证完全擦除。

`ask_auth_admission` 的四个列表即使禁用也必需。ID 来自已校验豆包 open_id/tenant_key，拒绝名单优先，空允许列表拒绝所有人。ID 非空、最长 256 字节、无控制字符，列表无重复，合计最多 10,000 项。

```json
{"schema_version":1,"is_enabled":false,"allowed_users":[],"allowed_tenants":[],"denied_users":[],"denied_tenants":[]}
```

`ask_auth_cleanup` 可为最小禁用值或完整启用值，独立库池且不请求豆包：

```json
{"schema_version":1,"is_enabled":false}
```

```json
{"schema_version":1,"is_enabled":true,"interval_seconds":15,"batch":100,"pass_budget_ms":5000,"transaction_budget_ms":1000,"rows_per_second":20}
```

间隔 1–300 秒，批量 1–500，单轮预算 1–5000 ms，事务 1–1000 ms 且不超过单轮预算，速率 2–100 行/秒。

## 配置生效、TLS 与独立迁移

启用启动先校验配置/provider/crypto，再连接 API 和 cleanup 两个独立有界 writer 池，强制 VerifyIdentity TLS；同一 `Arc<Crypto>` 供存储及 LoginService 使用。两个池连接后再取新鲜快照，数据库 UTC 减去向上取整的快照年龄播种策略，超过三十秒拒绝。`serve` 永不执行迁移。

发布器每次读完等待十五秒；源/解析/时钟失败立即清空 admission 和 cleanup，后续有效读取恢复。通道关闭默认拒绝。runtime/secrets 任意原始字节变化（含空白、禁用、删除 key）清空策略并以固定 restart-required 错误退出，必须重启；admission/cleanup 在线生效。相等 cleanup 策略刷新不重置定时器。

SIGINT/SIGTERM 同时取消启动。宿主直接持有 HTTP/cleanup/publisher futures；信号、错误、退出或 panic 先清空策略，再最多五秒排空 HTTP/cleanup、最多三秒关闭精确池。部分启动失败/取消也关闭已获取池，无后台 DPS/发现工作线程。

单次 `migrate` 使用独立 DDL 账号的受保护 JSON 文件：

```json
{"schema_version":1,"database_url":"<separate-migration-account-mysql-url>","database_ca_path":"/run/ask-auth/mysql-ca.pem","maximum_connections":1}
```

路径必须绝对、普通文件、root/当前有效 UID 所有、权限恰好 0400 或 0600，大小 1–32768 字节。叶符号链接/FIFO/目录/特殊文件在读取前拒绝，采用 `O_NOFOLLOW/O_NONBLOCK` 和 fstat，固定擦除缓冲区。迁移 VerifyIdentity TLS，保留原嵌入 DDL、advisory lock/checksum/dirty 校验，所有退出路径关闭池；dirty/checksum 需操作员审查修正，不自动 repair/drop。

运行账号仅授权现有认证表必要 SELECT/INSERT/UPDATE/DELETE，无 DDL/admin。MySQL >=8.0.21、16 KiB 页、UTC、strict SQL、utf8mb4_0900_bin、writer DNS 与证书 SAN 匹配，挂载身份校验 CA。cleanup 另需 ROW replication 和 READ COMMITTED。生产不使用 LoopbackTlsTestOnly。

## TLB、监控与发布验收

TLB 终止 HTTPS，受控后端路径转发 HTTP/1；注册精确 HTTPS callback。保留边缘限制：创建 5 次/IP/分钟，轮询 1 次/auth_id/秒，刷新 6 次/auth_id/分钟。放通本地 Consul、发现的 TCC、MySQL 与固定豆包 authorization/token/userinfo 出站；不添加任意 provider URL。

liveness 使用 `/healthz`，流量 readiness 使用 `/readyz`。启动探针至少六十秒，终止宽限至少十五秒。先单 BOE 副本验新鲜 TCC/ACL、CA、健康/就绪、默认拒绝与显式允许/拒绝、注册 OAuth 和轮换；M0 门禁后再双副本验共享库状态、策略撤销、清理及滚动重启。以上真实验收仍待执行，本地合成测试不能证明生产可用。

日志只允许 `ask_auth::request`、`ask_auth::cleanup`、`ask_auth::config`，配置事件带生成的 UUIDv7 request_id、固定 status 和启用布尔值；上游错误/panic 内容被抑制。不记录 token/AppSecret/DSN/身份/原始配置/哈希/TCC 版本。关注 readiness 丢失、configuration unavailable/restart_required、cleanup 积压/失败、HTTP 失败及反复重启。

## 验证边界

普通测试结合纯注入环境、fake reader 和真实 loopback HTTP，不改进程全局环境，不连接真实 TCC/Consul/OAuth。decoder 保留资源精确匹配、版本一致、space 恰好一次、非空 gray 拒绝、深度/数量/载荷限制：wire 16 MiB、decoded meta 2 MiB/content 8 MiB、space/entry 128、单载荷 256 KiB/选中总计 1 MiB。原生传输不是官方 SDK，真实协议与工作负载 ACL 需要 BOE 验收。

```sh
cargo test -p ask-auth --locked
cargo test -p tos-core --locked discovery
cargo check --workspace --locked
python3 services/ask-auth/tests/validate_contracts.py
bash -n services/ask-auth/build.sh services/ask-auth/run.sh
```

三个忽略的实际 TLS/factory/migration/worker 测试只允许一次性测试库。操作员显式设置 `ASK_AUTH_TEST_DATABASE_URL`、`ASK_AUTH_TEST_MIGRATION_URL`、绝对 `ASK_AUTH_TEST_CA`；两个 URL 必须为数字 `127.0.0.1` 且 schema 恰好 `ask_auth_stage3_test`，连接前双重检查。这些变量不会成为生产宿主回退。

```sh
cargo test -p ask-auth --locked --lib platform::factory::tests::production_tls_ -- --ignored --test-threads=1
```
