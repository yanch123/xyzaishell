# 03 — Token 计量与接口契约

状态：v0.2.0 设计契约，不是已实现 API。接口命名用于后续实现对齐，不能把现有字节上限改名为 token 上限。

## 1. 单一权威输入

引入后端统一请求准备路径 `prepare_context`：收集当前授权范围 → 脱敏 → 选择历史/摘要/证据 → 构造消息和工具定义 → 计量 → 预算检查 → 发出同一份准备结果。普通聊天、带选文聊天和终端规划都走这条路径；前端不独立拼接另一份“用于统计”的上下文。

`PreparedContext` 是即将发送的一次模型请求输入投影。包含系统规则、工具 schema、消息角色、工具参数/结果和实际发送的引用文本；不包含整个 UI 历史、未采用的日志、私密任务文件、未经同意的附件、HTTP 头、API key。草稿预览可以纳入当前草稿和有效已授权附件，但仅本地准备，不触发模型调用。发送前必须重新准备并校验版本，不能信任过时预览。

计量对象是适配器最终发送的语义请求及已知角色/工具封装，不是简单对 HTTP JSON 字符串做 tokenization。JSON 编码字节数单独统计；服务端 chat template、隐式提示或供应商额外开销不可见时，估算误差必须保留。即使有匹配 tokenizer，也不宣称与供应商实报完全一致。

计量、预算和压缩必须共用同一规范化序列化路径。构造 token 分类清单后，发送的是同一修订投影；不得统计删减前 A、发送删减后 B、却不刷新 UI。候选输入超过既有 prompt 限制时，应在旧截断/拒绝之前检测并尝试合法压缩，避免始终只看到截断后的 60 KiB 而隐藏压力。最终发出前仍验证所有迁移后生效的字节护栏。

## 2. 三种不同的数量

### 2.1 当前输入估算

`estimated_input_tokens`：当前准备请求的输入 token 估算。每次草稿、摘要、工具输出、权限、模型或请求配置改变都可失效。采用匹配模型 tokenizer 时记录其版本；没有可靠映射时采用明确标注的启发式估算（覆盖中文、代码、JSON、长数字和混合文本）。不能把固定 bytes/4 当作所有语言模型的精确事实，也不能远程发送草稿以完成计数。

启发式估算建议使用保守上界参与保护判断，展示主估算及“粗略估算”标签；若有上下界，字段需明确其为工程估计区间而非统计置信区间。未知估算返回 null 和原因，不返回 0。数字显示约数，例如 ≈24k；内部阈值计算用未格式化整数，不根据 UI 四舍五入数触发。

### 2.2 供应商上一次实报

`last_request_usage`：供应商对某个已完成/部分完成请求返回的 usage，绑定其 request_id、请求类型、模型、配置修订和投影修订。仅表示那个请求，不是下一轮上下文。输入、输出、缓存和推理相关字段按供应商语义解析；缓存输入通常是输入子集，不能重复相加。

没有 usage、流式末包丢失、供应商不支持或字段意义不明时返回 null/unknown，不补造“实际 token”。供应商通常不给类别分解，因此类别始终标估算，不把实报总量按比例分摊后叫实报。压缩摘要请求的实报归属 `compaction`，普通问答/终端规划归属 `conversation`/`planning`；摘要调用不能覆盖“上一次主请求”的展示。详情可另列最近压缩请求用量。v0.2.0 不承诺费用、价格或精确账单。

