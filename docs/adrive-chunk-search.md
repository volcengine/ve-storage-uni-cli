# Space 文档片段检索

Type: Change。新增只读客户端入口，不修改服务端索引、配置或文件。

## 使用

`chunk` 归属 Low-Level Commands；根 `--help` 只展示 `chunk` 入口，
`chunk --help` 展开 search/read。Describe 与 capabilities 使用 `low-level`
层级（筛选可写 `--layer low_level`）；导出的 Skill 归属 `adrive-chunk`，
MCP 工具名称及命令参数保持一致。

实例需已开启 Doc 与文档切片，用户对目标目录及文件需有读取权限。
复用 `ve-adrive-cli auth login` 的 OAuth profile、实例绑定和 Token 续期；
不接受 AK/SK 或 Unified 签名替代用户授权，不自动登录或换管理员身份。
查询不修改服务端资源；Token 续期可能更新所选 profile 的本地凭证。

```bash
ve-adrive-cli chunk search adrive://instance-id/space-id/handbook/ \
  --text '出差住宿标准是多少' --limit 5 --output json
ve-adrive-cli chunk read adrive://instance-id/space-id/handbook/travel.txt \
  --etag 'etag-from-current-search' --ordinals 0,1 --output json
```

可改用 `--instance`、`--space` 和 `--folder`（search）/`--file`（read）。
选择器为 ID；不混用 URI 与显式选择器，不隐式进行名称解析。
search 默认包含子目录，`--no-recursive` 仅查当前目录。`--text` 是检索文本；
全局 `--query` 仍用于 JMESPath 输出过滤。`--range START-END` 与 `--ordinals` 二选一。

## 契约

- POST 实例业务域名的 `/v2/file/search_chunk` 或 `/v2/file/read_chunk`。
  公共 endpoint `https://idsapi.volces.com` 自动定位到 `<InstanceId>.idsapi.volces.com`；
  显式实例域名须匹配目标实例。自定义接入地址保持原样，由管理员保证其路由到目标实例。
  不使用不存在的 `/v2/instances/.../file/` 路由，OAuth 实例绑定在每次请求及续期后校验。
- search 请求体为 `SpaceIds`、`FolderPath`、`Recursive`、`SearchQuery[].Text`、`Limit`。
  文本去首尾空白后非空且不超过 100 UTF-8 字节，Limit 为 1～100，默认 10。
- read 请求体为 `SpaceId`、`FilePath`、`ETag`，加 `ChunkOrdinals` 或闭区间 `ChunkRange`。
  片号非负、不重复，单次最多 20 片。实例、文件、版本和片号须来自同一条命中。
- 返回标准 Envelope，服务片段保留于 `data.Items`，包括正文、版本与截断标记。
  空 Items 是成功的无匹配，不是索引就绪或无内容的证明。默认不轮询空结果。
- 复用现有只读请求重试与一次 401 刷新；403、404、409 保留服务错误，
  不自动改配置、覆盖文件或触发存量。ChunkTruncated 的未索引正文无法靠扩读恢复。
- `--describe` 与 `--dry-run` 离线执行，不读取 Token、访问网络或改变配置；
  dry-run 校验参数并返回请求计划，不返回伪造的检索结果。
- 两个命令同步进入 help、describe、capabilities、Skill 导出和 MCP 发现。

## 验证

解析与领域校验测试覆盖选择器、UTF-8 长度、数量边界、片号冲突和请求字段。
本地 HTTP fixture 覆盖真实二进制的 OAuth、V2 路由、JSON 请求、空命中、版本错误、
权限错误、实例绑定、401 刷新、输出与离线预演。运行受影响 crate 和 workspace 回归。
本地 fixture 不代表北京环境验收；MR 合入不代表发布安装包或完成真实业务验证。

运行本地回归（不读取个人凭证）：

```bash
cargo build --locked
cargo build --locked --manifest-path packaging/cargo/ve-adrive-cli/Cargo.toml --target-dir target
python3 tests/chunk_search_regression.py target/debug/ve-storage-uni-cli target/debug/ve-adrive-cli
cargo test --workspace --locked --no-fail-fast -- --test-threads=4
```

兼容性：仅新增 `chunk` 命令，不改变现有文件、分享命令或凭证格式。
回退到旧版 CLI 不需要数据迁移；旧版不提供这两个命令。
发布后仍需在已开通文档切片的实例验证真实检索和版本扩读。
