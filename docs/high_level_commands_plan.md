<!--
Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
-->

# High-Level Commands 实现计划

本文档记录 `tos` High-Level Commands 的命令骨架、参数设计、能力边界和实现计划。所有命令都必须满足 [API 实现六项原则](api_implementation_principles.md)。

## 总体设计

High-Level Commands 不重复实现 TOS API 协议映射，而是在用户任务层编排 Low-Level Bucket/Object/Multipart 能力。

- 输入路径统一支持本地路径和 `tos://bucket/key`。
- 对象操作复用 Object Core API：`PutObject`、`GetObject`、`CopyObject`、`DeleteObject`、`DeleteMultiObjects`、`ListObjectsV2`、`HeadObject`、`RestoreObject`。
- Bucket 操作复用 Bucket Core API：`CreateBucket`、`DeleteBucket`、`ListBuckets`、`HeadBucket`、`GetBucketLocation`。
- 大文件上传已复用 `tos-core::transfer` 策略并接入 multipart、checkpoint 与 checkpoint lock；上传/下载/`cat` 均使用流式 I/O，下载路径先写同目录临时文件，非 `--force` 场景通过原子 hard-link 防覆盖持久化，`--force` 场景才替换目标。
- 所有危险操作必须支持 `--dry-run` 计划；删除、覆盖、移动删除源、同步删除多余目标等场景必须要求 `--force`。
- 批量成功/失败清单和断点续传目录都必须支持 CLI 参数覆盖，并在 `[profile.tos]` 中提供默认值。

## 批量操作模型

High-Level Commands 必须支持批量操作，但批量语义只在明确的命令和参数下启用，避免用户误操作。

### 支持批量的命令

| 命令 | 批量入口 | 批量来源 | 说明 |
|---|---|---|---|
| `cp` | `--recursive` | 本地目录扫描或 `ListObjectsV2` 前缀扫描 | 批量上传、下载、TOS 内复制。 |
| `mv` | `--recursive` | 复用 `cp --recursive` 的计划 | 复制成功后再删除源，删除源必须受 `--force` 保护。 |
| `sync` | 默认批量 | 本地目录 manifest + TOS 前缀 manifest | 增量同步天然是批量操作。 |
| `rm` | `--recursive` | 本地目录扫描或 `ListObjectsV2` 前缀扫描 | 删除前缀或目录时必须显式 `--recursive` 和 `--force`。 |
| `rb` | 无批量入口 | `DeleteBucket` | 只删除 bucket 本身；如需清空内容，应组合 `rm --recursive` 后再 `rb`。 |
| `restore` | `--recursive` 或 `--manifest` | `ListObjectsV2` 前缀扫描或 manifest 文件 | 仅 `ve-tos` 暴露；批量解冻归档对象，必须记录每个对象的成功/失败清单。 |
| `ls` | 默认分页 | `ListBuckets` 或 `ListObjectsV2` | 输出 bucket/object 列表；仅显式传 `--manifest-path` 时写 manifest，不写 report。 |
| `du` | 默认批量 | `ListObjectsV2` | 聚合 size/count；可写 manifest，不写 report。 |
| `find` | 默认批量 | `ListObjectsV2` | 客户端过滤对象清单；可写 manifest，不写 report。 |

`cat`、`presign`、`stat` 默认是单对象或单资源命令，不在第一阶段支持多目标批量输入。后续如果需要批量版本，应新增 `--manifest` 或 `--from-file`，不要隐式把普通参数解释成多目标。

### 路径配置

`[profile.tos]` 必须写入以下默认配置，CLI 参数优先级高于 config：

```toml
[default.tos]
checkpoint_dir = "~/.tos/checkpoints"
batch_report_dir = "~/.tos/reports"
batch_report_format = "csv"
batch_concurrency = 16
list_concurrency = 4
progress_enabled = true
```

命令级覆盖规则：

