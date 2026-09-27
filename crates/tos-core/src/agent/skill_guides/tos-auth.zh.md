## ByteCloud TOS 鉴权

默认使用 `aksk`。可通过 `--auth-mode zti`，或在 Profile 的
`[profile.tos].auth_mode = "zti"` 选择 ZTI。优先级依次为 CLI 参数、Profile、
`BYTETOS_AUTH_MODE`、`aksk`。ZTI 按顺序从 `SEC_TOKEN_STRING`、本地 Agent
（`ZTI_AGENT_SOCKET_PATH`，默认 `/run/zti-agent.sock`）、`SEC_TOKEN_PATH`
获取 Token。Windows 不支持 Agent 来源。文件来源在每次请求凭据时检查变化；
Agent 来源会在到期前刷新，最长间隔 600 秒。ZTI 不读取 AK/SK，也不会回退。

使用 `doctor --check auth` 检查模式及 Token 来源；该离线检查不读取 Token，
也不验证远端访问权限。不要把 Token 值放在 CLI/MCP 参数、日志或导出的 Skill 中。
ZTI 不支持 presign；只有用户授权使用 `aksk` 模式时才能生成签名 URL。
