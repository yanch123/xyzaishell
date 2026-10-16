# 03 — Token 计量与接口契约

状态：v0.2.0 设计契约，不是已实现 API。接口命名用于后续实现对齐，不能把现有字节上限改名为 token 上限。

## 1. 单一权威输入

引入后端统一请求准备路径 `prepare_context`：收集当前授权范围 → 脱敏 → 选择历史/摘要/证据 → 构造消息和工具定义 → 计量 → 预算检查 → 发出同一份准备结果。普通聊天、带选文聊天和终端规划都走这条路径；前端不独立拼接另一份“用于统计”的上下文。

`PreparedContext` 是即将发送的一次模型请求输入投影。包含系统规则、工具 schema、消息角色、工具参数/结果和实际发送的引用文本；不包含整个 UI 历史、未采用的日志、私密任务文件、未经同意的附件、HTTP 头、API key。草稿预览可以纳入当前草稿和有效已授权附件，但仅本地准备，不触发模型调用。发送前必须重新准备并校验版本，不能信任过时预览。

计量对象是适配器最终发送的语义请求及已知角色/工具封装，不是简单对 HTTP JSON 字符串做 tokenization。JSON 编码字节数单独统计；服务端 chat template、隐式提示或供应商额外开销不可见时，估算误差必须保留。即使有匹配 tokenizer，也不宣称与供应商实报完全一致。

计量、预算和压缩必须共用同一规范化序列化路径。构造 token 分类清单后，发送的是同一修订投影；不得统计删减前 A、发送删减后 B、却不刷新 UI。候选输入超过既有 prompt 限制时，应在旧截断/拒绝之前检测并尝试合法压缩，避免始终只看到截断后的 60 KiB 而隐藏压力。最终发出前仍验证所有迁移后生效的字节护栏。

### 归档统计不属于模型占用

长期 ConversationArchive、UI 历史分页缓存、动态 WorkingMemory/ContextProjection 必须分别计量。归档的消息数量、磁盘字节数、页缓存大小不进入 input_tokens 或类别份额；只有当前投影实际选择的内容计入模型输入。翻阅／加载旧历史页本身不改变有效投影，不触发压缩或模型网络请求。

归档保存状态与上下文 revision 分离：消息保存成功、缓存页载入等纯存储／展示事件不能伪造新的可压缩来源；新增用户内容或明确选入旧资料才按内容与范围更新来源索引。压缩提交只改工作记忆投影，不覆盖归档原文。持久化配额、保存失败和原文未保存／过期状态需有独立字段／事件，不得用 context_truncated 代替这些不同事实。

## 2. 三种不同的数量

### 2.1 当前输入估算

`estimated_input_tokens`：当前准备请求的输入 token 估算。每次草稿、摘要、工具输出、权限、模型或请求配置改变都可失效。采用匹配模型 tokenizer 时记录其版本；没有可靠映射时采用明确标注的启发式估算（覆盖中文、代码、JSON、长数字和混合文本）。不能把固定 bytes/4 当作所有语言模型的精确事实，也不能远程发送草稿以完成计数。

启发式估算建议使用保守上界参与保护判断，展示主估算及“粗略估算”标签；若有上下界，字段需明确其为工程估计区间而非统计置信区间。未知估算返回 null 和原因，不返回 0。数字显示约数，例如 ≈24k；内部阈值计算用未格式化整数，不根据 UI 四舍五入数触发。

### 2.2 供应商上一次实报

`last_request_usage`：供应商对某个已完成/部分完成请求返回的 usage，绑定其 request_id、请求类型、模型、配置修订和投影修订。仅表示那个请求，不是下一轮上下文。输入、输出、缓存和推理相关字段按供应商语义解析；缓存输入通常是输入子集，不能重复相加。

没有 usage、流式末包丢失、供应商不支持或字段意义不明时返回 null/unknown，不补造“实际 token”。供应商通常不给类别分解，因此类别始终标估算，不把实报总量按比例分摊后叫实报。压缩摘要请求的实报归属 `compaction`，普通问答/终端规划归属 `conversation`/`planning`；摘要调用不能覆盖“上一次主请求”的展示。详情可另列最近压缩请求用量。v0.2.0 不承诺费用、价格或精确账单。

