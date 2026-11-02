# ADrive 分享验证步骤

适用于当前源码。以下统一使用独立入口；统一入口对应为 `ve-storage-uni-cli ve-adrive`。管理命令使用已配置业务 endpoint 并登录的 OAuth Profile，接收命令使用分享 URL。

## 1. 构建与本地自动化

在仓库根目录执行：

```bash
cargo build --locked --offline
cargo build --manifest-path packaging/cargo/ve-adrive-cli/Cargo.toml --locked --offline
cargo test -p ve-adrive-cli-core --locked --offline -- --test-threads=1
SHARE_UNIFIED="$PWD/target/debug/ve-storage-uni-cli"
SHARE_CLI="$PWD/packaging/cargo/ve-adrive-cli/target/debug/ve-adrive-cli"
for SHARE_TEST in tests/share_*_regression.py; do
  if [ "$SHARE_TEST" != tests/share_pty_regression.py ]; then
    python3 "$SHARE_TEST" "$SHARE_UNIFIED" "$SHARE_CLI" || exit 1
  fi
done
# PTY 回归依赖 macOS/Linux，每次只接收一个二进制。
if [ "$(uname -s)" = Darwin ] || [ "$(uname -s)" = Linux ]; then
  python3 tests/share_pty_regression.py "$SHARE_UNIFIED" || exit 1
  python3 tests/share_pty_regression.py "$SHARE_CLI" || exit 1
fi
```

预期：Rust 测试通过；Python 报告两个入口 PASS。Python 只访问本地 HTTP 夹具，不访问云端，覆盖完整只读规划、分页、恢复本地摘要、显式空密码、错误聚合、SIGINT、选择器保护、403、目录别名恢复及无效下载地址。MCP 回归通过实际 stdio RPC 核对下载工具的 readOnlyHint=false、默认离线 argv 规划，以及 execute=true/dry_run=true 的联网只读预演。

## 2. 文档与能力发现

```bash
"$SHARE_CLI" share --help --language zh
"$SHARE_CLI" share create --help --language zh
"$SHARE_CLI" share create --describe --language zh --output json
"$SHARE_CLI" share files --help --language zh
"$SHARE_CLI" share files --describe --language zh --output json
"$SHARE_CLI" share list --help --language zh
"$SHARE_CLI" share list --describe --language zh --output json
"$SHARE_CLI" share download --help --language zh
"$SHARE_CLI" share download --describe --language zh --output json
"$SHARE_CLI" share view --help --language zh
"$SHARE_CLI" share view --describe --language zh --output json
"$SHARE_CLI" capabilities --view full --output json --query "data.capabilities[?group=='share']"
"$SHARE_CLI" skill list --output json --query "data.skills[?starts_with(name, 've_adrive_share_')]"
SHARE_DOCS="$(mktemp -d)"
for SHARE_ACTION in create files download; do
  "$SHARE_CLI" skill export --name "ve_adrive_share_$SHARE_ACTION" --language zh --dir "$SHARE_DOCS"
done
"$SHARE_CLI" completion bash --output json
```

预期：发现 9 个命令 `create/get/list/update/cancel/inspect/files/view/download`，对应 9 个 Skill。 create 明确 Space 根 URI（可带尾斜杠）与 flags 对应 `ShareAllFiles=true`、`FilePathList=[]`；management 输出 schema 将 `ShareAllFiles` 标为可选 boolean，无默认 false。files/download 说明省略 path 或 `--path /` 的整 Space 选择。下载保留完整空间相对层级，根没有 basename，include-parent 不增加包装；选中子目录仍按 cp 布局。上述三项核对 en/zh；机器字段、参数类型与 9 个命令名称保持一致。MCP 描述说明整 Space 语义，未新增 scope flag。Describe 包含参数类型、风险、API 映射、输出结构和行为说明；list 暴露可选位置参数 `target` 和条件必填的 `instance`，说明实例 URI 与 flag 两种写法。下载 Describe 保留真实执行 output_schema，另加 dry_run_output_schema；side_effects 说明实际下载会创建目录、临时文件、checkpoint/锁文件、默认 force 覆盖及可选跳过策略，以及签发 URL 可能消耗下载次数。预演仅访问元数据且不写本地。导出的下载 Skill 包含相同中文规划、域名、根目录与未知结果说明。补全包含分享命令。MCP 工具由同一注册表生成，名称为 `ve_adrive_share_*`；工具调用默认生成计划，`execute=true` 才执行，`describe=true` 可省略实例选择器。

