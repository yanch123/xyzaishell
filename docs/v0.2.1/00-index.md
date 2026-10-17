# 00 — v0.2.1 设计索引

日期：2026-10-17（UTC）。状态：**已合并 main 并发布未签名预发布版；最终资产已核验，详见发布记录**。

## 1. 范围与状态

| 项目 | 本阶段状态 |
| --- | --- |
| 五类上下文面板、真实来源计量 | 已实现；回归与原生自动化验收通过 |
| LLM 整理较早用户消息，quick 保守保护用户约束 | 已实现；回归与原生自动化验收通过 |
| 原有预算、调度、权限、脱敏和归档机制 | 沿用 dev 基线，不重写整体架构 |
| 文档交付 | 设计文档已先行提交；当前开发证据见 04 |
| 应用开发 | 已上传 dev；精确候选、原生结果及未覆盖项见 04 |
| main／标签／Release | 已获授权并完成；标签指向精确被测候选，预发布且非 Latest |

本版不新增 Skills、MCP 或子 Agent 产品功能。

## 2. 设计阶段已核实基线（历史记录）

- 已重新 fetch：`origin/dev = b1f1e6a24d875a799e6aec95914be9cc2efe0303`；`origin/main = b89a9ff1ee1f820623727ceb2570fa3cf800b1ab`，其 `package.json` 仍为 `0.1.4`。
- dev 最新提交只更新 v0.2.0 文档；应用代码基线为 `bcd44168403fe0611ca0b7da0929987fa7febdfe`。两者差异仅为三份 `docs/v0.2.0` 文档。
- v0.2.0 [既有验收记录](../v0.2.0/06-verification-report.md)记录 [CI 38957752276](https://github.com/yanch123/xyzaishell/actions/runs/38957752276) 的 12 个作业成功。这是既有基线证据，本阶段没有重跑 CI，也不是 v0.2.1 验收结果。
- 已阅读 `docs/v0.2.0`、`context.rs`、`tasks_context.rs`、`ContextIndicator`、`useContextUsage` 及相关请求构造、类型和测试。旧文档中的“用户消息全部保护／策略待确认”保留为历史状态；本目录明确新策略，不倒写旧验收。

## 3. 阅读顺序

1. [01 — 需求与交互](01-requirements-and-interaction.md)：五类定义、两种压缩的差异和不变项。
2. [02 — 架构与接口变更](02-architecture-and-interfaces.md)：来源标记、协议开销、模式保护和来源／依赖拆分。
3. [03 — 验收与交付](03-acceptance-and-delivery.md)：阻断性用例、开发切片和证据边界。
4. [04 — 开发与验证记录](04-development-and-verification.md)：当前精确代码、已执行检查和待验项。

核对重点：五类来自实际构造来源；五类内容加独立协议开销对账；LLM 摘要属于可能有损的模型解释；quick 原样保护所有现用摘要；追溯来源不等于保留原文的依赖。

## 4. 发布完成

[公开 Release v0.2.1](https://github.com/yanch123/xyzaishell/releases/tag/v0.2.1) 已于 2026-10-17 发布，包含源码与 Windows／Linux 安装包。最终来源、七项资产摘要及边界见 [02 — 发布核验](release-evidence/02-published-release.md)。本页早期开发证据不替代最终发布核验。