- `--checkpoint-dir`：覆盖 checkpoint 目录，适用于 `cp`、`sync`、`mv`。
- `--report-path`：覆盖本次批量成功/失败清单输出基准路径，适用于 `cp`、`mv`、`sync`、`rm`；`restore` 仅适用于 `ve-tos`。
- `--manifest-path`：覆盖本次主动 list 生成的清单基准路径；`cp`、`mv`、`sync`、`rm` 批量模式默认生成，可用 `--no-manifest` 关闭；`ls`、`du`、`find` 仅显式传入时生成；`restore` 仅适用于 `ve-tos`。
- `--batch-concurrency`：覆盖批量执行阶段并发数，适用于 `cp`、`mv`、`sync`、`rm`；`restore` 仅适用于 `ve-tos`，默认 `16`。
- `--list-concurrency`：覆盖 recursive list 中按 `delimiter="/"` 扫描 prefix 的并发数，默认 `4`；仅 `ve-tos` 平铺 `delimiter=""` 场景不能并发。
- `--recursive-list-mode auto|flat|hierarchical`：`tos` surface 的递归枚举固定使用 `delimiter="/"` 并递归 common prefixes；`ve-tos` 的 `auto` 下 HNS 桶使用 `delimiter="/"`，FNS 桶保持 `delimiter=""`，可显式选择 `flat` 或 `hierarchical`。
- `--storage-class`：仅 `ve-tos` 暴露；ByteTOS `tos` 不提供对象写入或筛选的 storage class 参数，并会拒绝旧调用中传入的 `--storage-class`。
- 未指定 `--report-path` 或 `--manifest-path` 时，默认写入 `batch_report_dir` 下自动生成的 `.csv` 基准路径。
- `batch_report_format` 只支持 `csv`。实际落盘按大小滚动，文件名为 `<基准文件名>.part-0001.csv`、`<基准文件名>.part-0002.csv` 等，默认单分片上限 50MiB。

### 进度回调

批量操作和传输类 High-Level Commands 必须支持进度回调能力，默认启用，并允许用户显式关闭。

- 配置默认值：`[profile.tos].progress_enabled = true`，也可通过环境变量 `TOS_PROGRESS_ENABLED=false` 关闭。
- CLI 关闭参数：批量或传输命令提供 `--no-progress`，优先级高于 config。
- 输出隔离：实时进度只写入 `stderr`，不得污染 `--output json` 的 `stdout`；`--quiet` 必须关闭实时进度。
- dry-run 行为：`--dry-run` 不启动实时进度，只在计划中输出 `progress.enabled`、`render_to` 和 `disabled_reason`。
- Agent 兼容：当前真实执行已统一遵循 `progress_enabled`、`--no-progress`、`--quiet`；后续可继续增强进度事件中的 `run_id`、`item_id`、`bytes_done`、`bytes_total`、`objects_done`、`objects_total`。

### 批量执行计划

批量命令分三步执行：

1. `Discover`：扫描本地目录或分页调用 `ListObjectsV2`，生成候选项。
2. `Plan`：应用 `--include`、`--exclude`、`--size-only`、`--exact-timestamps`、`--delete` 等规则，生成确定性任务列表。
3. `Execute`：按任务类型执行上传、下载、复制、删除、跳过，并记录每一项结果。

`--dry-run` 只执行前两步，必须输出计划摘要和样例任务，不发真实变更请求。

Discover 阶段的远端递归 list 规则：

- ADrive 与 `tos` surface 默认按目录层级 `delimiter="/"` 扫描，按 prefix 并发，默认并发 `4`。
- `ve-tos` 的 HNS 桶默认使用 `delimiter="/"`；`ve-tos` 的 FNS 桶默认保持平铺 `delimiter=""` 语义，显式指定 `--recursive-list-mode hierarchical` 后改用 `delimiter="/"` 并启用 prefix 并发。
- `ve-tos` 平铺 `delimiter=""` 场景只沿 continuation token 串行分页，不做无效并发。

Execute 阶段的批量任务默认并发 `16`。`sync --delete`、`rm --recursive`、`mv --recursive` 在 `tos` surface 和 `ve-tos` FNS 场景按 planned object delete 并发执行；`ve-tos` HNS 桶和 ADrive 的分层删除场景走 bottom-up 时，先并发删除所有叶子项，再按目录深度逐层并发删除空目录，避免父目录早于子项删除。

### 成功和失败清单

