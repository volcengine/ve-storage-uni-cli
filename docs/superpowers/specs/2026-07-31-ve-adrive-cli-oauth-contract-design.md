# ve-adrive-cli OAuth 完整闭环契约冻结稿

## 1. 文档状态与适用范围

- 状态：**Frozen**
- 冻结日期：2026-08-01
- 适用客户端：仅 `ve-adrive-cli`
- 服务端依据：[IDS CLI 登录技术方案](https://bytedance.larkoffice.com/wiki/OScPwVIMyiNBlbkDOzPcrbWwnLb)，Revision 23
- 关联设计：[Unified Credentials Store Design](./2026-07-11-credentials-store-design.md)

本稿冻结 `ve-adrive-cli` 的 Device Authorization 登录、Token 持久化、Bearer 资源请求、Refresh Token 轮换和认证失败恢复行为。`ve-tos-cli` 与 `tos-cli` 不支持 Auth Mode，其现有 AK/SK 行为保持不变。

本稿是关联凭证设计在 ADrive OAuth 场景下的增量契约；发生冲突时，以本稿为准。

## 2. 冻结原则

1. OAuth 与 AK/SK 是两套显式、互斥的认证策略，不做隐式探测或相互降级。
2. OAuth 使用 RFC 8628 Device Authorization Grant；CLI 是 Public/Native Client，不持有或发送 `client_secret`。
3. ADrive Resource Server 的所有接口都接受 Bearer Token。
4. IDS OAuth Authorization Server 使用独立于 ADrive Resource Server 的域名与配置项。
5. 登录时固定发送字面值 `scope=all`，CLI 不展开、不允许覆盖该值。
6. 只有明确可恢复的认证失败才触发 Refresh Token；CLI 永远不会自动发起需要用户参与的重新登录。
7. Access Token、Refresh Token、Device Code 不得出现在 URL、普通输出、日志或错误信息中。

## 3. 认证模式与兼容边界

Auth Mode 继续使用既有优先级：

```text
--auth-mode > config.toml [profile.adrive].auth_mode > ADRIVE_AUTH_MODE > aksk
```

- 有效模式为 `aksk` 或 `oauth`。
- `oauth` 模式只读取 OAuth 凭证，不读取或回退到 AK/SK。
- `aksk` 模式完全沿用当前签名、Endpoint、重试和错误处理逻辑，不读取 OAuth Token。
- `auth login` 和 `auth logout` 要求有效模式为 `oauth`。若有效模式为 `aksk`，命令返回参数校验错误且不修改任何文件。用户可以用当前命令的 `--auth-mode oauth` 显式覆盖配置。
- 本功能不迁移、不删除、不双写 `config.toml` 中已有的 AK/SK。

## 4. Endpoint、Client 和登录输入

### 4.1 两类 Endpoint

| 配置 | 用途 | 冻结规则 |
|---|---|---|
| `endpoint` | ADrive Resource Server | 必须显式配置；允许从可识别 Endpoint 解析 Region，不允许从 Region 构造 Endpoint |
| `auth_endpoint` | IDS OAuth Authorization Server | 必须通过命令行、配置文件或环境变量显式配置，不得从资源 `endpoint` 或 Region 推导 |

`auth_endpoint` 与资源 `endpoint` 是两个独立域名。Device Authorization 和 Token 请求只发送到 `auth_endpoint`；业务请求只发送到资源 `endpoint`。两个 Endpoint 解析后的 Origin 必须不同，否则 CLI 在发请求前返回配置错误。

Endpoint 必须是无 Userinfo、Query 和 Fragment 的绝对 HTTP(S) Base URL，比较和持久化前统一规范化 Scheme、Host、默认端口和末尾斜杠。正式与持久配置必须使用 HTTPS；仅测试时允许显式使用 HTTP loopback 地址。

### 4.2 输入解析优先级

| 输入 | 优先级 | 缺失行为 |
|---|---|---|
| Auth Endpoint | `--auth-endpoint` > `[profile.adrive].auth_endpoint` > `ADRIVE_AUTH_ENDPOINT` | 返回 `oauth_auth_endpoint_required`，不发请求 |
| OAuth Client ID | 非空 `ADRIVE_OAUTH_CLIENT_ID` > CLI 内置常量 `global_74c584` | 正式发行使用内置线上 Client ID；测试环境通过进程环境变量覆盖 |
| Instance ID | `--instance` > `[profile.adrive].default_instance` > `ADRIVE_DEFAULT_INSTANCE` | 返回参数缺失错误，不发请求 |
| Device Name | `--device-name` > `ADRIVE_DEVICE_NAME` > 本机 hostname > `ve-adrive-cli` | 使用 `ve-adrive-cli` |

Client ID 是公开标识，不是密钥。发行版内置线上值固定为 `global_74c584`。它不提供命令行或配置文件参数，也不写入 credentials；普通用户始终使用发行版内置值，测试环境可以通过 `ADRIVE_OAUTH_CLIENT_ID` 临时覆盖。Device Name 长度为 1–64 个 UTF-8 字符，禁止控制字符，不写入配置或凭证文件。

## 5. CLI 命令契约

### 5.1 登录

```text
ve-adrive-cli [--profile NAME] [--auth-mode oauth] auth login \
  [--instance INSTANCE_ID] \
  [--auth-endpoint URL] \
  [--device-name NAME]
```

`auth login` 是前台阻塞命令。它创建 Device Grant、立即展示服务端返回的完整验证地址及二维码，并在同一进程内轮询 Token。它不启动守护进程或后台常驻任务。

人类可读模式展示二维码、`verification_uri_complete` 和 `user_code`。机器可读模式保持最终 stdout envelope 的结构化完整性，登录过程提示写到 stderr。任何模式都不得输出 `device_code`。

`--dry-run` 只输出脱敏后的目标 Auth Origin、路径、Client ID、Instance ID、Device Name 和固定 Scope，不调用服务端、不创建 Device Grant、不生成二维码、不进入轮询。

### 5.2 状态与退出

- `auth status` 不发网络请求，只展示有效 Auth Mode、来源、凭证来源、Access/Refresh Token 是否存在、Access Token 过期状态、Scope，以及已知时的登录 Instance；Token 值必须脱敏。
- `auth logout` 仅原子删除当前 Profile 的 `[<profile>.adrive.oauth]`，不影响 AK/SK、其他 Profile 或环境变量。
- `auth logout --dry-run` 只报告当前 Profile 是否存在待清理 OAuth 凭证，不改写 `credentials.toml`，也不创建本地加密密钥材料。
- 当前版本没有服务端 Revoke 调用；`logout` 的输出必须明确标记为 `local_only`。

### 5.3 OAuth 模式下的顶层 `ls`

无目标参数的 `ve-adrive ls` 始终表示“列举当前身份可见的 Instance”，不能因为 Auth Mode 不同而改成列举 Space：

| Auth Mode | `ve-adrive ls` 行为 |
|---|---|
| `aksk` | 保持现有行为，调用 `list_instances` 并返回所有可见 Instance |
| `oauth` | 从当前 OAuth 文件凭证读取绑定的 `instance_id`，调用 `get_instance`，按 Instance 列表格式返回这一个 Instance |

OAuth 返回结构继续使用 `scope=instances` 和 `instances` 数组，数组中恰好有一个通过 `get_instance` 获取的完整 Instance；`next_marker` 为空且 `is_truncated=false`。列选择和显式 Manifest 行为保持现有 Instance 列表规则。由于该结果不可分页，OAuth 裸 `ls` 显式传入非空 `--marker` 时返回参数校验错误，不发资源请求。

显式目标不参与上述转换：`ve-adrive ls --instance <id>` 和 `ve-adrive ls adrive://<id>` 仍表示列举该 Instance 下的 Space。目标 Instance 与 OAuth 凭证不一致时继续在网络请求前拒绝。

环境变量 OAuth 凭证以及缺少 `instance_id` 的历史文件凭证无法推断唯一 Instance。此时裸 `ls` 返回 `oauth_instance_required`，用户必须显式提供 `--instance` 或 `adrive://<instance-id>`；CLI 不调用 `list_instances`，也不从普通 Profile 的 `default_instance` 猜测 Token 绑定关系。

## 6. Device Authorization 契约

CLI 调用：

```http
POST {auth_endpoint}/v1/oauth/device_authorization
Content-Type: application/x-www-form-urlencoded; charset=UTF-8
```

请求字段固定为：

| 字段 | 规则 |
|---|---|
| `client_id` | 必填，`ADRIVE_OAUTH_CLIENT_ID` 非空时使用环境变量值，否则使用 CLI 内置的线上 Public Client ID `global_74c584` |
| `instance_id` | 必填，只接受 Instance ID，不接受名称 |
| `device_name` | 必填，使用解析后的 Device Name |
| `scope` | 必填，固定字面值 `all` |

服务端把 `all` 作为快捷表示展开为该 App 对目标 Instance 可用的最终权限集合；CLI 不在本地展开或缓存快捷表示本身。

成功响应必须包含非空 `device_code`、`user_code`、`verification_uri`、`verification_uri_complete`，以及大于 0 的 `expires_in` 和 `interval`。字段缺失或非法时登录立即失败，旧凭证保持不变。

`device_code` 是 CLI 后通道敏感凭据，只保存在本次登录进程内存中。CLI 必须原样使用 `verification_uri_complete` 生成二维码，不对 URL 解码、重编码或自行拼接 User Code；`verification_uri` 与 `user_code` 只作为扫码失败时的备用方式展示。

## 7. Device Code 轮询契约

CLI 在 Device Grant 有效期内调用：

```http
POST {auth_endpoint}/v1/oauth/token
Content-Type: application/x-www-form-urlencoded; charset=UTF-8

grant_type=urn:ietf:params:oauth:grant-type:device_code
device_code=<in-memory device code>
client_id=<resolved client id>
```

第一次轮询发生在等待一个 `interval` 后。轮询使用单调时钟控制间隔和总截止时间，系统时间调整不得造成高频轮询或越过 `expires_in`。

| OAuth `error` | CLI 行为 |
|---|---|
| `authorization_pending` | 按当前 interval 继续 |
| `slow_down` | 将 interval 永久增加至少 5 秒；响应包含更大合法 interval 时取更大值，然后继续 |
| `access_denied` | 立即终止，返回用户拒绝授权 |
| `expired_token` | 立即终止，返回登录会话已过期 |
| 其他错误 | 立即终止，不继续轮询 |

临时网络错误可以在 Device Grant 总截止时间内重试，但不得快于当前 interval。收到 Ctrl-C、进程终止、截止时间到达或任一终态后立即停止；未取得完整 Token 响应时绝不写凭证文件。

## 8. Token 响应与凭证持久化

成功 Token 响应必须包含：

- 非空 `access_token`
- 非空 `refresh_token`
- `token_type=Bearer`，比较时不区分大小写
- 大于 0 的 `expires_in`
- `scope`
- 与登录请求一致的 `instance_id`

`user_id` 可用于成功输出，但不作为本地认证决策依据。CLI 使用本地接收时间加 `expires_in` 计算 RFC 3339 UTC 格式的绝对 `expires_at`。

完整校验通过后，CLI 才把 Token 和来源元数据原子写入当前 Profile：

```toml
[default.adrive.oauth]
access_token = "ENC:..."
refresh_token = "ENC:..."
expires_at = "2026-07-31T12:00:00Z"
token_type = "Bearer"
scope = ["file:read", "file:list"]
instance_id = "inst_123"
auth_endpoint = "https://idsauth.volces.com"
```

服务端 `scope` 字符串按 ASCII 空白分隔后写成数组；CLI 保存服务端实际返回的 resolved scope，而不是把请求中的 `all` 当作响应 Scope。

Access Token 和 Refresh Token 使用现有 AES-256-GCM `ENC:` 机制加密；元数据可以明文保存。文件及本地密钥继续使用 Unix `0600`，并采用临时同目录文件加 rename 的原子写入。一次成功登录只保留当前 Profile 的一组 OAuth 凭证；登录其他 Instance 会在新 Token 完整落盘后替换旧组。

`instance_id` 和 `auth_endpoint` 是 OAuth 子表的来源元数据，新登录写出的 OAuth 记录必须包含它们。`client_id` 不写入 config 或 credentials：登录和 Refresh 分别在当前进程中按 `ADRIVE_OAUTH_CLIENT_ID` 非空值优先、CLI 内置线上值兜底的规则解析。兼容读取早期构建已经写入的 `client_id`，但不使用该值且后续写回时删除。历史记录缺少来源元数据但 Access Token 仍有效时可以继续发送 Bearer 请求，由 Resource Server 校验 Instance；它不能自动刷新，也不能执行本地 Instance 一致性预检。Access Token 失效后返回 `login_required`。

OAuth 凭证按整组选择，不允许把 credentials 文件中的 Access Token 与环境变量中的 Refresh Token 混合成一组：

```text
credentials.toml 中存在 OAuth Token -> 使用完整文件凭证组
否则 -> 使用 ADRIVE_ACCESS_TOKEN / ADRIVE_REFRESH_TOKEN 环境凭证组
```

环境变量凭证只读、不落盘。由于 Refresh Token Rotation 无法安全回写调用方环境，环境变量来源不执行自动 Refresh；调用方必须提供仍然有效的 Access Token。需要 CLI 管理完整生命周期时必须使用 credentials 文件。

## 9. Bearer 资源请求契约

OAuth 模式下，所有 ADrive Resource Server 请求使用：

```http
Authorization: Bearer <access_token>
```

同一请求不得再添加 AK/SK 签名认证头。业务参数、资源 Endpoint、超时、并发控制和与认证无关的重试策略保持现有行为。

Resource Server 对 Bearer Token 使用以下统一 HTTP 语义：`401` 表示 Token 缺失、无效或过期，`403` 表示 Token 已通过认证但没有执行目标操作的权限。CLI 的刷新判断以该状态语义为准，不把 `403` 当作 Token 失效。

对带明确 Instance ID 的业务命令，目标 Instance 必须与凭证中的 `instance_id` 一致；不一致时在发请求前返回 `login_required`。不绑定具体 Instance 的接口直接使用当前 Bearer Token，由 Resource Server 校验 Token 权限。

## 10. Refresh Token 契约

仅 credentials 文件来源支持自动刷新。Access Token 剩余有效期不超过 60 秒，或文件中没有 Access Token 但有 Refresh Token 时，CLI 在发送业务请求前调用：

```http
POST {stored_auth_endpoint}/v1/oauth/token
Content-Type: application/x-www-form-urlencoded; charset=UTF-8

grant_type=refresh_token
refresh_token=<stored refresh token>
client_id=<ADRIVE_OAUTH_CLIENT_ID or built-in production client id>
```

刷新不是轮询：每次刷新只发送一次逻辑请求，网络层可以执行既有的有限瞬时错误重试。Refresh Token 绑定 Client 和 Instance，服务端可以采用 Rotation；成功响应必须返回新的 Access Token 和非空 Refresh Token。返回的 Refresh Token 可以与旧值相同，也可以不同，CLI 不比较两者，完整校验后始终原子替换整组 Token 并重新计算 `expires_at`。

CLI 不维护或推断 Refresh Token 的本地过期时间，也不为了给 Refresh Token 续期而启动后台任务。Access Token 需要刷新时，CLI 直接提交当前 Refresh Token：成功则保存服务端返回的整组 Token；`invalid_grant` 则按登录失效处理。

CLI 永远使用凭证中记录的 `auth_endpoint` 刷新，并在每次 Refresh 进程中重新解析 `client_id`：`ADRIVE_OAUTH_CLIENT_ID` 非空时优先，否则使用 CLI 内置的线上值。当前配置只影响下一次显式登录，不改变现有凭证的签发域名。测试环境必须在需要 Refresh 的每次命令中继续提供相同的 `ADRIVE_OAUTH_CLIENT_ID`；凭证缺少 `auth_endpoint` 或 `instance_id` 时不自动刷新，返回 `login_required`。

为避免多个进程同时消费同一轮换型 Refresh Token，同一 Profile 的 Refresh 临界区必须持有跨进程排他锁。持锁后重新读取凭证；若其他进程已经刷新且新 Access Token 仍有效，则直接复用，不再请求服务端。锁只覆盖“重新读取、调用 Refresh、原子保存”过程，不影响普通读取、普通配置写入或其他 Profile。锁文件使用 credentials 路径和 Profile 派生的稳定无敏感信息名称，等待超时为 10 秒，锁和临时文件均不得包含 Token。

`config.toml`、普通 `credentials.toml` 修改和 `auth login` 最终落盘均不加通用文件锁，只使用同目录临时文件加 rename 保证单次写入原子性；并发修改采用 last-writer-wins。

## 11. 认证失败恢复契约

| 场景 | 行为 |
|---|---|
| Access Token 剩余不超过 60 秒 | 请求前刷新一次 |
| OAuth 业务请求首次返回 401，且有可刷新的文件凭证 | 强制刷新一次，原请求最多重试一次 |
| 刷新返回 `invalid_grant`，表示 Refresh Token 无效、过期或已撤销 | 原子清除当前 OAuth Token，返回 `login_required` |
| 刷新返回其他终态错误 | 保留现有凭证，返回对应认证错误 |
| 刷新成功后重试仍返回 401 | 不再刷新或重试，返回认证失败并提示显式登录 |
| 任意业务请求返回 403 | 不刷新、不自动登录，返回权限错误 |
| 429、5xx 或网络错误 | 沿用普通请求重试策略，不改变本地认证状态 |

Refresh Token Endpoint 返回 OAuth 错误时采用以下映射：

| OAuth error | CLI 行为 |
|---|---|
| `invalid_request` | 不重试、不清凭证，返回请求或本地凭证校验错误 |
| `invalid_client` | 不重试、不清凭证，返回 Client 配置错误 |
| `unauthorized_client` | 不重试、不清凭证，返回应用状态、Grant Type 或 Instance 授权错误 |
| `invalid_scope` | Refresh 请求不发送 Scope；按服务端或凭证契约错误处理，不重试、不清凭证 |
| `invalid_grant` | 原子清除当前 Profile 的 OAuth 凭证，返回 `login_required` |
| `access_denied` | 停止刷新并返回账号或应用权限错误，不自动登录 |
| `unsupported_response_type` | Refresh 不应返回；按协议错误处理，不重试 |
| `unsupported_grant_type` | 按 Client/Server 契约不匹配处理，不重试 |
| `temporarily_unavailable` | 遵守合法 `Retry-After` 或退避策略有限重试，保留凭证 |
| `server_error` | 有限退避重试，保留凭证，并输出脱敏后的 `request_id` |

自动重试原请求的上限为一次。请求体必须能够被安全重建；不可重放的流式请求不得在底层静默重试，而应把可恢复错误交给现有分片或 Checkpoint 层处理。

任何错误都不能自动回到 Device Authorization 流程。重新登录需要用户显式执行 `auth login`。

## 12. 安全与可观测性

- Token、Device Code、凭证加密密钥不得实现 `Debug` 明文输出。
- URL、请求体、错误上下文和 trace 日志必须按字段脱敏；`device_code`、Access Token、Refresh Token 完全隐藏，`user_code` 只允许在登录引导中展示。
- 日志可以记录 Profile、Instance、Auth Mode、Endpoint host、OAuth error、HTTP status 和 `request_id`。
- `auth status`、`config show`、`doctor` 只能输出 Token 是否存在、来源和过期状态；`doctor` 还可以输出不含具体值的 Client ID 来源和 placeholder 状态，不得输出 Token 或 Client ID 原值。
- 登录成功写入失败时返回本地持久化错误，不得声称登录成功；旧凭证必须保持可用。

## 13. 实现边界

建议按以下独立组件实现，避免改写现有 AK/SK 客户端逻辑：

1. `OAuthClient`：Device Authorization、Device Code Token、Refresh Token 三类 IDS Auth 请求。
2. `OAuthCredentialManager`：整组解析、过期判断、加锁刷新、原子持久化和清理。
3. `RequestAuthenticator`：在请求发送前选择 HMAC 或 Bearer，确保两种认证头互斥。
4. `AuthHandler`：登录引导、二维码、轮询状态机、状态和退出命令。
5. `RetryCoordinator`：只协调一次 401 刷新重试，不处理 403，也不发起登录。

Resource Client 的业务方法不复制 OAuth 判断；认证差异应收敛在统一的请求发送边界。

## 14. 验收与回归测试

至少覆盖：

1. Device Authorization 请求固定发送 `scope=all`，不发送 `client_secret`。
2. 正常登录经过 Pending 后成功，Token 加密、原子写入正确 Profile。
3. `slow_down` 永久增加间隔；Denied、Expired 和其他错误停止轮询。
4. 非法或不完整 Token 响应不覆盖旧凭证，日志与输出无敏感值。
5. OAuth 模式下所有 Resource API 使用 Bearer 且不含 HMAC 认证头。
6. Access Token 临期主动刷新，Refresh Token Rotation 后整组原子替换。
7. 首次 401 只刷新并重试一次；403、429、5xx 不触发重新登录。
8. 两个并发进程只消费一次 Refresh Token，另一个进程复用新 Token。
9. Profile 和 Instance 不匹配时不会误用 Token；刷新始终使用凭证记录的 Auth Endpoint，并在当前进程重新解析 Client ID。
10. credentials 文件优先于环境变量且不混合字段；环境 Token 不写盘。
11. `auth logout` 只清理当前 Profile 的 OAuth 凭证。
12. OAuth 裸 `ls` 只调用绑定 Instance 的 `get_instance`，返回单元素 `instances`，绝不调用 `list_instances`。
13. OAuth 裸 `ls` 缺少凭证 `instance_id` 或显式传入 Marker 时，在网络请求前返回对应校验错误。
14. AK/SK 裸 `ls` 继续调用 `list_instances`；`tos-cli` 和 `ve-tos-cli` 全量回归测试保持通过。

## 15. 明确不在本期范围

- Authorization Code、PKCE 或 Localhost Callback 流程
- `client_secret`
- 自动启动后台登录守护进程
- 自动重新登录或遇到 403 自动登录
- 服务端 Token Revoke/全局 Logout
- Keychain 存储替换；本期继续使用已落地的加密 `credentials.toml`
- 对旧凭证进行启动时迁移或向 `config.toml` 双写
