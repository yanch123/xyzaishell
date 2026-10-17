# 02 — 架构与接口变更

状态：**以下接口为拟议增量，尚未编码**。继续使用现有 ledger → projection → prepared request → 预算准入／原子提交路径，不替换整体架构。

## 1. 现状与最小修改点

| 位置 | 当前已实现行为 | v0.2.1 设计变更 |
| --- | --- | --- |
| `context.rs::prepare_serialized_context`／`meter_planning_json` | 七类；部分摘要／证据分类依赖文本前缀和递归键名 | 从实际构造来源计量，增加独立展示分类，不靠正文猜测 |
| `context.rs::prepare_context` | `other` 加 `16 + 8 × fragments.len()` 工程余量 | 按实际请求／消息／工具结构计算协议开销，与分类拆片数量脱钩 |
| `tasks.rs::chat_request`、`agent.rs::planning_request` | 职责、规则混合在同一 preamble | 保留实际提示与规则，构造时明确片段边界和来源 |
| `tasks_context.rs::RuntimeContext::project` | ledger 投影为消息并添加摘要／证据说明文字 | 同步产生来源侧表；说明包装独立记账，不用前缀反推来源 |
| `tasks_context.rs::sync_messages` | 所有用户消息设为 `Required` | 分离通用保护、LLM 候选与 quick 额外保护；不简单删除保护标记了事 |
| `context.rs::llm_candidate`／`commit_compaction` | 约束 source_ids 混入 evidence_refs，再写 dependency_ids | 分离可追溯来源与必须保留的证据／执行依赖 |
| `context.rs::quick_candidate` | 每个可压缩来源取约 160 字符摘录 | quick 来源集中排除用户输入和全部现用摘要，并在候选／提交再次验证 |
| `ContextIndicator`／`src/lib/context.ts` | 七类平面数组 | 五类 DTO、当前对话子项及协议明细；沿用 hook 和 IPC 的竞态保护 |

现有内部 `ContextCategory` 参与保护、证据处理与脱敏路径，不能为五类 UI 直接改名或合并。新增展示分类，与 `Trust`、`Protection`、`Completeness`、来源身份和保留策略分开。尤其不能因工具输出显示在“当前对话”就让它获得 `UserInput` 信任。

## 2. 主机来源标记

拟增进程内 `RequestSourceMap`，与最终 `PreparedContext.serialized_body` 一起产生和校验，不增加发送给供应商的协议字段，也不增加持久化的原文副本。

- 来源记录绑定最终 JSON 路径；合并字符串内记录 UTF-8 片段范围；记录主机生成的 source／segment ID、展示类别与对话子类。JSON 路径定位来自构造器，不递归匹配用户自定义同名键。
- `chat_request`、`planning_request` 和 `COMPACTION_RULES` 构造时明确划出职责／模式要求与固定安全规则／输出契约。可以保持拼接后的提示文本不变；动态 `availability` 等主机只读状态单独标当前对话的其他实际输入。工具 schema 仅从实际 tools／functions 字段登记。
- `project` 按 ledger 来源标出摘要、用户／助手、执行结果、选入资料；规划 JSON 的 `conversation_history_untrusted`、`user_instruction`、观察、动作及 task_context 逐个由对应生产者登记，嵌套摘要仍靠 segment 来源识别。
- 每个实际内容位置只登记一次；同一内容确实发送两份，则计两份。未选入的 UI 历史、归档页、日志和凭据不进清单。用户伪造前缀、JSON 键或 source ID 不可创建主机来源。
- 请求变形、脱敏或适配器序列化后须同步重建／映射侧表，校验覆盖、范围、无重叠及与最终 body 的一致性。来源信息缺失或冲突时分类标不可用，不猜类别或输出伪 0。独立完整请求总量护栏仍可靠时，可以显示总量并通过正常准入后发送；若总量也无法可靠计量，必须停止发送。展示不可用不能跳过 token／字节硬预算。

来源表供计量与保护核验，不返回原文给前端。已有摘要／证据说明文字可继续用于模型提示，但不再作为分类依据。实际保护项校验基于主机 ID／片段映射，不能仅凭 `after_text.contains(...)` 认定原保护内容仍在正确来源位置。