批量命令的结构化输出必须包含统计和明细，`--output json` 推荐 schema：

```json
{
  "status": "success",
  "command": "tos cp",
  "data": {
    "summary": {
      "planned": 3,
      "succeeded": 2,
      "failed": 1,
      "skipped": 0,
      "bytes_transferred": 1048576
    },
    "succeeded": [
      {
        "source": "./a.txt",
        "destination": "tos://bucket/a.txt",
        "operation": "upload",
        "bytes": 1024,
        "etag": "\"etag\""
      }
    ],
    "failed": [
      {
        "source": "./b.txt",
        "destination": "tos://bucket/b.txt",
        "operation": "upload",
        "error_kind": "validation_error",
        "error_code": "LocalFileNotFound",
        "message": "source file does not exist"
      }
    ],
    "skipped": []
  }
}
```

批量命令的退出码规则：

- 全部成功或全部跳过：退出码 `0`。
- 部分失败：退出码使用 `TransferFailed`，但仍输出成功和失败清单，便于 Agent 续跑或人工排查。
- 计划阶段失败：不执行任何任务，返回确定性 `ValidationError`。
- 执行阶段失败：不中断整个批次，除非失败是认证、权限、配置缺失等不可恢复错误。

### 任务记录和续跑

批量命令应在内存中维护 `BatchReport`，并在配置或参数指定的 report 路径中以 CSV 分片持久化：

- `operation`：`upload`、`download`、`copy`、`delete`、`skip`。
- `source` / `destination`：原始用户路径，敏感信息不得写入。
- `bucket` / `key`：规范化 TOS 目标。
- `bytes`、`etag`、`version_id`、`request_id`：成功项可观测字段。
- `error_kind`、`error_code`、`message`：失败项确定性错误字段。
- `retryable`：标记是否可直接重试。

## 命令计划

