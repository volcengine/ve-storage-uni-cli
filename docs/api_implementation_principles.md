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

# API 实现六项原则

本文档适用于本仓库当前公开的 API 和二进制入口，包括 `tos`、`ve-tos`、`ve-adrive`，以及 TOS 的 Core、Bucket Configuration、Advanced 等 Low-Level API。实现、评审和测试时必须以以下六项原则为验收准则。

## 原则一：Discovery（能力发现）

所有 API 能力必须可被用户和 Agent 发现。

- 分组命令需要支持能力枚举，例如 `--help` 和分组级 `--describe`。
- `--describe` 应输出完整子命令清单、能力说明、适用范围和关键路由约束。
- 新增 API 时必须同步补齐 discoverability 测试，避免命令只能靠源码或文档猜测。

## 原则二：Understanding（命令理解）

命令必须让人和 Agent 都能理解其协议语义。

- `--help` 面向人类用户，必须展示关键参数、风险提示和用法说明。
- `--describe` 面向机器消费，必须尽量包含 API 名称、参数位置、必填性、风险级别、请求/响应契约。
- Header、Query、Path 参数应显式表达；复杂 Body 参数统一说明其结构化输入方式，例如 `--config` JSON。

## 原则三：Safe Execution（安全执行）

默认执行路径必须避免误操作和不可逆风险。

- 支持 `--dry-run` 的命令必须先生成执行计划，不发真实请求。
- 删除、覆盖、批量变更等危险操作必须要求显式确认，例如 `--force`。
- 解析类和原则类测试必须使用 `--dry-run` 或其它隔离机制，禁止误触发真实网络请求或真实资源变更。

## 原则四：Controlled Output（可控输出）

输出必须可控、可解析、可组合。

- 命令应支持结构化输出，尤其是 `--output json`。
- dry-run、describe、错误和正常响应都应避免只输出不可解析的自由文本。
- 输出中不得泄露敏感信息；包含凭证、Token、密钥的内容必须脱敏或省略。

## 原则五：Deterministic Errors（确定性错误）

错误必须稳定、可预测、可被 Agent 处理。

- 参数缺失、格式错误、配置缺失、安全保护触发等应返回确定性错误类型和稳定文案。
- 禁止在核心状态机中使用会导致 panic 的路径；应返回带上下文的错误。
- 测试应覆盖关键错误路径，确保退出行为不依赖宿主机真实配置、环境变量或外部服务状态。

## 原则六：Agent Ecosystem（Agent 生态集成）

CLI 必须能作为 Agent 工具链中的可靠执行单元。

- 能力发现、命令理解、安全预检、结构化输出和确定性错误必须组合成完整 Agent 工作流。
- `--describe` 和 `--dry-run` 应足以支持 Agent 在执行前完成规划、风险评估和参数校验。
- 新增 API 时应补充 Agent 友好的测试矩阵，包括 discover、understand、safe execute、controlled output、deterministic error。

## 当前复核状态

所有 API 新增或变更都必须逐项复核以上六项原则。当前仓库已有的 Core、Bucket Configuration、Advanced 相关测试分别覆盖了部分原则；后续扩展 ADrive 或其它公开 API 时，也必须补齐对应原则测试。

本轮 Advanced Low-Level API 已按以上原则完成复核：

- `Discovery`：8 个 Advanced 分组的 `--describe` 返回完整 action 数和 endpoint rule。
- `Understanding`：叶子命令 `--help` 暴露关键通用参数，`--describe` 输出 `api`、`parameters`、`risk_level`、`endpoint_kind`。
- `Safe Execution`：`DELETE` 非交互要求 `--force`；解析测试使用 `--dry-run`。
- `Controlled Output`：dry-run / describe 支持 `--output json`，真实请求沿用统一 envelope。
- `Deterministic Errors`：缺少必需参数、缺少 `--config`、缺少 `--force` 均返回稳定 `ValidationError`。
- `Agent Ecosystem`：测试覆盖 group discovery、leaf help、endpoint dry-run plan、Body 契约、force 保护和 describe metadata。
