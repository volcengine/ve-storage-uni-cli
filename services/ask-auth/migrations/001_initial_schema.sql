-- ask authorization schema v1.15, initial and not deployed; MySQL >= 8.0.21.
-- InnoDB 16 KiB pages, DYNAMIC rows, UTC connections and strict SQL mode.
-- [Review Fix #14] One authorization session owns login and authorization phases;
-- auth_id is the sole public resource ID and CLI-generated creation key.
-- [Review Fix #15] Clear login digests before migrating the shared session HMAC
-- version; each refresh request retains its own immutable HMAC version.
-- [Review Fix #16] The version covers HMAC digests, not the SHA-256 challenge.
-- [Review Fix #17] status/logout use auth_id PK lookup and per-row broker proof;
-- no broker unique or ordinary index is required.
-- [Review Fix #21] All locally generated UUID roles use UUIDv7 in RFC byte order.
-- Phase shapes and operation guards are enforced by the shared Store, not CHECK.
-- This replaces an undeployed initial design; it is not an upgrade migration.

CREATE TABLE `auth_sessions` (
  `auth_id` BINARY(16) NOT NULL COMMENT 'CLI 创建前生成的 auth_id UUIDv7；兼作登录创建幂等主键，所有 HTTP 和凭据组统一使用；不是豆包 session_id 或凭据',
  `app_id` VARCHAR(256) NOT NULL COMMENT '创建时的配置应用 ID；必须匹配当前部署，不随配置切换改写',
  `status` VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL COMMENT 'pending / exchanging / ready / active / refreshing / denied / error / expired / revoked / reauth_required',
  `request_hash` BINARY(32) NULL COMMENT '登录完整规范化请求的 HMAC；登录临时组清理后为 NULL，不能据主键重建操作',
  `login_ticket_hash` BINARY(32) NULL COMMENT 'CLI 高熵 login_ticket 的 HMAC；不保存原文，登录临时组清理后为 NULL',
  `claim_challenge` BINARY(32) NULL COMMENT 'SHA-256(ASCII(claim_verifier))；登录临时组清理后为 NULL',
  `oauth_state_hash` BINARY(32) NULL COMMENT '完整 OAuth state 的 HMAC；回调和清理验证原操作，未授权终态保留至删除',
  `hmac_key_version` SMALLINT UNSIGNED NOT NULL COMMENT '本行 HMAC 摘要使用的非零 TCC HMAC 密钥版本；成功刷新先清登录临时组再迁移版本',
  `encrypted_oauth_context` MEDIUMBLOB NULL COMMENT '短期 AEAD 信封：state / 授权 URL / 平台支持时的 PKCE verifier；ready 起清除',
  `claim_request_id` BINARY(16) NULL COMMENT '首次领取 UUIDv7 幂等键；仅首次领取后的有效登录临时组保留，清理时置 NULL',
  `encrypted_result` MEDIUMBLOB NULL COMMENT '第 1 代登录 TokenBundle 的短期 AEAD 信封；领取重放后清理，刷新成功同时清除',
  `login_expires_at` DATETIME(3) NULL COMMENT '登录临时期截止；首领前为创建后 10 分钟，首领后为最多 5 分钟重放截止，整组清理后为 NULL',
  `user_id` VARCHAR(256) NULL COMMENT '平台验证后的可信用户 ID，保留大小写；授权建立前为 NULL',
  `tenant_id` VARCHAR(256) NULL COMMENT '平台验证后的可信租户 ID；个人账号及授权建立前为 SQL NULL',
  `scope_json` JSON NULL COMMENT '平台验证并规范化的 scope 数组，最多 100 项；授权建立前为 NULL，刷新不得自由扩权',
  `broker_hash` BINARY(32) NULL COMMENT '当前 broker_token 的 HMAC；按 auth_id 主键定位后用本行版本验证，不用于反查，授权建立前为 NULL',
  `refresh_token_hash` BINARY(32) NULL COMMENT '当前 refresh token 的 HMAC；不长期保存原文，授权建立前为 NULL',
  `credential_generation` BIGINT UNSIGNED NULL COMMENT '授权建立时为 1，成功刷新加 1；授权建立前为 NULL，禁止溢出',
  `auth_expires_at` DATETIME(3) NULL COMMENT '授权及 broker 统一截止，受已知 RT 期限约束；授权建立前为 NULL，结束授权时截断至 now',
  `reason_code` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NULL COMMENT '脱敏错误枚举；不保存上游正文或敏感资料',
  PRIMARY KEY (`auth_id`),
  KEY `ix_session_login_expiry` (`login_expires_at`),
  KEY `ix_session_auth_expiry` (`auth_expires_at`)
) ENGINE=InnoDB ROW_FORMAT=DYNAMIC DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_bin;

CREATE TABLE `auth_refresh_requests` (
  `auth_id` BINARY(16) NOT NULL COMMENT '所属授权会话 auth_sessions.auth_id UUIDv7；HTTP 统一使用 auth_id',
  `request_id` BINARY(16) NOT NULL COMMENT 'CLI 发送前持久化的 UUIDv7 幂等键；与 auth_id 组成唯一执行身份',
  `request_hash` BINARY(32) NOT NULL COMMENT '完整规范化请求 HMAC：操作 / auth_id / 输入 broker / RT / 代次及全部语义字段',
  `hmac_key_version` SMALLINT UNSIGNED NOT NULL COMMENT '本次请求固定的非零 TCC HMAC 密钥版本；不跟随会话版本迁移',
  `input_generation` BIGINT UNSIGNED NOT NULL COMMENT '输入凭据代次；成功输出为 input_generation + 1，禁止溢出',
  `status` VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL COMMENT 'pending / completed / unknown / failed；终态不回 pending',
  `expires_at` DATETIME(3) NOT NULL COMMENT 'pending 为执行截止，completed / failed 为最多 5 分钟重放截止，unknown 为 now；重放不续期',
  `encrypted_response` MEDIUMBLOB NULL COMMENT 'completed 的短期 AEAD TokenBundle 信封；输出代次为 input_generation + 1',
  `error_code` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NULL COMMENT 'unknown / failed 的脱敏错误码；failed 仅用于已证明上游未消费凭据',
  PRIMARY KEY (`auth_id`, `request_id`),
  KEY `ix_refresh_expiry` (`status`, `expires_at`)
) ENGINE=InnoDB ROW_FORMAT=DYNAMIC DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_bin;