## 3. 云端准备

将下列 Instance、Space 和 Profile 替换为自己的测试配置；先确认该 Profile 的业务 endpoint 和 OAuth 会话绑定对应实例。

```bash
SHARE_INSTANCE=your-instance-id
SHARE_SPACE=your-space-id
SHARE_PROFILE=default
share_manage() {
  "$SHARE_CLI" --profile "$SHARE_PROFILE" --auth-mode oauth \
    --output json "$@"
}
share_manage auth status
share_manage ls "adrive://$SHARE_INSTANCE/$SHARE_SPACE/"
```

预期：OAuth `ready=true`，空间列表成功。如果不成功，先处理鉴权/实例绑定问题再进行后续写操作。本例直接使用已有 API 配置，不传 `--endpoint` 或 `--share-endpoint`。管理侧默认读取服务端精确字段 `ShareURL`，校验 URL 与 ShareID 后输出稳定字段 `ShareUrl`；不再按 API endpoint 或 Instance 猜测分享页面域名。

配置覆盖值时仍按 `--share-endpoint` > 所选 profile 的 `share_endpoint` > `ADRIVE_SHARE_ENDPOINT` > 旧环境变量 `ADRIVE_SHARE_BASE_URL` 的优先级拼接。例如 `--share-endpoint http://pages.example.test` 和 ShareID `2T4U8z0lY1n` 得到 `http://pages.example.test/share/2T4U8z0lY1n`，不添加 Instance 或密码。公开配置名为 `share_endpoint`；旧 CLI/TOML/环境变量名称仅兼容读取。接收侧标准 IDS 页面推导 API 的规则仍保留协议、端口及实例前缀，自定义接收 API 可单次传 `--endpoint`。

创建本轮专用文件夹和一个小文件，保留空目录：

```bash
SHARE_LOCAL="$(mktemp -d)"
SHARE_FOLDER="share-manual-$(python3 -c 'import uuid; print(uuid.uuid4().hex[:12])')"
SHARE_REMOTE="adrive://$SHARE_INSTANCE/$SHARE_SPACE/$SHARE_FOLDER/"
printf 'share test\n' > "$SHARE_LOCAL/report.txt"
share_manage mkdir "${SHARE_REMOTE}empty/" --parents
share_manage cp "$SHARE_LOCAL/report.txt" "${SHARE_REMOTE}report.txt"
SHARE_CREATED="$(share_manage share create "$SHARE_REMOTE" --name "$SHARE_FOLDER" --permission download --expires-in 1h --require-login false)"
printf '%s\n' "$SHARE_CREATED"
```

确认创建成功后提取响应字段（只在本地变量中保存）：

```bash
SHARE_ID="$(printf '%s' "$SHARE_CREATED" | python3 -c 'import json,sys; print(json.load(sys.stdin)["data"]["ShareID"])')"
SHARE_URL="$(printf '%s' "$SHARE_CREATED" | python3 -c 'import json,sys; print(json.load(sys.stdin)["data"].get("ShareUrl", ""))')"
```

预期：返回服务端 `ShareURL`，或显式 `share_endpoint` 覆盖值加 `/share/<编码后的 ShareID>`。未配置覆盖值且服务端 URL 缺失时，操作仍成功并保留 `ShareID`，省略 `ShareUrl`，返回可选 `ShareUrlWarning` 提示；本地 `SHARE_URL` 因而为空。此时保留 ShareID，配置 `share_endpoint` 后用 `share get` 获取既有分享，或等待服务端补齐 URL；不要重复 create。仅在 `SHARE_URL` 非空后继续接收测试。

create/get 可保留服务端 URL 中已编码的 `pwd` 查询参数供授权交付；完整 URL 与 `SharePwd` 都视为秘密，不写入日志或保存的计划。SearchShareLinks 不返回密码查询参数；CLI 的 list/update 仍移除全部 URL query 和密码字段，即使 UpdateShareLink 响应带 `pwd`。覆盖值生成的 URL 不添加密码。诊断不能出现账户 token 或原始响应 URL。

不附加文件名或目录路径；接收侧标准 IDS 页面可推导 API，自定义页面需显式传 `--endpoint`。例如 `instance.adriveapp.volces.com` 当前不会自动改写为 IDS API。网页是否重定向由服务端决定。复测至少覆盖：无覆盖时采用服务 URL（含编码的 pwd）、覆盖优先级、list/update 去 query、服务 URL 缺失保留成功 ShareID，以及诊断脱敏。