## 3. 协议开销和对账

本版选用**内容与协议分账**，不将旧 `other` 任意按比例分给系统指令／固定规则，不新增第六类内容。

令五类内容估算为 `C_i`，`C = Σ C_i`；`O = 已知包装估算 + 未知模板工程余量`。`I = C + O`，启发式路径下 `estimated_input_tokens = conservative_input_tokens = decision_input_tokens = I`，主占用为 `I / B`。未来如果提供不同的保守上界，必须另列差额及口径，不能在五类中暗摊。

- 已知包装包括实际角色／关联标识、消息／工具结构、模型输入里的结构键与分隔、必要转义，以及主机插入的摘要／证据说明包装；按最终结构各计一次。JSON 字节护栏仍精确统计整个最终编码 body。
- 模型输入的语义结构与 HTTP JSON 编码不是同一个 tokenization 对象：不把 HTTP headers、API key、URL 或纯传输设置当输入；不重复把同一字符串内容与它的整体 JSON 文本各算一次。已作为模型输入的嵌套 JSON，其键／转义按该实际输入形态计算。
- 未公开服务端 chat template 只能记明确版本化的工程余量。其函数参数是实际请求、消息及工具结构数量等，不是 `fragments.len()`；同一 wire 请求只改变来源拆分，O 和 I 必须不变。计量先确定规范语义单元，再把分类小片段分配回该单元，避免每片单独向上取整而放大总量。
- 估算器更新为新版本（拟 `unicode-conservative-v2`），协议算法记录 `protocol_structure_v1`。旧模型校准不得跨估算器版本沿用；校准仍只调整已绑定接收方／模型／版本的安全余量，不伪造类别实报。
- 五类的显示百分比 `C_i / I`，协议开销为 `O / I`；子类 `Σ child = current_conversation`。I=0 时不做除法，未知时返回 null。显示舍入误差仅影响显示，不改整数预算或自动阈值。

## 4. 拟议用量接口

保留 `get_context_usage`、`get_context_compaction`、`compact_context`、`cancel_context_compaction` 的参数与现有状态／事件字段，仅扩展 `ContextUsageSnapshot.estimate`。示意类型中的名称待编码时同步 Rust／TypeScript：

```ts
type DisplayCategory = 'system_instructions' | 'tool_definitions' |
  'fixed_rules' | 'history_summary' | 'current_conversation';
type DialoguePart = 'user_messages' | 'assistant_messages' | 'tool_calls' |
  'tool_results' | 'other_inputs';
type ContentEstimate = {
  category: DisplayCategory;
  estimated_tokens: number | null; // 内容；不含下列协议开销；未知为 null
  children?: { category: DialoguePart; estimated_tokens: number | null }[];
};
type EstimateV2Fields = {
  category_schema_version: 2;
  categories: ContentEstimate[]; // 五项固定顺序；确认无来源为 0，计量未知为 null
  content_input_tokens: number | null;
  protocol_overhead: {
    method: 'protocol_structure_v1';
    known_framing_tokens: number | null;
    template_allowance_tokens: number | null;
    estimated_tokens: number | null;
  };
  // 原有 method / estimator_version / allocation_method 与总输入字段继续保留
};
```

来源计量不可用时，五类及受影响子项为 null，C 为 null，不显示分类份额或已成立的对账等式；0 只用于已确认不存在的来源。独立总量计量可靠时保留 I／O 和总预算占用，并明确“分类不可用”；总量不可靠则 I／占用为 null 且停止发送。未知不能显示为五个“本次未发送”。

`ContextIndicator` 不再以五类之和冒充全部输入；用后端 I 作“占输入”分母，详情对账 C+O。`useContextUsage` 的 identity／revision floor、草稿本地预览、fresh、unknown IPC 回执核对、重复提交和取消机制保持。新旧 DTO 不做静默七类→五类映射；若遇到旧 schema，提示计量暂不可用并刷新。现有历史／压缩记录不被重新解释为新分类，也不因这个 DTO 变化开启历史保存。

