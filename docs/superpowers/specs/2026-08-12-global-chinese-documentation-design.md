# 全局中文文档覆盖设计

## 目标

完整清理 `ve-adrive`、`ve-tos`、`tos` 三套命令中以下中文输出面的英文说明残留：

1. `--help --language zh`
2. `--describe --language zh`
3. `skill list/export --language zh`

命令、参数、枚举、环境变量、URI、JSON 字段、协议名和示例命令属于机器契约，保持原值。

## 根因

当前实现使用英文元数据加事后字符串替换。翻译表只覆盖部分短语，并且允许子串替换，因此会出现两类问题：

- 完整英文说明未命中，整句保留英文；
- 只命中句子的一部分，形成中英文混排。

现有测试主要抽查少量命令，没有遍历完整命令树，也没有对 Describe 和 Skill 的全部人类可读字段执行覆盖检查。

## 方案

保留现有英文元数据作为唯一命令契约，使用集中中文翻译目录完成中文渲染，不改 Clap、registry、MCP 或业务处理器的字段结构。

### Help

- 保留根分发层的中文 Help 渲染入口。
- 补齐三套 CLI 的命令说明、参数说明、长帮助、Notes 和 After-help 文案。
- 对完整源短语优先做精确翻译；较短的通用短语只在确认不会形成半句翻译时复用。
- 顶层分组帮助使用的 registry description 也必须纳入同一覆盖审计。

### Describe 与 Skill

- ADrive、VeTos、TOS 分别在 `crates/adrive`、`crates/tos`、`crates/toscli`
  的所属 meta 模块维护文档翻译目录。
- Describe 与 Skill 复用所属模块的同一递归本地化函数，禁止各自维护独立翻译副本。
- 中文 Skill 不再以“原始英文说明”包装未翻译内容；所有人类可读说明必须有中文结果。
- 英文输出路径保持原样。

## 覆盖审计

新增自动化门禁，覆盖三套 CLI 的完整命令树，而非固定抽样：

- 遍历 Clap 命令及全部子命令，检查 command about/long-about、argument help/long-help、before/after-help 和 possible-value help。
- 检查三套顶层 grouped-help 使用的 registry description。
- 遍历中文 Describe JSON 中的人类可读字段。
- 检查中文 Skill list 元数据及实际导出的 Markdown。
- 允许稳定机器内容和约定技术名词；普通英文句子、半句英文及“原始英文说明”均判为失败。
- 同时验证英文 Help、Describe、Skill 与修改前一致。

当后续新增命令或参数但未补中文说明时，覆盖测试必须直接失败，并输出命令路径和未翻译文本。

## 兼容性边界

- 不修改命令名称、参数名称、默认值、枚举值和环境变量。
- 不修改 JSON schema、Envelope、registry/MCP tool 名称或 Skill 名称。
- 不修改鉴权、配置、Doctor、网络请求和业务行为。
- `--language en` 以及未指定语言的默认英文输出保持不变。
- 仅修复中文文档渲染和覆盖测试。

## 验证

- 先增加能够稳定复现现有混排问题的全局覆盖测试并确认 RED。
- 补齐翻译后确认三套 CLI 的 Help、Describe、Skill 全部 GREEN。
- 运行 CLI、ADrive、VeTos/TOS 相关完整测试、格式检查与 diff 检查。
- 完成独立 Reviewer 视角审查，修复全部 Critical/Major 后提交。