默认权限按 PRD 为仅预览，对应 V1 的 `view`。本例会下载目录，因此创建时显式传 `--permission download`。更新时未传 permission 保留原权限。

密码是服务端提取码：create/update 的 `--password-stdin` 把输入映射为 V1 `SharePwd`；分享者将链接和提取码交给接收者。接收者在 files/view/download 中使用同一参数输入该提取码，CLI 调用 `GetShareAccessToken`，由服务端校验后返回分享访问 token。inspect 只读取公开概况，不需要密码。显式输入移除末尾一个 LF/CRLF 后为空时校验失败（退出码 6），覆盖 EOF/LF/CRLF 三种空输入；空格原样保留。create/update 的 --dry-run 也读取并校验显式输入，省略该参数则 update 保留原密码。--help/--describe 不读取 stdin、配置或网络。PRD 尚未确认清除提取码语义，空输入不会清除密码。

`--require-login` 映射为 V1 `RequireLogin`，控制接收者是否必须登录，不改变分享者创建和管理时的身份要求。false 允许按密码、权限等规则匿名接收；true 要求接收者登录。接收默认匿名，即使 profile 已配置 OAuth；默认 files/view/download 会拒绝 RequireLogin=true 的分享，保留服务端拒绝码（实测 Forbidden.ShareRequireLogin）；如果服务端允许读取概况，CLI 则返回 `share_login_required`。显式 `--auth-mode oauth` 在 inspect/files/view/download 中复用所选 `--profile`，沿用登录与刷新机制，不自动登录或切换身份。服务端验证登录、实例绑定、scope 和分享权限；auth status 有缓存 token 并不等于服务端必然允许访问。

接收者先配置 profile 的 API endpoint，执行登录与状态检查，再显式启用 OAuth：

```bash
"$SHARE_CLI" --profile receiver auth login --instance "$SHARE_INSTANCE"
"$SHARE_CLI" --profile receiver auth status
"$SHARE_CLI" --profile receiver share inspect "$SHARE_URL" --auth-mode oauth
"$SHARE_CLI" --profile receiver share files "$SHARE_URL" --auth-mode oauth --path "$SHARE_FOLDER/" --max-keys 1
# 将上一页 data.NextMarker 赋给 NEXT_MARKER，再取下一页
"$SHARE_CLI" --profile receiver share files "$SHARE_URL" --auth-mode oauth --path "$SHARE_FOLDER/" --max-keys 1 --marker "$NEXT_MARKER"
"$SHARE_CLI" --profile receiver share download "$SHARE_URL" ./share-oauth-download --auth-mode oauth --recursive
```

显式 OAuth 的 API 来自所选 profile 或 `--endpoint`，不向复制来的任意页面域名发送账户凭据；标准 IDS 页面实例须匹配 OAuth 绑定。OAuth 用于概况和换取分享 token，文件 API 只携带 x-share-token，内容下载不携带账户凭据。设置提取码时 OAuth 仍须提供。普通终端可隐藏提示输入；`--password-stdin` 读取到 EOF，建议通过管道输入。提取码不会保存，每个 files 页和 download 独立调用都需要提供：

```bash
read -r -s -p '提取码: ' SHARE_PASSWORD; printf '\n'
printf '%s\n' "$SHARE_PASSWORD" | "$SHARE_CLI" --profile receiver share files "$SHARE_URL" --auth-mode oauth --password-stdin --max-keys 1
printf '%s\n' "$SHARE_PASSWORD" | "$SHARE_CLI" --profile receiver share files "$SHARE_URL" --auth-mode oauth --password-stdin --max-keys 1 --marker "$NEXT_MARKER"
printf '%s\n' "$SHARE_PASSWORD" | "$SHARE_CLI" --profile receiver share download "$SHARE_URL" ./share-oauth-download --auth-mode oauth --password-stdin --recursive
unset SHARE_PASSWORD
```