| 命令 | 参数 | 提供能力 | 计划实现 |
|---|---|---|---|
| `cp` | `source`、`destination`、`--recursive`、`--include-parent`、`--include`、`--exclude`、`--checkpoint`、`--checkpoint-dir`、`--batch-concurrency`、`--list-concurrency`、`--recursive-list-mode`、`--report-path`、`--bandwidth-limit`、`--no-progress`、`--force` | 本地上传到 TOS、TOS 下载到本地、TOS 内复制；支持本地目录和 TOS 前缀递归复制。 | 解析 `source/destination` 为 Local 或 TOS；local->tos 调 `PutObject`/Multipart，tos->local 调 `GetObject`，tos->tos 调 `CopyObject`；递归时本地扫描或 `ListObjectsV2` 后批量编排。 |
| `mv` | `source`、`destination`、`--recursive`、`--include-parent`、`--include`、`--exclude`、`--checkpoint-dir`、`--batch-concurrency`、`--list-concurrency`、`--recursive-list-mode`、`--report-path`、`--no-progress`、`--force` | 移动文件/对象，语义为 copy 成功后删除源。 | 复用 `cp` 的复制计划；只有复制成功后才删除源；递归移动按 include/exclude 删除已复制源项，保留排除项；缺少 `--force` 时禁止删除源。 |
| `sync` | `source`、`destination`、`--delete`、`--force`、`--size-only`、`--exact-timestamps`、`--include-parent`、`--include`、`--exclude`、`--checkpoint-dir`、`--batch-concurrency`、`--list-concurrency`、`--recursive-list-mode`、`--report-path`、`--bandwidth-limit`、`--no-progress` | 本地目录与 TOS 前缀、TOS 前缀之间增量同步。 | 两侧生成候选项；按过滤器、size-only 或 exact-timestamps 计算 copy/update/skip/delete plan；`--delete` 删除目标多余对象时要求 `--force` 且只作用于 include/exclude 管理范围。 |
| `mb` | `bucket`、`--region`、`--storage-class`、`--acl`、`--az-redundancy`、`--bucket-object-lock-enabled` | 创建 bucket 的高阶快捷入口。 | 解析 bucket 或 `tos://bucket`；复用 Bucket Core `CreateBucket` 参数构造；`--describe` 暴露映射 API 与 header 参数；dry-run 输出目标 region、storage class、ACL。 |
| `rb` | `bucket`、`--force`、`--destroy`、`--no-progress` | 删除 bucket 的高阶快捷入口。 | 只删除 bucket 本身，映射 `DeleteBucket`；不负责清理 bucket 内对象；非交互环境必须显式确认。 |
| `rm` | `path`、`--recursive`、`--recursive-delete-mode`、`--batch-concurrency`、`--list-concurrency`、`--recursive-list-mode`、`--force`、`--report-path`、`--include`、`--exclude`、`--no-progress` | 删除单对象、对象前缀或本地路径；TOS 前缀删除需要递归确认。 | TOS 单对象走 `DeleteObject`；TOS 前缀必须带 `--recursive` 并先 `ListObjectsV2`；批量删除走 `DeleteMultiObjects`；本地删除使用 filesystem；危险场景要求 `--force`。 |
| `ls` | `path`、`--recursive`、`--human-readable`、`--sort`、`--manifest-path` | 列举 bucket 或对象前缀。 | 无 path 时映射 `ListBuckets`；`tos://bucket[/prefix]` 映射 `ListObjectsV2`；支持 prefix、delimiter、分页聚合；输出支持 table/json；仅显式传入时写 manifest。 |
| `stat` | `path`、`--version-id` | 查看 bucket 或 object 元信息。 | `tos://bucket` 映射 `HeadBucket`、`GetBucketLocation` 或 bucket info；`tos://bucket/key` 映射 `HeadObject`；输出标准化 metadata、size、etag、storage_class、last_modified。 |
| `du` | `path`、`--human-readable`、`--max-depth`、`--list-concurrency`、`--manifest-path`、`--no-progress` | 统计 bucket/prefix 容量和对象数量。 | `ListObjectsV2` 分页聚合 size/count；按 `--max-depth` 聚合目录层级；dry-run 输出扫描范围和预计 API 类型；可显式写 manifest。 |
| `find` | `path`、`--name`、`--size`、`--mtime`、`--manifest-path`、`--no-progress`；`--storage-class` 仅 `ve-tos` | 按条件筛选对象。 | `ListObjectsV2` 分页扫描，客户端过滤 glob/name、大小表达式、mtime 表达式；`ve-tos` 额外支持 storage class；输出匹配对象列表；可显式写 manifest。 |
| `cat` | `path`、`--range`、`--version-id` | 输出对象内容到 stdout，支持范围读取。 | 映射 `GetObject`；`--range` 转换为 Range header；默认只适合文本/小对象，后续可加二进制保护或 `--output-file`。 |
| `presign` | `path`、`--expires`、`--method` | 生成对象预签名 URL。 | 在 `tos-core::infra` 增加 presign 能力，按当前 surface 的签名算法分派（`tos` 使用 ByteTOS V1，`ve-tos` 使用 TOS4）；限制 method 白名单；输出 URL、expires_at、method；敏感信息不落日志。 |
| `restore` | `path`、`--recursive`、`--manifest`、`--include`、`--exclude`、`--days`、`--tier`、`--version-id`、`--batch-concurrency`、`--list-concurrency`、`--recursive-list-mode`、`--report-path`、`--force`、`--no-progress` | 仅 `ve-tos` 暴露，用于解冻单个或批量归档对象。 | 单对象映射 Low-Level `object restore`；批量模式通过 prefix scan 或 manifest 生成任务；dry-run 输出恢复天数、tier、影响对象数和计费风险提示；批量执行必须输出成功/失败清单。 |

## 上传下载数据一致性

`cp`、`sync`、`mv` 涉及本地文件和对象读写，必须同时利用本地文件系统原子操作和 TOS 原生一致性能力。核心原则是：读使用条件请求固定对象版本，写流式计算 CRC64 并与 TOS 服务端返回的 CRC64、ETag 或 VersionId 对比确认，不做写后的二次 `HeadObject` 或本地重读校验。

### Local -> TOS