协议依据：OpenAI Chat Completions 的流式 usage 需要请求 `stream_options.include_usage`，最后的 usage 块统计该请求；流中断或取消时可能收不到它。该协议事实不能推广成所有 OpenAI 兼容服务都支持，适配器需按能力处理并保留缺失状态。见 [OpenAI 官方流式事件参考](https://developers.openai.com/api/reference/resources/chat/subresources/completions/streaming-events)（核对日期：2026-10-16）。分阶段 80%/95% 触发阈值、默认容量及类别方案是本项目设计决策，不是供应商或 Cursor 的既定行为。

### 2.3 字节护栏

`serialized_request_bytes`：最终 API 请求 JSON 的 UTF-8 字节数。

`planning_prompt_bytes`：终端规划 prompt 的 UTF-8 字节数（适用时，用于诊断与旧路径迁移对照，不代表永久维持 60 KiB 发送门槛）。

它们与 token 计量并列，不进行单位替换。现有实现中 `agent.rs` 的 `MAX_PROMPT_BYTES = 60 * 1024`、`model_transport.rs` 的 1 MiB 请求限制、`tasks.rs` 的 16 KiB 单消息与 256 KiB transcript 限制，分别保护不同资源。60 KiB 是规划 prompt 上限，不是整个最终模型请求上限；工具 schema 和外层规则还会增加开销。这些是当前实现基线，不是要求新版本原样保留的全局发送限制。v0.2.0 按 02/04 退役旧 128 条/256 KiB 全局发送门槛，改为独立历史保留；替代旧 60 KiB prompt 门槛，保留审计后的 transport 保护。压缩上下文不取消历史内存保留边界。

## 3. 预算与显示公式

统一命名：

- `context_window_tokens`（W）：解析后的配置窗口。优先级为 `user_override` > `verified_metadata` > `product_default`；产品默认 256K = 256000 tokens（十进制 K），普通缺少元数据的模型使用该默认。仅配置确实不可用时为 null，默认值不代表供应商真实能力。
- `output_reserve_tokens`（R）：继承现有 `request.max_output_tokens`，当前实现默认 4096；不新增第二个独立输出配置。
- `safety_margin_tokens`（S）：默认 max(1024, ceil(W × 0.05))；高级覆写必须验证，W 不可用时为 null。
- `input_budget_tokens`（B）：W − R − S；只有 W 为有效配置且 B > 0 时有效。
- `estimated_input_tokens`（I）：当前请求估算；使用粗略估算时保护判断可取其明确记录的保守上界。
- `input_occupancy_ratio`：I/B；主界面百分比与默认自动触发都使用该分母。
- `after_turn_trigger_ratio = 0.80`、`in_turn_trigger_ratio = 0.95`：仅控制各阶段的自动触发；不设置压缩后目标比例或以降到触发阈值以下作为成功条件。
- `turn_phase = end_of_turn | active_turn`；`effective_trigger_ratio` 分别取 0.80 或 0.95，由后端权威运行状态确定。

为了避免“看起来未达阈值却自动压缩”，如保护采用上界，主占用也使用同一上界并注明“保守估算”，不得主指示显示点估计而阈值暗用另一量。类别可显示主估算，若与保护估算不同，详情明确标记，不伪装二者精确相加。

常规自动触发仅看 token I/B 与当前阶段阈值，不设字节占用 80%/95% 软阈值。仅当某个待发送请求的 `serialized_request_bytes > transport_limit_bytes` 时，进入 `hard_byte_limit` 保护：安全点按已配置方式尝试压缩，或停止并提示请求过大；不得强行发送。它不是闲置历史检查，不会因仅字节占比高但尚未超限而发起压缩。主指示仍按 token I/B，硬限制错误独立解释。

触发原因统一为 `token_pressure | hard_byte_limit | both | manual`；`both` 表示当前 token 阶段阈值与实际待发请求字节硬超限同时满足。默认方式 LLM；用户明确选择自动 quick 时不得调用模型。预算配置无效或计量不可用时不能用字节例外绕过验证，须停止并保留输入。

输入构成占比 = 某类别估算 / 同一方法下全部类别估算。输出预留和安全余量不进入类别份额分母。构成条只代表输入，预算占用则代表当前输入与 B 的关系，两个标题必须区分。舍入后的份额可使用最大余数法使显示总和为 100%；输入为零时不做除法。

默认配置示例：W=256000，R=4096，S=12800，则 B=239104。本轮最终回复结束后达到 80% 才评估压缩；本轮进行中达到 95% 才在安全检查点评估，但无需达到某个固定压缩后比例；回复结束后或手动压缩的 90% → 85% 严格缩减在保护项完整、最终输入不超过 B 且字节硬预算满足时允许成功。分类份额与占用分母保持独立。

压缩结果记录 `benefit_tokens` 与 `benefit_bytes`（同一估算器、同一编码规则下的缩减量）：提交时两者均不得为负，至少一项严格为正，并按 02 的触发原因检查对应指标。硬字节限制原因下允许 tokens 不变而 bytes 减少，不能把 benefit_tokens >0 误设为所有模式的必要条件。结合 `budget_fits`（待发请求通过全部 token/字节硬预算）与保护集/引用验证共同判定；不能用“未降到当前触发阈值以下”否定一个合法结果，也不能用严格缩减掩盖仍超出硬预算。前后对比不能跨模型或估算器版本。

普通模型无可靠窗口元数据时使用 `product_default`，UI 标记“按默认容量估算”，仍可按配置预算正常自动压缩。不能将该默认改称已验证模型窗口。容量解析随模型/接收方变化重新执行；持久化的可选用户覆盖与解析后的有效值分开，用户未编辑时不得把产品默认值记成 user_override；无效的显式覆盖应报错而不是静默回退默认值。R+S >= W、非法值或配置不可取得时为预算配置错误，B 与占用比返回 null，不做负数、零除或自动减输出；保留草稿并指引修改配置。

供应商实际窗口可能小于默认配置。遇到 provider context overflow 时安全失败，提示调整窗口配置，不自动把失败请求重发、不重放命令、不以一次成功或报错反推精确真实容量。缺少 usage 仍独立显示未知，不影响容量来源标签。

### 3.1 阶段与本地预览的边界

`active_turn` 从用户新消息提交开始，覆盖分析、模型生成、工具循环及其暂停；只有主机确认该轮最终回复完整结束，才进入 `end_of_turn`。工具结果返回、等待审批、等待密码、预算暂停、执行结果未知，均不等于本轮结束。取消或异常不能伪造最终回复完成；它们的停止/核对状态优先，不借自动压缩恢复任务。

自动计量采用 `measurement_scope=committed_context`，只含已提交消息、已授权工具结果与工作记忆；当前输入草稿仅供 `draft_preview` 本地估算。前端如显示草稿预览，注明“含未发送草稿”，不能拿该百分比当作自动任务已采用的输入；弹层可显示后台自动检查的已提交占用。自动摘要请求绝不携带未提交草稿，草稿输入事件不触发模型网络调用。

新用户消息使旧 end_of_turn job 的 turn_id/phase/revision fence 失效，取消或丢弃结果并转 active_turn 的 95% 检查。正在发出的模型请求或命令不得打断，达阈值后等下一个安全点；若下一次发送已经违反 token/byte 硬上限则必须阻止，不能强行发出。回复后的压缩成功不自动发新消息、不恢复命令或暂停的任务。

字节不得成为任何阶段的隐藏软阈值。只有待发送请求实际超过硬上限才可触发显式 hard_byte_limit 保护，安全点处理或停止。压缩提交前重新验证 turn_id、phase 和所有原有修订 fence；回复后没有待发请求时不得借硬字节例外主动调用。

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
  turn_id: string;
  turn_phase: 'end_of_turn' | 'active_turn';
  measurement_scope: 'committed_context' | 'draft_preview';
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
    capacity_source: 'verified_metadata' | 'user_override' | 'product_default' | 'unavailable';
    output_reserve_tokens: number;
    safety_margin_tokens: number | null;
    input_budget_tokens: number | null;
    decision_input_tokens: number | null;
    input_occupancy_ratio: number | null;
    after_turn_trigger_ratio: number; // 默认 0.80
    in_turn_trigger_ratio: number;    // 默认 0.95
    effective_trigger_ratio: number;
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

`decision_input_tokens` 固定记录本次阈值所用值，正常为 I，保守估算时为上界；主占用亦使用此值。实际模型能力、配置容量来源与计数方法是独立维度；无元数据仍用默认配置计算，但不因可估算而声称真实窗口已验证。`stale` 快照可短暂保留旧值但必须标记更新中，不能据其发请求。

后端快照只返回安全聚合数据和关联标识，不携带原始终端文本、secret、摘要正文或序列化请求。开发日志也不得为诊断计数而记录这些内容。界面不根据供应商实报反写当前草稿估算。

## 6. 操作边界（拟议）

- `get_context_usage(chat_id, expected_context_revision)`：只读当前准备结果；草稿预览如需传入，由本机桥传递，不调用外部模型。
- `compact_context(chat_id, expected_context_revision, mode, client_request_id)`：`mode = llm | quick`，具体执行/排队、状态机和幂等见核心设计。必须带修订号，防止对旧上下文提交摘要。
- 自动压缩及手动压缩共用预算、投影与提交路径；并发事件通过 chat/config/session/run 修订重新校验。
- 自动尝试记录内部 `eligible_source_fingerprint` 与覆盖清单，基于可压缩来源范围而非单纯投影 revision 去重。没有新增可压缩内容时，即使占用仍超过当前阶段阈值，也不重复压缩同一批来源；候选摘要本身不能成为无限自我再压缩的新内容。细节与显式重试见 02。该指纹不泄露原文或秘密，不作为前端公开计量字段。
- 快照变化事件携带完整修订号，前端丢弃旧 revision；订阅断线后读取权威快照，不猜测操作成功。
- `client_request_id` 只去重同一操作，不允许压缩重试隐式重放原模型消息或终端命令。

输出 reserve 的预算变化不等于更改用户的模型最大输出配置；实现不能为“让比例变小”暗降输出。用户切换模型或接收方之后，旧实报可作为带明确来源的历史详情，但新投影必须重新计量、检查同意范围及容量来源。

## 7. 必测不变量

1. 分类和对应输入估算精确相加，所有类别非负，输出预留独立；中英混排、emoji、JSON、工具 schema、角色封装均覆盖。
2. 草稿变长、工具结果变大、模型切换、秘密重脱敏或引用失效后，快照刷新且过期投影不能发送。
3. 新响应的实报不能替换下一次请求估算；压缩请求用量不会混入主请求实报。
4. 无元数据模型回落 W=256000、capacity_source=product_default，显示“按默认容量估算”且允许自动压缩；实际无效/不可用配置显示 — 并暂停。缺 usage 显示未知；超预算可显示 >100%，不能钳成 100% 后丢失超限状态。
5. 主指示与分阶段 80%/95% 触发共用 B 和 decision_input_tokens；边界用原值判断，并区分 committed_context 与 draft_preview。预留、类别占比和字节上限的分母不混用。
6. 重复文本按真实发送份数计费式计数，不按 UI 对象数量；移除未发送终端记录不改变输入量。
7. 大历史/小投影与小历史/大工具 schema 两个反例，证明不是按 UI 历史字数估算。
8. 旧 60 KiB prompt/128 条/256 KiB 发送门槛按 02/04 迁移，不残留隐藏的旧拒绝分支；单消息、独立历史内存及审计后的 transport 限制分别校验。实际待发请求超过字节硬限的例外必须独立标因，不能改写 token 占用，不能把未超硬限的字节占比当作软触发。

9. 验证回复结束后或手动压缩 90% → 85% 可合法成功，没有固定压缩后比例门槛；同一可压缩来源无新增时不循环自动触发，新增可压缩内容后才重新评估。
10. 手动覆盖、可靠元数据、256000 默认的优先级正确；无效覆盖不静默回退；供应商实际容量低于默认时保留上下文并提示调整，不自动重试或重放。

11. active_turn 的 token 占用在 80% 至不足 95% 时不常规自动压缩；到 95% 后仅在安全点执行。end_of_turn 的 token 占用在 80% 触发，工具返回/审批/密码/预算暂停/未知结果均不能误判为结束。token 40%、bytes 95% 且未超硬限必须零自动调用；待发请求 bytes > hard limit 才允许明确标因的安全点压缩或停止。
12. 回复后任务不发送未提交草稿；新用户消息使旧任务 fence 失效并切到 95%；任何阶段达到硬上限都不强行发送，回复后压缩也不恢复终端执行。
