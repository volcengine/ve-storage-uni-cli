# ChangeLog of Volcengine Storage Unified CLI

## 版本号：v1.0.3（未发布）

- 业务 HTTP 客户端升级至 reqwest 0.13.5，启用平台证书验证及系统 CA，并显式保留 query/form/json/stream 能力；凭据 SDK 暂用的 reqwest 0.12 同时启用系统根证书加载（与其内置 CA 合并）。同步三个独立 CLI 的锁文件及证书说明，保持证书链与主机名校验，沿用原有代理、超时和重试行为。
- 新增整 Space 分享的 CLI 对接：Space 根 URI（可带尾斜杠）或实例/空间 flags 构造 `ShareAllFiles=true`、空 `FilePathList`；非根仍为单路径分享。接收 files 默认或 `--path /` 列举空间根，download 以 `--recursive` 保留完整空间相对层级，根级 include-parent 不增加包装目录；沿用匿名默认、显式 OAuth、cp 传输与无本地写入预演契约，并同步双语发现和验证资料。当前 BOE V1 明确返回 `InvalidParameter.ShareAllFilesNotAllowed`，端到端可用性待服务端开放后验收。
- 补齐 ve-adrive `share download` 与 cp 的适用能力：目录内容默认布局、include-parent、筛选、并发、共享限速、覆盖策略、报告／清单、扫描与传输进度，以及分片断点恢复；默认 force 覆盖，可用 no-clobber/newer 跳过现有文件。普通文件和目录失败保留报告并继续独立文件，未知下载地址签发结果不自动重试。
- 补齐 `share files` 与 ls 的大小格式、排序、显示列和显式清单选项，修复 ls 的 sort 参数未生效，并同步中英 help、Describe、MCP、Skill 和验证文档。检查点采用任务快照与增量完成账本，恢复时须同时保留 JSON、`.completed/` 和 `.parts/`。

- 修复统一入口的 Describe 命令定位：profile 等参数值与 tos/ve-tos/ve-adrive 同名时，正确识别实际服务及叶命令；覆盖短参数组合、参数位置、双语帮助和 MCP 派发。

- 补齐 ve-tos HNS 筛选式递归 mv：与 ADrive 一样只移动选中文件，保留源目录和排除项，避免收尾删除非空目录误报失败；同步 help、describe、能力发现与 skill 文档。

- 扩查非上传批量路径：修复带筛选条件的递归 mv 误删排除项及同源复制后删源风险；实际目标重合或位于源内时提前拒绝。
- 修复 ByteTOS 远端复制的源路径重复编码，完整源路径只编码一次，保留中文及字面百分号等名称。
- 修复并发分片失败时文件 report 的 request_id 被成功分片覆盖，以及纯本地复制误记录 CLI 关联 ID。
- 修复批量执行中途列举、同步删除计划、direct 删除或上传检查点扫描失败时缺少已有结果报告／摘要的问题。
- 修复 rm --include-uploads 的计划计数、清单及 all-versions 分支漏执行终止上传任务；HNS direct 删除补齐目录计划清单。

- tos、ve-tos、ve-adrive 各格式命令级 `service_request_ids` 统一保留前 100 个，超出数量记录在 `service_request_ids_omitted`。
- 执行结果 report CSV 尾部新增 `request_id` 列，成功、失败项有服务 ID 均逐项记录，没有则留空；文件明细不受命令级 ID 上限影响。
- 修复 ByteTOS V1 中文及特殊字符路径的签名错误；签名使用原始路径，HTTP URL 保持一次编码，PSM 转发保留完整编码路径。
- 修复 tos、ve-tos、ve-adrive 批量操作的 table/CSV 被请求 ID 列表替代，恢复摘要、计划清单路径和报告路径。
- 修复 ve-tos 独立入口 MCP/SSE 子进程重复传入 `ve-tos`，导致工具执行参数解析失败的问题；统一入口保持原有参数格式。
- 修复 tos、ve-tos、ve-adrive 本地目录递归上传遗漏空目录的问题；cp 和 sync 默认保留目录，新增 `--disable-dir-object` 关闭显式目录上传，并同步帮助与发现信息。
- 修复 ve-adrive 独立入口 MCP/SSE 子进程重复传入 `ve-adrive`，导致工具执行参数解析失败的问题。

## 版本号：v1.0.0 日期：2026-06-16

### 变更内容

- 第一次正式发布