- 上传前读取本地 metadata：size、mtime、可选 hash。
- 打开文件后再次读取 metadata；如果 size/mtime 与计划阶段不一致，返回确定性 `ValidationError`，避免上传半新半旧内容。
- 上传读取本地文件流时同步计算 CRC64；不能为了校验再额外读取第二遍本地文件。
- 简单上传应尽量携带 `Content-MD5` 或 SDK 支持的校验头，由 TOS 在服务端校验 payload。
- 如果目标不存在语义是必须的，应使用条件写入能力，例如 `If-None-Match: *`；如果是覆盖已知版本，应使用 `If-Match: <etag>` 或等价条件头。
- 简单上传完成后记录 TOS 返回的 `crc64`、`etag`、`version_id` 和 `request_id`；本地流式 CRC64 必须与服务端返回的 CRC64 对比，写一致性以对比结果为准。
- Multipart 上传每个 `UploadPart` 都必须在读取 part 流时计算 part CRC64，并与服务端 `UploadPart` 返回的 CRC64 对比；同时记录每个 part 的 `etag`。
- Multipart Complete 前按 part number 排序并校验 part 列表连续。
- Multipart Complete 后必须记录 TOS 返回的最终对象 `crc64`、`etag` 或 `version_id`；当前实现会在 Complete 响应返回最终 CRC64 时与上传前流式计算的本地对象 CRC64 对比，不只依赖 HTTP 2xx，也不做 Complete 后二次 `HeadObject`。
- 如果启用 `--checkpoint`，checkpoint 中必须包含 file_size、mtime、part_size、upload_id、completed_parts；恢复时全部校验一致才允许续传。

### TOS -> Local

- 下载必须先写入同目录临时文件，例如 `<target>.tos-partial-<pid>`（实现使用 `std::process::id()` 作为后缀，保证不同进程不会写同一个临时文件）；未指定 `--force` 时通过 hard-link 原子创建最终路径以避免覆盖并发创建的文件，指定 `--force` 时才替换目标路径。
- 如果目标文件已存在且未指定 `--force`，必须拒绝覆盖。
- 下载前通过计划阶段的 list/head 结果获取 `etag` 或使用用户指定的 `version_id`；`GetObject` 必须携带 `If-Match: <etag>` 或固定 `version_id`，避免下载过程中对象被覆盖导致内容漂移。
- 下载后只校验本次响应的 `Content-Length` 与实际写入字节数，确认流完整写入临时文件；不重新读取临时文件做内容校验。
- 支持 Range 或分段下载时，每段读取都必须基于同一个 etag/version_id；所有 range 都完整写入并校验后才能 rename。
- rename 前再次检查目标路径；如果目标已被其他进程创建且未指定 `--force`，必须保留临时文件并返回确定性冲突错误。

### TOS -> TOS

- 单对象复制使用 TOS 服务端 `CopyObject`，避免数据经本地落盘。
- 如果源对象在计划阶段有 etag/version_id，CopyObject 必须使用源条件头，例如 `x-tos-copy-source-if-match` 或等价能力；其语义等同于 `GetObject` 的 `If-Match`，用于确保复制的是计划中的对象版本。
- 目标不存在语义使用目标条件写入能力；覆盖已知目标时使用目标 `If-Match` 或等价条件头。
- 复制完成后记录 TOS 返回的 `crc64`、`etag`、`version_id` 或 copy result，不额外二次 `HeadObject`。
- 跨桶/大对象 Multipart Copy 必须记录每个 copy part 的 etag，Complete 后记录服务端返回的最终校验信息。
- `mv` 只有在 copy 成功且服务端返回校验信息后才能删除源对象；如果 bucket 开启版本控制，应删除计划中记录的 source version_id；未开启版本控制时，删除必须带源对象条件约束，避免删除已被其他进程改写的对象。
- 批量 `mv` 对每个对象独立执行 copy -> verify -> delete；部分失败不能删除未验证成功的源。

### sync 一致性

