# 文档索引

本目录保留持续维护的指南。已完成的实施计划、一次性验收和审查记录从当前目录移除，历史内容可通过 Git 查询。

| 文档 | 用途 |
| --- | --- |
| [API 实现原则](api_implementation_principles.md) | 命令发现、结构化输出、安全执行及错误契约的验收要求 |
| [开发工作流](development_workflow.md) | 小修复、功能开发、文档联动与发布验证边界 |
| [ADrive 分享测试指南](adrive-share-testing.md) | 本地回归、云端验收步骤、当前能力边界和待验证事项 |
| [Space 文档片段检索](adrive-chunk-search.md) | Chunk 检索、版本扩读、OAuth 边界与本地回归 |
| [OAuth group Space 列举](adrive-space-listing.md) | RootSpace 显示、分页游标、统计与清单契约 |
| [CLI 直连豆包](ask-direct.md) | 固定权限、应用凭据构建注入、登录、问答及当前验收边界 |
| [构建与发布手册](../packaging/README.md) | 各平台构建、应用凭据注入、独立版本与各分发渠道发布 |
| [E2E 测试指南](../scripts/e2e/README.md) | 测试环境、对象协议回读、资源清理与显式云端验收 |

用户命令和默认行为见 [项目 README](../README.md)，完整参数以各命令的 `--help` 和 `--describe` 为准；Agent 工作流见 [安装式 Skills](../skills)。