files 与 ls 一样列举当前层级，`--max-keys` 默认 1000，文件和目录合计不超过此上限；内部自动获取服务分页，直到达到上限或真正结束，服务单页较短也会继续。`--marker` 使用上次结果的 NextMarker 续取，IsTruncated=false 时结束；`--max-keys` 仅接受 1 至 2147483647 的整数，files 不支持 `--limit` 或其别名。精确文件查询返回一个文件且不分页。JSON/query 保留 Files、Folders、NextMarker、IsTruncated 字段名，内容为聚合、规范化后的当前层级结果；目录标记、自身标记和重复项统一处理，各格式的 Total 和 pagination.total_returned 均按逻辑条目计数。inspect 返回分享概况；文件详情通过 files 查询。download 自动枚举分页，不提供 `--max-keys`、`--marker`、`--limit`；管理侧 share list 继续使用原有 `--limit` 和 `--offset`。新的匿名/OAuth checkpoint 均记录实际 API；OAuth 还绑定选定 profile、模式和实际请求中可用的 user_id，续 token 时该身份上下文改变会拒绝继续。环境变量或仅 access token 凭据未提供 user_id 时只能绑定 API/profile/模式，CLI 不能确定跨恢复替换的未知主体 token 是否来自另一个账户，也不会解析 token 推断身份。旧匿名 checkpoint 只在 URL 推导 API 下迁移，原来自定义 API 未被记录时须新建任务。checkpoint 仅保留非敏感身份上下文，不含账户 token、分享 token 或密码。

`share files` 默认 table/CSV 与 `ls` 使用相同五列：file_path、size、file_type、updated_at、is_folder，目录在前，空结果也保留列头。所有格式按 ls 规则统一规范化当前目录和重复目录 marker；表格 Total 与 JSON 的 pagination.total_returned 统计相同逻辑条目。JSON 和显式 `--query` 保留聚合、规范化后的 Files/Folders 元数据及续取字段。`-H` 和 `--columns` 与 ls 共用 table/CSV 显示逻辑；显式 `--sort key|name|size|last-modified|mtime` 对本次 max-keys 集合在全部格式/query 中排序，保留原始元数据值，不改变 marker/截断边界；显式 `--manifest-path FILE` 才写列表 CSV，默认不写。

## 4. 核心验收

整 Space 服务端状态（2026-10-27）：CLI、本地 HTTP/PTY 夹具和 release 回归已完成，
但 BOE V1 真实创建仍受阻。`ShareAllFiles=true` 配合空 `FilePathList` 返回
`FilePathList is required`；使用合法相对目录后，服务端明确返回 HTTP 400、
`InvalidParameter.ShareAllFilesNotAllowed`，消息为 `Sharing all files in a space is not allowed.`，
request ID：`aec1e04bcbf35e7b0cf3a988d4a4f8c8-ac1d1cde-1xLbde-CSL-ccs-ids`。
这不是已通过的 BOE 整空间验收；创建请求的根范围契约、根列举及下载链路须在服务端支持后重新联调。
测试仅使用新建隔离 Space 和生成的测试内容，测试分享、文件、回收站及 Space 均已清理。

整 Space 先使用离线 create 预演核对选择器（无需真实凭据）：

```bash
"$SHARE_CLI" share create "adrive://$SHARE_INSTANCE/$SHARE_SPACE" --dry-run --output json
"$SHARE_CLI" share create "adrive://$SHARE_INSTANCE/$SHARE_SPACE/" --dry-run --output json
"$SHARE_CLI" share create --instance "$SHARE_INSTANCE" --space "$SHARE_SPACE" --dry-run --output json
"$SHARE_CLI" share create --instance "$SHARE_INSTANCE" --space "$SHARE_SPACE" --folder / --dry-run --output json
# 负例：必须退出 6，不得解释成 Space 根。
"$SHARE_CLI" share create --instance "$SHARE_INSTANCE" --space "$SHARE_SPACE" --file '' --dry-run --output json
```

前四项 selection 为 `ShareAllFiles=true`、`FilePathList=[]`；非根目录或文件仍为单项路径，selection 的 `ShareAllFiles=false`。真实非根创建请求省略 `ShareAllFiles`（服务默认 false）。实际 create/get/list/update 仅在服务返回时保留可选 `ShareAllFiles`（true/false），缺失表示未知。匿名概况可能不含该字段；整 Space 根列表使用只读适配，仅明确分享 scope 拒绝允许只读根列表 fallback，不把任意 403 当作可重试。

只在专用测试 Space 中另建整 Space 分享，并把其 URL 放入 `SHARE_SPACE_URL` 后核对：

