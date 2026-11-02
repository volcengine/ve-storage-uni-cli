# 文档索引

本目录保留持续维护的指南。已完成的实施计划、一次性验收和审查记录从当前目录移除，历史内容可通过 Git 查询。

| 文档 | 用途 |
| --- | --- |
| [API 实现原则](api_implementation_principles.md) | 命令发现、结构化输出、安全执行及错误契约的验收要求 |
| [开发工作流](development_workflow.md) | 小修复、功能开发、文档联动与发布验证边界 |
| [ADrive 分享测试指南](adrive-share-testing.md) | 本地回归、云端验收步骤、当前能力边界和待验证事项 |

用户命令和默认行为见 [项目 README](../README.md)，完整参数以各命令的 `--help` 和 `--describe` 为准；Agent 工作流见 [安装式 Skills](../skills)，真实环境测试配置见 [E2E 指南](../scripts/e2e/README.md)。