## 5. 模式化保护与来源谱系

拟在现有保护闭包上增加显式用途：`projection_protection(mode)` 与 `retention_protection()`（具体命名可调整）。前者决定本次谁能被摘要替代；后者保护仍被活动投影／执行／证据实际需要的数据。不能把 quick 的保守限制直接当成永久磁盘／RAM 保留政策。

| 内容 | LLM | quick |
| --- | --- | --- |
| 系统／固定规则／工具定义、当前轮及最近两完整轮、显式 Required、未闭合工具单元、主机执行边界及依赖闭包 | 原样保护 | 原样保护 |
| 其余较早用户消息 | 允许进入摘要来源 | 原样保护 |
| 不属通用保护集的现用摘要 | 可参与后续语义整理，保留来源谱系和不确定性 | 全部原样保护 |
| 其余旧 AI 回复及闭合结果 | 可总结 | 可按现有确定性规则摘录 |

不再由 `sync_messages` 永久把所有用户消息设 Required；当前／近期保护随主机轮次计算。显式强制保护不会因选择 LLM 消失。quick 在来源选择、候选验证、原子提交三处校验禁截用户／摘要，不能只依赖 UI 模式选择。来源指纹包含模式和保护策略版本；无新增离开保护窗口的原始来源时，不因分类重算、投影 revision 或摘要新 ID 重复自动调用。分块／合并来源覆盖仍有界。

区分两种引用：

1. **追溯谱系**：`coverage.source_ids`、`user_constraints[].source_ids` 与摘要的 source_ids，仅说明来源。coverage 必须恰好是本作业直接来源；约束引用必须属于本作业直接来源或受保护锚点，不能混入 candidate.evidence_refs／segment.dependency_ids。主机另外保留有界祖先谱系，可指向完整来源、可恢复归档页或明确 tombstone（含覆盖范围／可用性），不强制原文继续驻留。
2. **保留依赖**：真正用于主机执行解释或仍被引用的证据锚点／工具配对。使用现有 dependency_ids 路径，只能引用主机认可的证据／执行来源，继续保留闭包；模型不能通过任意 source ID 创造执行权限，也不能通过省略依赖撤销主机保护。缺失或不可用的必需依赖应失败，不能用谱系 tombstone 冒充完整证据。

现有闭包锁住被覆盖来源的索引／原文内存，不会自动将它重新放进请求投影。本版解除纯约束追溯对旧用户原文 RAM 的永久占用；不能反过来移除仍需要的真实命令证据依赖。

重复 LLM 时，新作业将上一份摘要 ID 作为直接来源；旧摘要 JSON 内的源 ID 只属于祖先谱系。旧摘要作为不透明来源文本提交，不递归把其中 ID 塞进新 coverage 或新别名表；新约束指向这份直接摘要 ID，主机负责沿谱系追溯。新别名表仅对应本作业直接来源及受保护锚点；分块／合并的 coverage 仍是这些直接来源的合法并集，不把祖先展开成重复覆盖，不重新激活被覆盖原文。真实证据引用由主机保留依赖提供可用锚点，不能靠接受任意祖先 ID 绕过引用验证。

`llm_candidate` 分别校验约束来源、普通来源覆盖和证据引用；`commit_compaction` 只将确需保留的引用写入 dependency_ids，摘要来源索引另存。摘要仍是 ModelInterpretation，不因引用用户或主机事实提高信任。已有摘要来源属性不全时 quick 全保留；持久化新增元数据只能走现有显式历史保存路径，旧记录仍只读，不自动恢复 live ledger 或权限。

LLM 固定契约保留目标、限制、纠正、事实／证据、疑点和步骤；结构覆盖与保护校验只是机械检查，不是语义证明。压缩期间的新消息、配置／目标／会话／脱敏变化和取消继续使 fence 失效。最终候选重新构造完整请求、计量、检查真实收益和全部硬预算，成功才在锁内原子替换；失败／取消／过期均不提交部分摘要，不触碰 ExecutionState、归档原文或规划计数。