```bash
"$SHARE_CLI" share files "$SHARE_SPACE_URL" --max-keys 1 --output json
"$SHARE_CLI" share files "$SHARE_SPACE_URL" --path / --max-keys 1 --output json
"$SHARE_CLI" share download "$SHARE_SPACE_URL" "$SHARE_LOCAL/space-preview" --recursive --dry-run --output json
"$SHARE_CLI" share download "$SHARE_SPACE_URL" "$SHARE_LOCAL/space-preview" --path / --recursive --include-parent --dry-run --output json
```

验收至少覆盖：根层级文件/目录及 marker 分页；根与嵌套同名文件保留完整空间相对层级；空目录；Space 根加 include-parent 不包装额外空间名；子目录仍按 cp 内容/父目录布局；筛选、并发、报告/清单和 checkpoint/resume 规则沿用现有契约。无 `--recursive` 的整 Space 下载须拒绝，不能隐式递归；预演不得写本地目录、checkpoint、锁、report、manifest 或签发下载 URL。匿名默认与显式 `--auth-mode oauth --profile receiver` 均覆盖，后者复用所选 profile，受保护分享仍需提取码。不得用普通文件/目录分享伪装整 Space 验收。

下载不传 `--path` 会选择全部分享根；单文件分享自动选中该文件，目录分享仍需 `--recursive`。`--path` 用于选择某个文件或子目录。预检查会先创建目录结构，后续申请下载地址被服务端拒绝时，可能只留下空目录。默认 table 与 cp 一样展示 total/succeeded/failed/skipped 汇总，并补充 pending；空结果也保留汇总。逐文件明细用 JSON 或显式 query 查看。JSON 的 Summary.Skipped 包括恢复复用的已验证完成文件和覆盖策略跳过项，包含在 Summary.Completed 中；表格 succeeded=Completed-Skipped。Files.Status 支持 completed/failed/pending/skipped。data 回显 report_path、manifest_path、checkpoint_path。列举回显和执行进度分别控制，TTY 默认在 stderr 开启。--list-echo/--no-list-echo 控制扫描回显；--progress/--no-progress 仅控制执行进度，非 TTY 强制开启时为普通文本行；--quiet 关闭两者。同组相反参数互斥。`Forbidden.ShareDownloadDisabled` 可能是分享权限禁止下载，也可能是下载次数配额耗尽；仅凭错误码无法区分。请分享者检查权限和配额，保留服务端错误码、HTTP 状态与 request ID。实际下载创建目录、临时文件、默认报告/清单 CSV 和启用的 checkpoint/锁；默认 force 覆盖，兼容 --overwrite；--no-clobber 和 --overwrite-strategy newer 可跳过现有/较新文件。校验通过后才替换普通文件，签发 URL 可能消耗下载次数，即使后续传输失败。

下载预演可在真实下载前执行：

```bash
"$SHARE_CLI" share download "$SHARE_URL" "$SHARE_LOCAL/downloads" --recursive --checkpoint "$SHARE_LOCAL/checkpoint.json" --dry-run --output json
```

预期：仅有分享 info/token/list/get 元数据请求，自动枚举全部分页及选中递归目录。`data.selection` 包含完整 `Manifest`（Roots、Files、Directories 与本地映射）、`PlanComplete=true`、`FileCount`、`DirectoryCount`、`TotalBytes`、`SkippedCount` 和 `Conflicts`。文件大小缺失或为负数时校验失败（退出码 6），不把未知大小当作零字节；显式 0 是有效空文件。完成的计划有本地冲突时退出 9，保留计划供查看。`DownloadsIssued=0`、`LocalWrites=false`；不签发下载 URL、不取内容、不创建目录/临时文件、不写 checkpoint/锁/report/manifest 文件。`MetadataResponses` 仅统计收到的元数据响应，未统计网络尝试次数，因此 `NetworkRequests=null`。`SnapshotGuaranteed=false`，检查后远端和本地仍可能变化。恢复预演使用已保存清单，只读校验身份、远端元数据与已完成本地摘要，不加锁、不保存。预演不获取 checkpoint 锁，现有普通锁文件可能正被使用；实际执行需重新检查并加锁。OAuth 预演禁用自动刷新以免写凭据/认证锁；token 已过期、距到期 60 秒内或收到 401 时，先正常执行 `"$SHARE_CLI" --profile "$SHARE_PROFILE" share inspect "$SHARE_URL" --auth-mode oauth`，再重试。其他分享 dry-run 离线，create/update 显式密码输入仍须校验。--describe 始终无配置、网络和 stdin I/O。