协议依据：OpenAI Chat Completions 的流式 usage 需要请求 `stream_options.include_usage`，最后的 usage 块统计该请求；流中断或取消时可能收不到它。该协议事实不能推广成所有 OpenAI 兼容服务都支持，适配器需按能力处理并保留缺失状态。见 [OpenAI 官方流式事件参考](https://developers.openai.com/api/reference/resources/chat/subresources/completions/streaming-events)（核对日期：2026-10-16）。80%/50% 阈值及类别方案是本项目设计决策，不是供应商或 Cursor 的既定行为。

### 2.3 字节护栏

`serialized_request_bytes`：最终 API 请求 JSON 的 UTF-8 字节数。

`planning_prompt_bytes`：终端规划 prompt 的 UTF-8 字节数（适用时，用于诊断与旧路径迁移对照，不代表永久维持 60 KiB 发送门槛）。

它们与 token 计量并列，不进行单位替换。现有实现中 `agent.rs` 的 `MAX_PROMPT_BYTES = 60 * 1024`、`model_transport.rs` 的 1 MiB 请求限制、`tasks.rs` 的 16 KiB 单消息与 256 KiB transcript 限制，分别保护不同资源。60 KiB 是规划 prompt 上限，不是整个最终模型请求上限；工具 schema 和外层规则还会增加开销。这些是当前实现基线，不是要求新版本原样保留的全局发送限制。v0.2.0 按 02/04 退役旧 128 条/256 KiB 全局发送门槛，改为独立历史保留；替代旧 60 KiB prompt 门槛，保留审计后的 transport 保护。压缩上下文不取消历史内存保留边界。

## 3. 预算与显示公式

统一命名：

- `context_window_tokens`（W）：已验证模型元数据或用户明确设置的模型窗口；未知为 null。
- `output_reserve_tokens`（R）：继承现有 `request.max_output_tokens`，当前实现默认 4096；不新增第二个独立输出配置。
- `safety_margin_tokens`（S）：默认 max(1024, ceil(W × 0.05))；高级覆写必须验证，W 未知时为 null。
- `input_budget_tokens`（B）：W − R − S；只有 W 已知且 B > 0 时有效。
- `estimated_input_tokens`（I）：当前请求估算；使用粗略估算时保护判断可取其明确记录的保守上界。
- `input_occupancy_ratio`：I/B；主界面百分比与默认自动触发都使用该分母。
- `auto_trigger_ratio = 0.80`；`target_ratio = 0.50`。

为了避免“看起来未达阈值却自动压缩”，如保护采用上界，主占用也使用同一上界并注明“保守估算”，不得主指示显示点估计而阈值暗用另一量。类别可显示主估算，若与保护估算不同，详情明确标记，不伪装二者精确相加。

已知 W 时，若审计后的传输字节保护先于 token 阈值达到，可以另行触发自动压缩；原因记录为 `byte_pressure`（与 token 同时触发则为 `both`；完整枚举见 02 的 6.5），主占用百分比仍按原公式。所有自动原因均使用已配置方式，默认 LLM，明确选择自动 quick 时不得调用模型。未知 W 时不按字节触发自动压缩。

输入构成占比 = 某类别估算 / 同一方法下全部类别估算。输出预留和安全余量不进入类别份额分母。构成条只代表输入，预算占用则代表当前输入与 B 的关系，两个标题必须区分。舍入后的份额可使用最大余数法使显示总和为 100%；输入为零时不做除法。

示例（仅演示，不代表某个真实模型）：W=100000，R=4000，S=5000，则 B=91000。I=72800 时主指示 ≈80%，达到自动阈值；目标输入不超过 45500。弹层单列 W、R、S，不另加一个易混淆的常驻窗口占比。

W 未知时 B 与占用比均为 null，显示“容量未知 / 自动压缩待确认容量”，仍显示可得的估算输入和字节检查。不自动触发 LLM 或快速压缩；手动压缩仍可用，LLM 仅在迁移后生效的字节护栏内有界尝试，超限安全失败。不可因 `model.name` 类似官方名称或 API 兼容而填入臆测窗口。模型元数据过期/目标改变时重新解析；用户输入优先，并标记 `user_override`。R+S >= W 时为预算配置错误，不做负数、零除或自动减输出；保留草稿并指引修改配置。

## 4. 互斥类别契约

每个实际发送片段在准备阶段带一个 `category`，下列键稳定；界面中文可调整。

| category | 实际计入内容 | 不重复归属规则 |
| --- | --- | --- |
| `rules_tools` | 系统/开发者规则、工具名称/描述/schema | 包括工具定义，不包括实际调用 |
| `goal_constraints` | 显式结构化携带的当前目标和约束 | 若仅存在用户消息正文则归 dialogue；单独复制发送的结构化段另计 |
| `summary` | 已生效的压缩摘要 | 摘要中的事实/证据仍归 summary |
| `dialogue` | 保留用户和模型自然语言正文，含本轮草稿 | 结构化工具调用/结果不在此项 |
| `tools_results` | 实际工具调用参数、结果、终端输出 | 工具结果中自带证据归此项，不再算 evidence |
| `evidence` | 独立附加的引用/选文/附件摘录 | 已嵌入摘要或工具结果的相同实体不再另加虚拟计数 |
| `other` | 已知角色封装、控制元数据、计数归一化残差 | 不包括 HTTP 头、密钥和输出预留 |

“不重复”指不对同一个发送片段重复归类；如果实际请求真的重复了文本，两份都必须计入，并可作为后续去重优化信号。来源相同不等于 token 成本只发生一次。

优先对规范化完整输入一次计量并按 token/span 归属分类，以免跨边界 tokenization 造成分项之和不等于总计。如 tokenizer 不能暴露 span，适配器必须使用确定性非负归一化分配并标注 `allocation_method`；已知封装开销归 other。不能用负数 other 硬补齐，也不能悄悄抹掉差值。始终满足各类别 token 非负、总和等于对应的分类总输入估算；估算器不可用时所有相关百分比一起失效。

## 5. 只读快照（拟议）

以下是实现契约形状，不是当前可调用接口。字段序列化统一 snake_case。

```ts
type ContextUsageSnapshot = {
  chat_id: string;
  context_revision: string;
  config_revision: string;
  projection_id: string; // 不含原文的关联 ID；不得公开秘密派生指纹
  request_kind: 'conversation' | 'planning' | 'compaction';
  prepared_at: string;
  status: 'ready' | 'estimating' | 'unavailable' | 'stale';
  estimate: {
    method: 'model_tokenizer' | 'heuristic' | 'unavailable';
    estimator_version: string;
    allocation_method: 'token_spans' | 'normalized_estimate';
    estimated_input_tokens: number | null;
    conservative_input_tokens: number | null;
    categories: Array<{ category: string; estimated_tokens: number }>;
  };
  budget: {
    context_window_tokens: number | null;
    capacity_source: 'verified_metadata' | 'user_override' | 'unknown';
    output_reserve_tokens: number;
    safety_margin_tokens: number | null;
    input_budget_tokens: number | null;
    decision_input_tokens: number | null;
    input_occupancy_ratio: number | null;
    auto_trigger_ratio: number;
    target_ratio: number;
  };
  byte_guards: Array<{
    scope: 'planning_prompt' | 'serialized_request';
    measured_bytes: number;
    limit_bytes: number;
    exceeded: boolean;
  }>;
  last_request_usage: ProviderUsage | null;
  last_compaction_usage: ProviderUsage | null;
};

type ProviderUsage = {
  request_id: string;
  request_kind: 'conversation' | 'planning' | 'compaction';
  projection_id: string;
  config_revision: string;
  model: string;
  received_at: string;
  input_tokens: number | null;
  output_tokens: number | null;
  cached_input_tokens: number | null; // 仅已知是 input 子集时填写
  reasoning_tokens: number | null;   // 语义依供应商；不盲目累加
  completeness: 'reported' | 'partial';
};
```

`decision_input_tokens` 固定记录本次阈值所用值，正常为 I，保守估算时为上界；主占用亦使用此值。模型能力未知与计数方法未知是两个独立维度，不因缺窗口而丢掉可得估算，也不因可估算而声称窗口已知。`stale` 快照可短暂保留旧值但必须标记更新中，不能据其发请求。

后端快照只返回安全聚合数据和关联标识，不携带原始终端文本、secret、摘要正文或序列化请求。开发日志也不得为诊断计数而记录这些内容。界面不根据供应商实报反写当前草稿估算。

## 6. 操作边界（拟议）

- `get_context_usage(chat_id, expected_context_revision)`：只读当前准备结果；草稿预览如需传入，由本机桥传递，不调用外部模型。
- `compact_context(chat_id, expected_context_revision, mode, client_request_id)`：`mode = llm | quick`，具体执行/排队、状态机和幂等见核心设计。必须带修订号，防止对旧上下文提交摘要。
- 自动压缩及手动压缩共用预算、投影与提交路径；并发事件通过 chat/config/session/run 修订重新校验。
- 快照变化事件携带完整修订号，前端丢弃旧 revision；订阅断线后读取权威快照，不猜测操作成功。
- `client_request_id` 只去重同一操作，不允许压缩重试隐式重放原模型消息或终端命令。

输出 reserve 的预算变化不等于更改用户的模型最大输出配置；实现不能为“让比例变小”暗降输出。用户切换模型或接收方之后，旧实报可作为带明确来源的历史详情，但新投影必须重新计量、检查同意范围及容量来源。

## 7. 必测不变量

1. 分类和对应输入估算精确相加，所有类别非负，输出预留独立；中英混排、emoji、JSON、工具 schema、角色封装均覆盖。
2. 草稿变长、工具结果变大、模型切换、秘密重脱敏或引用失效后，快照刷新且过期投影不能发送。
3. 新响应的实报不能替换下一次请求估算；压缩请求用量不会混入主请求实报。
4. 未知窗口不显示 0%/100% 或伪造窗口；缺 usage 显示未知；超预算可显示 >100%，不能钳成 100% 后丢失超限状态。
5. 主指示与 80% 触发共用 B 和 decision_input_tokens；边界用原值判断。预留、类别占比和字节上限的分母不混用。
6. 重复文本按真实发送份数计费式计数，不按 UI 对象数量；移除未发送终端记录不改变输入量。
7. 大历史/小投影与小历史/大工具 schema 两个反例，证明不是按 UI 历史字数估算。
8. 旧 60 KiB prompt/128 条/256 KiB 发送门槛按 02/04 迁移，不残留隐藏的旧拒绝分支；单消息、独立历史内存及审计后的 transport 限制分别校验。已知 W 的字节保护触发必须独立标因，不能改写 token 占用。