- sync 先生成 source manifest 和 destination manifest，再做 diff，manifest 中必须包含 size、mtime、etag、version_id 或 crc64，避免边扫描边删除；当前 TOS 方向通过 ListObjects/HeadObject 生成 manifest entry，按 size/etag 判断 skip/copy/delete。
- `--delete` 只删除 diff 中明确判定为目标多余的对象；执行前 dry-run plan 必须列出删除数量和样例。
- 如果同步过程中 source manifest 对应文件/对象发生变化，应跳过该项并记录为 failed 或 retryable，不得上传不一致内容。
- 对 TOS 目标的删除必须使用计划阶段记录的 etag/version_id 做条件约束；条件不满足时记录冲突失败，不应盲目删除。当前 extra 删除和 mv 源删除已携带 ETag 条件，目标覆盖条件写入仍作为后续增强。
- 部分失败时不能回滚已成功项，但必须输出成功/失败清单和可重试标记。

## `cp` 大文件和断点续传

`cp` 必须支持大文件，上传方向的第一阶段实现重点是 local -> TOS。当前 `tos-core::transfer::UploadStrategy` 已具备按文件大小选择 `Simple` 或 `Multipart` 的策略骨架：

- `<= 5GiB`：使用 `PutObject` 简单上传。
- `> 5GiB`：使用 Multipart 上传，按文件大小选择 part size。
- 1GiB 以下候选 part size：20MiB。
- 1GiB 到 10GiB 候选 part size：50MiB。
- 10GiB 以上候选 part size：100MiB。

### Multipart 上传流程

大文件上传按以下步骤实现：

1. `CreateMultipartUpload`：获取 `upload_id`。
2. 切分本地文件：按 part size 读取固定范围，part number 从 1 开始。
3. `UploadPart`：逐分片上传，读取 part 流时计算 CRC64，并与服务端返回的 part CRC64 对比；记录每个 part 的 `etag` 和 `crc64`。
4. `CompleteMultipartUpload`：提交已完成 part 列表。
5. 失败清理：如果没有启用 checkpoint 且不可恢复失败，应提示用户可执行 abort；后续可自动调用 `AbortMultipartUpload`。

### 断点续传

`cp --checkpoint` 必须支持断点续传。checkpoint 的身份由稳定任务指纹决定，而不是由单次运行的 `run_id` 决定；这样同一个 CLI 任务第一次中断后，第二次运行可以命中同一个 checkpoint 并续传。当前 `tos-core::transfer::Checkpoint` 已包含：

- `bucket`
- `key`
- `source_path`
- `file_size`
- `part_size`
- `upload_id`
- `completed_parts`

实现要求：

- checkpoint 目录来自 `--checkpoint-dir` 或 `[profile.tos].checkpoint_dir`。
- 每次 CLI 执行生成唯一 `run_id`，只用于本次进程的 lock ownership、临时文件、batch report 和日志关联；`run_id` 不参与 checkpoint 文件名，也不能影响是否命中旧 checkpoint。
- checkpoint 文件名由稳定任务指纹 hash 生成，当前指纹包含 `source_path + bucket + key + file_size + mtime + part_size + profile + endpoint`，同一任务跨两次 CLI 运行必须得到同一个 checkpoint 文件。
- checkpoint 内容必须包含 `schema_version`、`checkpoint_id`、`created_at`、`updated_at`、`last_run_id`、`source_fingerprint`、`target_fingerprint`、`upload_id`、`part_size`、`completed_parts`、`object_lock` 信息。
- 同一 checkpoint 必须配套 lock 文件，例如 `<checkpoint>.lock`；创建 lock 使用原子 `create_new` 或 `flock`，获取失败时返回确定性 `CheckpointLocked`，不得两个进程同时写同一 checkpoint。
- lock 文件包含 `run_id`、`pid`、`hostname`、`started_at` 和 heartbeat；如果同机进程已退出或 lock 超过安全 TTL，可以接管并续传；如果无法判断持有者是否存活，返回 `CheckpointLocked`，可由用户显式 `--force` 接管。
- 上传前如果 checkpoint 存在，必须校验本地文件 size/mtime/inode 或平台可用 file id、目标 bucket/key、part size、endpoint、upload_id 是否一致。
- 续传前应调用 `ListParts` 或等价能力核对服务端已存在 part；本地 checkpoint 与服务端不一致时，以服务端 part 列表为准并更新 checkpoint。
- 已完成 part 不重复上传；只上传缺失 part。
- 每个 part 上传成功后立即写 `<checkpoint>.tmp` 并 `fsync` 后原子 rename 到 checkpoint，避免进程中断导致记录损坏。
- `CompleteMultipartUpload` 成功后删除 checkpoint 和 lock；如果删除失败，后续启动应通过对象已完成状态安全清理。
- 如果 upload_id 已失效，应返回确定性 `CheckpointInvalid`，并提示删除 checkpoint 或重新开始。