有强 ETag 或 CRC64 且大于等于 20MB 的文件自动使用 16MiB Range、默认 4 路并发，无需 checkpoint。用 `--checkpoint-threshold 64MB`、`--multipart-concurrency 8` 调整；当前文件的分片完成进度也写到 stderr。分片失败只重试该分片，复用同一下载 URL。无强 ETag/CRC64 时使用单次流式 GET，避免混合版本。启用 checkpoint 后可恢复未完成文件中已验证的分片。默认文件/目录/分片并发为 16/4/4，使用 --batch-concurrency、--list-concurrency、--multipart-concurrency 覆盖。--bandwidth-limit 0 不限速；非零值为共享每秒字节限速。--progress-granularity part|byte 选择进度单位，--list-echo/--no-list-echo 控制扫描回显。

`share view` 使用预览权限，默认返回文档预览 URL 或普通文件内容信息；`--raw` 将原始字节输出到管道/重定向（例如接 `less`），不会混入成功 Envelope。MCP 只提供结构化模式，不支持 raw。单文件分享可省略 --path，目录分享需指定完整文件路径。沿用提取码和显式 OAuth 规则；预览请求不会自动重试。历史 BOE 预览验收遇到纯文本 404，仍需确认可达的部署路由，详见下文待验证项。普通单文件下载有 info/token/root-list/download-url/content 五次请求，显式 --path 以 get-file 替代 root-list；分页、刷新和内容重试可能增加请求。

```bash
share_manage share get "$SHARE_ID" --instance "$SHARE_INSTANCE"
share_manage share get "adrive://$SHARE_INSTANCE/$SHARE_ID"
share_manage share list "adrive://$SHARE_INSTANCE" --name "$SHARE_FOLDER"
share_manage share list --instance "$SHARE_INSTANCE" --name "$SHARE_FOLDER"
share_manage share update "$SHARE_ID" --instance "$SHARE_INSTANCE" --description 'manual check' --download-limit 0
"$SHARE_CLI" share inspect "$SHARE_URL" --output json
"$SHARE_CLI" share files "$SHARE_URL" --output json
"$SHARE_CLI" share files "$SHARE_URL" --path "$SHARE_FOLDER/report.txt" --output json
# 预览路由确认后执行；view 权限即可预览，无需允许下载。
"$SHARE_CLI" share view "$SHARE_URL" --path "$SHARE_FOLDER/report.txt" --output json
"$SHARE_CLI" share view "$SHARE_URL" --path "$SHARE_FOLDER/report.txt" --raw | less
"$SHARE_CLI" share download "$SHARE_URL" "$SHARE_LOCAL/downloads" --recursive --checkpoint "$SHARE_LOCAL/checkpoint.json" --output json
cmp "$SHARE_LOCAL/report.txt" "$SHARE_LOCAL/downloads/report.txt"
test -d "$SHARE_LOCAL/downloads/empty"
"$SHARE_CLI" share download "$SHARE_URL" "$SHARE_LOCAL/downloads" --recursive --checkpoint "$SHARE_LOCAL/checkpoint.json" --resume --output json
```

预期：两种 get 写法返回相同 ShareID 和 URL；两种 list 写法在相同过滤条件下返回相同分享条目和分页字段。list 的 URI 可带一个根目录斜杠，不能带 ShareID/空间/文件路径；update/cancel 也支持分享 URI，重复指定的 `--instance` 必须一致，`--by-name` 解析实例名称。下载和恢复都为 `Complete=true`、`Summary.Completed=1`、`Summary.Failed=0`；`cmp` 和 `test` 返回 0。空目录保留，精确文件查询只返回一个文件。`share files` 默认自动聚合至当前层 1000 个逻辑条目或结束；使用 `--max-keys 1` 检查 `NextMarker`，再传 `--marker` 继续，直到 `IsTruncated=false`，续取结果不应跳过条目。

整个命令组的输入按资源范围保持一致：create 用文件/文件夹 URI 或显式选择器；list 用实例 URI 或 `--instance`；get/update/cancel 用分享 URI 或 ShareID 加 `--instance`；inspect/files/view/download 使用公开 HTTP(S) 分享 URL。接收支持 `/share/<ShareID>` 和 `/share/<ShareID>/`，只规范化末尾一个斜杠；两者是同一分享和恢复任务。目录下钻使用 `--path '完整目录前缀/'`，不追加到分享 URL 后面。对应信息同步在 `--help`、`--describe`、capabilities、Skill 和 MCP 中。

