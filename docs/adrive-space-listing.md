# OAuth group Space 列举

`ls adrive://instance-id --owner-type group` 列举当前 OAuth 用户可访问的 group
Space。复用所选 profile 的 OAuth 登录、Instance 绑定和 Resource endpoint；不新增
CLI 选项，也不改变 Auth、user Space 集合或 AK/SK 的分页行为。

独立入口为 `ve-adrive-cli`；统一入口可使用 `ve-storage-uni-cli ve-adrive`。
以下示例假定 profile 已配置为 OAuth，并已授权目标 Instance。

## 返回的 Space 与显示

```bash
ve-adrive-cli ls adrive://instance-id --owner-type group --output json
```

服务返回的 `RootSpace` 与 `Spaces` 合并到 `data.spaces`，按 `SpaceID` 去重，root
置顶。每个 `data.spaces` 行都包含布尔字段 `is_root`。服务没有返回 RootSpace 时，
命令仍正常返回其余 Space；没有可见 Space 时可以返回空列表。

| 输出 | root 行 | 普通行 |
| --- | --- | --- |
| JSON / YAML / XML | `is_root: true`，保留真实 `owner_type` | `is_root: false`，保留真实 `owner_type` |
| 未使用 `--query` 的 table / CSV / Markdown | `owner_type` 显示为小写 `root` | `owner_type` 保持原值 |
| 显式 `--query` | 按查询选择结构化字段；`owner_type` 仍是真实值 | 按查询选择结构化字段 |

`root` 是显示标签，不是新的 owner 类型或 `--owner-type` 参数值。判断某行是否为
root 时使用 `is_root`，不要依赖真实 `owner_type` 的取值。

`data.root_space` 为兼容旧调用方保留。root 已经属于 `data.spaces`，因此页内数量、
跨页统计和清单都只计算合并去重后的 `data.spaces`，不要再单独加上 `root_space`。

## 页容量与续页

`--max-keys` 限制当前逻辑页的 Space 数量，root 也占一个名额。例如，服务同时返回
root 和普通 Space 时，`--max-keys 1` 的首个逻辑页只返回 root，普通 Space 留待续页，
不会因为 root 占用名额而被跳过。一旦服务通过 RootSpace 识别 root，沿 CLI 返回的游标链继续时，该 root 跨页只输出一次。

```bash
# 第一页；读取 data.is_truncated 和 data.next_marker。
ve-adrive-cli ls adrive://instance-id --owner-type group \
  --max-keys 1 --output json

# 仅在 is_truncated 为 true 时，把上一响应的 next_marker 原样传回。
ve-adrive-cli ls adrive://instance-id --owner-type group \
  --max-keys 1 --marker '<previous-next-marker>' --output json
```

重复续页直到 `data.is_truncated` 为 `false`。不要根据当前页是否为空、是否含 root
或 `root_space` 是否出现来决定是否结束，也不要自行生成下一页 marker。

OAuth group 的 CLI marker 保存 root 是否已经返回、服务端 continuation marker，
以及部分消费服务页后的重放位置，确保页容量小于服务页内容时仍能继续。它不含
OAuth Token、AK/SK 等凭据，但仍应作为不透明的续页值处理：

- 使用响应中完整的 `data.next_marker`，不解析、不截取、不拼接，也不修改内部内容。
- 只用于原来的 Instance 和 owner 集合；不要跨 Instance 或在 user / group 集合间复用。
- 仍兼容旧的 raw 服务 marker；新响应返回 CLI marker 后，继续原样使用新值，
  不要将自己取得的服务 marker 替换进去。

列举不是快照。服务端在翻页期间新增、删除或调整 Space，可能改变后续页；游标
记录续页位置，不保证在变化中的集合上提供快照一致性。若 RootSpace 首次出现前，
同 ID 已在此前逻辑页作为普通 Space 返回，CLI 无法追溯重分类此前的结果。

## 统计与清单

统计一个集合时，逐页累计 `data.spaces` 的实际行数即可。分别统计 user 和 group
集合后再汇总，不额外计算 `data.root_space`。

`--manifest-path` 显式写出当前列举的清单，来源是合并后的去重行，包含返回的 root，
清单总条数按实际写入行计算。没有该选项时不会创建列举清单。逐页保存时为每页
指定不同路径，例如：

```bash
ve-adrive-cli ls adrive://instance-id --owner-type group \
  --max-keys 1000 --manifest-path ./group-spaces-page-1.csv --output json
```

不要把兼容字段 `root_space` 再追加到清单。也不要把整个 Instance 的 Space 列举与
`ls adrive://instance-id/space-id/` 的文件列举合并计数；两者是不同层级的集合。