### 多 CLI 进程隔离

多个 CLI 进程即使使用同一个 `checkpoint_dir`，也必须保证 checkpoint、临时文件、报告文件和目标对象写入不会串任务，同时允许同一个任务在前一次退出后第二次运行继续使用 checkpoint。

- 每个命令执行生成唯一 `run_id`，格式建议为 `timestamp + pid + random`。
- checkpoint 文件按稳定任务指纹命名；同一任务的两次运行共用同一个 checkpoint，不同任务必须落到不同 checkpoint。
- 同一 checkpoint 同一时刻只能有一个进程持有 lock；另一个并发进程发现 lock 存活时直接返回 `CheckpointLocked`，不会读写该 checkpoint。
- 前一次运行正常退出或失败退出时释放 lock，保留 checkpoint；第二次运行重新获取 lock 后读取 checkpoint 并续传。
- 前一次运行崩溃时可能留下 lock；第二次运行先检查 lock owner 是否仍存活，同机确认死亡或 heartbeat 超时后可接管，无法确认时要求 `--force`。
- batch report 默认文件名包含 `command + run_id`，避免多个进程写同一个 report；用户显式指定同一个 `--report-path` 时，实际分片文件使用 `<基准文件名>.part-NNNN.csv`，后续如需并发写保护应加文件锁或直接返回 `ReportLocked`。
- 本地下载临时文件包含进程标识，例如 `<target>.tos-partial-<pid>`（实现以 `std::process::id()` 作为后缀），不同进程不会写同一个临时文件。
- 本地最终 rename 前必须检测目标路径状态；目标被其他进程改变时返回冲突错误，除非用户显式 `--force` 且计划允许覆盖。
- 远端目标写入必须尽量使用 TOS 条件请求；如果条件不满足，说明其他进程或用户已修改目标，必须记录为冲突失败。
- 批量命令的每个 item 都有独立 `item_id`，报告清单、checkpoint part 和日志都用 `run_id + item_id` 关联，避免并发日志和结果清单混淆。

### 下载断点续传

下载方向可以分两阶段实现：

- 第一阶段：支持普通 `GetObject` 和 `Range` 下载，但不承诺 checkpoint resume。
- 第二阶段：基于 `DownloadStrategy::Ranged` 增加 ranged multipart download checkpoint，记录已完成 byte range、目标临时文件和 etag 校验信息。

### TOS 内复制大对象

TOS -> TOS 复制优先使用 `CopyObject`。如果服务端限制单次 Copy 的对象大小，后续需要扩展为 Multipart Copy：

1. `CreateMultipartUpload`。
2. 按 range 调 `UploadPartCopy`。
3. `CompleteMultipartUpload`。
4. checkpoint 记录 `upload_id` 和已完成 copy part。

## 测试计划

- `Discovery`：`tos --help` 包含所有 High-Level 命令；每个命令支持 `--help`。
- `Understanding`：后续实现 handler 后，每个命令支持 `--describe`，展示底层 API 编排和参数语义。
- `Safe Execution`：`rm --recursive`、`sync --delete`、`mv`、`rb --force/--destroy` 覆盖 dry-run 和 `--force` 保护。
- `Controlled Output`：dry-run 和真实结果支持 `--output json`，包含 plan 和统计字段。
- `Deterministic Errors`：URI 错误、缺少 `--recursive`、缺少 `--force`、非法 size/mtime/method 均返回稳定 `ValidationError`。
- `Agent Ecosystem`：新增 `tests/high_level_principles_test.rs`，覆盖 discover、understand、safe execute、controlled output、deterministic error。
- `Batch Report`：批量命令必须测试部分成功/部分失败输出中同时包含 `succeeded` 和 `failed` 清单。
- `Large File`：`cp` 必须测试 strategy 选择、checkpoint 读写、跳过已完成 part、complete 后清理 checkpoint。