建议按以下用例继续验收：

| 用例 | 操作 | 预期 |
| --- | --- | --- |
| 合计上限与续取 | files 使用 `--max-keys 1` 列举含文件和目录的当前层，再传 NextMarker 继续 | 每次文件和目录合计不超过 1，续取无遗漏，结束时 IsTruncated=false |
| 默认自动分页 | 本地夹具返回短服务分页，不传 max-keys | 自动聚合到 1000 个逻辑条目或真实结束，不因单页较短而提前停止 |
| 空目录与目录标记 | files 查询空目录、包含自身目录及重复目录标记的列表 | 空结果列头稳定；所有格式按相同逻辑条目计数，无重复 |
| 非法上限和旧参数 | files 使用 max-keys 为 0、负数、2147483648，或传 `--limit` | 参数校验失败且不发出网络请求；MCP 拒绝同样输入 |
| 异常分页游标 | 本地夹具返回重复、循环游标，或缺少游标的截断结果 | 明确失败且有界退出，不挂起、不跳过尚未返回的条目 |
| 单文件分享 | 对 `${SHARE_REMOTE}report.txt` 再 create；调用 files/view/download | 一条文件，下载落为 `report.txt`；记录新 ShareID 用于清理 |
| 密码 | 创建时加 `--password-stdin`，接收时通过 stdin 提供正确/错误密码 | 正确密码成功，错误密码明确拒绝；inspect 不读取密码 |
| 明确禁止下载 | 将当前分享 update 为 `--permission view`，再下载到新目录 | 退出码 5、403、`Forbidden.ShareDownloadDisabled`、`IssuanceOutcomeUnknown=false` |
| 根目录与恢复 | 使用普通目录或自己配置的目录别名作为 destination/checkpoint | 正常下载；等价实际目录可恢复，macOS /tmp、/var 别名允许 |
| 完整预演 | download 加 --recursive --dry-run，检查多页和空目录 | selection.Manifest、计数与总字节数完整；仅元数据请求，无目录/临时文件/checkpoint/锁写入 |
| 覆盖策略预演 | 已有普通文件，分别预演默认 force、--no-clobber、newer | 默认计划 overwrite；no-clobber 计划 skip；newer 依时间决定，不写本地 |
| 恢复本地变化 | 完成后改变本地文件大小或同大小内容，再 --resume（含 dry-run） | Conflict，退出 9；本地文件和 checkpoint 不变 |
| 子路径链接 | 在下载后代路径放置符号链接/Windows reparse 点 | Conflict，退出 9；提示处理本地目标路径，不签发 URL |
| 显式空密码 | create/update（含 dry-run）或接收时通过 --password-stdin 输入 EOF/LF/CRLF | 校验失败退出 6，不提交修改或交换分享 token；仅空格输入保留 |
| 并发更新 | 保存 get 的 `UpdatedAt`，更新后用旧值 `--updated-at` 再 update | 冲突，退出码 9；旧请求不覆盖新设置 |
| 撤销 | cancel 后再次 files | 明确拒绝，不继续签发下载地址 |

密码通过 `--password-stdin` 传入，避免放进 argv 或日志。该参数等待 EOF：macOS/Linux 手动输入时，30 秒内输入密码、回车，再在空行按 Ctrl-D；单独回车不会结束读取。管道关闭时自动执行。普通交互终端接收 files/view/download 可不传该参数，按隐藏提示输入并回车。未知签发结果、路径穿越、内部链接、checkpoint/lock 冲突与签发期间中断由本地自动化夹具验证，无需破坏云端环境来制造异常。

## 5. 清理本轮云端数据

先取消本轮每个创建成功的 ShareID，再删除这个随机文件夹。额外单文件/密码分享也需要逐个取消。
已在撤销用例中取消的 ShareID 无需再次取消；跳过对应 cancel 命令即可。

```bash
share_manage share cancel "$SHARE_ID" --instance "$SHARE_INSTANCE" --yes
share_manage rm "$SHARE_REMOTE" --recursive --force --confirm "$SHARE_REMOTE" --yes
share_manage stat "$SHARE_REMOTE"
```

预期：取消、删除成功；最后 stat 返回 404 / 退出码 4。清理仅使用完整的随机测试目录 URI，保留现有 Instance、Space 和其他内容。本地样本与导出文档在 `$SHARE_LOCAL`、`$SHARE_DOCS`，检查后自行删除。

仓库中的 `scripts/e2e/adrive/test_share.py` 默认 fixture 使用 AK/SK 环境变量；现有 BOE 分享管理要求 OAuth，不能把该 fixture 的 AK/SK 失败当成上述 OAuth 功能失败。上一轮 OAuth 实测通过适配器复用了其中的生命周期断言；本文给出可复现的 OAuth 管理及匿名/显式 OAuth 接收命令。

## 6. cp 对齐与恢复验证

下载默认复制目录内容；`--path docs/` 的 `docs/a.txt` 落到目标 `a.txt`，加
`--include-parent` 后落到 `docs/a.txt`。空目录仍保留；`--include`/`--exclude`
按 cp 源相对路径过滤。多个根目录 contents 出现同名时会拒绝；使用 --include-parent
保留不同根目录名，不自动猜测新名称。先用 `--dry-run` 核对 Manifest、计数和冲突，再实际下载。
全量快照不再有固定 20,000 项/8 MiB 硬上限；路径、深度、循环和映射碰撞检查保留。
本地传输设置优先级为 CLI > 所选 profile 的 adrive/shared 字段 > ADRIVE 环境变量 > 默认值；不读取账户凭据或改变接收鉴权。

`--checkpoint` 裸参数使用 `~/.tos/checkpoints/ve-adrive` 下稳定任务路径；
`--checkpoint-dir DIR` 覆盖目录，`--checkpoint FILE` 保留显式文件写法。
恢复须保持选择、过滤、目录布局及覆盖策略；旧保留目录名布局需 `--include-parent`。
保存或迁移恢复任务时须一起保留 checkpoint JSON、`<checkpoint>.completed/` 增量账本和
`<checkpoint>.parts/` 目录；主 JSON 的 completed 不逐文件实时更新，不能只复制 JSON。
已完成文件重新验证远端身份和本地大小/SHA-256，未完成文件只复用验证通过的 Range。
普通文件失败继续其他文件；目录错误单列在 DirectoryFailures（LocalPath/Path/Message），并写失败 mkdir 报告行，Summary 仍仅统计文件。签发结果未知时停止新派发，不自动重新签发。
失败内容 Range 复用同一 URL 重试，不重新签发来绕过失败。

报告和清单 CSV 默认写入 `~/.tos/reports/ve-adrive`；`--report-path`、
`--manifest-path` 覆盖路径，`--report-failures-only` 仅保留失败报告行，
`--no-manifest` 只关闭 CSV，安全预扫描和恢复快照仍存在。
报告逐行 request_id 必须属于该文件操作；恢复/策略跳过行不得借用上一文件 ID。
quiet、query、table/JSON 格式均不影响产物写入；产物路径在 data 中回显。

| 场景 | 操作 | 检查 |
|---|---|---|
| 并发 | 多文件、多目录，设置文件/目录/分片并发为 2/2/2 | 相应请求重叠且不超过各自上限；默认值为 16/4/4 |
| 过滤与布局 | include/exclude，分别有无 include-parent | Manifest 只含匹配源路径，目标映射和空目录符合 cp |
| 策略跳过 | no-clobber 与 newer，预置普通文件及时间戳 | Files.Status=skipped；Skipped 属于 Completed；succeeded 不含跳过 |
| 部分恢复 | 开启 checkpoint，中断大文件后 resume | 已验证 Range 不重复拉取；无效 Range 重新获取；URL 不在状态中持久化 |
| 报告过滤 | 混合成功/失败，开启 report-failures-only | 报告只含失败，manifest 仍完整；列头及 request_id 正确 |
| 关闭 CSV | no-manifest，含可恢复任务 | 不写 manifest CSV，保留安全扫描/checkpoint；报告仍写入 |
| 路径碰撞 | report/manifest/checkpoint 与下载映射或 CSV 分卷重叠 | 内容请求前拒绝，不截断下载文件或状态 |
| MCP checkpoint | true、false、非空 FILE 字符串及空字符串 | true 为裸参数、false 不启用、FILE 为路径；空串拒绝，不把 bool 变路径 |
| 离线发现 | 不可读配置、关闭 stdin 后 help/describe | 不读配置/stdin/network；中英参数及 schema 一致 |

预览部署仍需单独验证：若 `/v1/share_file/preview` 返回无 IDS request ID 的
纯文本 404，核对部署路由或服务端确认的 URL。本地契约测试不表示云端预览验收，
不得自动回退下载接口或改分享权限。
