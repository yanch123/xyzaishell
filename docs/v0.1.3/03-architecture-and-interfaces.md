# 03 — v0.1.3 架构与接口约定

状态：接口契约；实现和验证进展见开发记录。以下约定描述本版接口语义；不代表对应功能或测试已经完成。字段与最终代码不一致时，应在合并前对齐并重新跑受影响测试，不能靠修改文档掩盖边界缺失。

## 1. 延续的模块与信任边界

| 模块 | v0.1.2 基线职责 | v0.1.3 调整 |
| --- | --- | --- |
| `crates/xyz-core/src/agent.rs` | 受限多轮终端任务、运行所有权、工具提议、动作展示与批准流程 | 保存本运行上下文、接纳有界更新、每轮携带、快照输出 |
| `observation.rs` | 同一可见 PTY 的有界增量观察、控制序列过滤、已知秘密脱敏 | 继续作为有界证据来源，不变成后台全文记录器 |
| `execution.rs`、`risk.rs`、`ssh.rs` | 单 AI 写入者、一次性执行许可、风险分类、同会话派发 | 原安全门保持，摘要／诊断不能产生执行许可 |
| `model_settings.rs`、`model_transport.rs` | 已保存配置的最小文字测试与安全传输 | 增加固定工具调用测试、分型结果和失败诊断 |
| `shell_integration.rs`、`shell_log_bootstrap.bash` | 当前 Shell 的临时识别与保守检查 | 将已有原因精确映射为安全错误码和说明 |
| `TerminalPane.tsx` | xterm 实例、选择、输入队列、resize、会话切换隔离 | 搜索、偏好、选择草稿入口、复制与粘贴预览 |
| `ChatPane.tsx` 与模型聊天 hook／bridge | 有界文字聊天及来源校验 | 选段可编辑预览、本次确认、文字化发送 |
| `TerminalTaskPane.tsx` 与 agent DTO | 任务状态和动作卡片 | 展示任务上下文及分层证据 |

前端显示数据不拥有 SSH 写入授权；模型生成内容、远端输出和用户选择的文字都不是宿主状态机。新增查询、搜索和诊断不能接触秘密值或直接调用原始 AI 写入 API。

## 2. 本任务上下文

### 2.1 快照 DTO

在 `TerminalTaskSnapshot` 增加 `task_context`。为旧快照／测试 fixture 兼容，消费者应能处理缺失值；新运行在宿主建立自身的上下文，不能从别的聊天填默认事实。

```ts
// 语义契约；Rust 与 TypeScript 应在实现阶段保持一致。
type TaskContext = {
  task_id: string;
  run_id: string;
  session: { session_id: string; generation: string };
  summary: string;
  confirmed_facts: Array<{
    command_id: string;
    kind: 'foreground_returned';
    exit_code: number;
  }>;
  findings: Array<{
    id: string;
    text: string;
    evidence_command_ids: string[];
  }>;
  hypotheses: Array<{
    id: string;
    text: string;
    evidence_command_ids: string[];
  }>;
  remaining_steps: Array<{
    id: string;
    text: string;
    status: 'pending' | 'in_progress' | 'blocked' | 'completed';
    evidence_command_ids: string[];
  }>;
  evidence: Array<{
    command_id: string;
    command_text: string;
    delivery: string;
    command_completed: boolean;
    exit_code: number | null;
    output_excerpt: string;
    output_truncated: boolean;
    output_gap: boolean;
    untrusted: true;
  }>;
  truncated: boolean;
};
```

`confirmed_facts` 由宿主根据匹配的动作前台返回事件生成；模型不能写这个数组、伪造退出码或提交自己的输出摘录。`findings` 是有依据的模型解读，`hypotheses` 是待核实推测，`summary` 和步骤也属于模型报告。所有类型都不携带执行许可。

### 2.2 更新通路

- `terminal_action` 可以增加可选的上下文更新字段，保持旧版仅含 `command_text` 的提议可识别。命令本身仍走原解析、风险、展示、批准和派发流程。
- 增加仅更新摘要的 `task_context_update` 提议。它不写 SSH、不完成任务、不产生动作批准；接纳后进入下一次受限模型轮次，仍受已有轮数、时限和取消机制约束。
- 每个完整响应仍只接受单个允许的工具提议，不因“只是更新摘要”而允许多动作批处理或未知工具。
- 模型只能提出摘要、解读、假设和步骤。宿主验证结构、长度、引用和运行身份后一次性提交，失败保留原快照并返回安全错误，不执行夹带命令。
- 下一轮请求携带最新上下文和本轮观察。不能依赖模型自己记得上轮，也不能把未验证的工具流片段直接作为事实回传。

模型有上下文更新能力不表示有 Skills、MCP 或动态工具注册。本版工具集合仍由宿主固定定义。

### 2.3 证据规则与容量

拟定条数上限：证据和宿主事实各 16 项，解读和假设各 8 项，步骤 12 项。文本、ID、引用数量和总 JSON 字节量还必须有实现常量及边界测试；最终值以整合代码和测试为准，不允许无界累积。

接纳解读时，引用必须指向本运行保留的、具有非空输出的证据。假设允许没有证据，但必须作为假设展示；已有引用仍需有效。步骤与摘要不因引用有效就转变为已确认事实。截断证据被替换或移除时，应清理失效引用或拒绝造成悬空引用的更新。

证据文本来自本任务已授权的增量 PTY 观察，保留 `output_gap` 和 `output_truncated`。可见 PTY 也可能有用户输入或后台程序输出；`command_id` 是宿主关联，不保证摘录是该命令排他的 stdout。远端还可能伪造文字，因此“有引用”不等于“结论为真”。

证据、模型更新、UI 快照与后续请求都使用既有已知秘密脱敏。模型配置 Key、任务秘密值和本机秘密文件路径不能借摘要泄露。脱敏不足不能被描述成完全隐私保证。

### 2.4 生命周期与持久化

上下文存在于 `AgentManager` 当前运行记录，身份至少由任务 ID、运行 ID、SSH 会话 ID 和 generation 共同限定。取消、失去所有权、会话改变或进入终态后，旧异步响应不能修改新的运行。

本版不增加独立磁盘记忆 schema。若沿用用户已启用的聊天历史保存任务说明，仅保存有界、分层标记的惰性文字摘要，继续脱敏；恢复后只读，不解析成任务状态、不恢复就绪证据、不重放动作或批准。日志记录仍独立且默认关闭。

## 3. 终端选段与文字聊天

### 3.1 本机草稿

前端维护内存中的选段草稿，每聊天最多一份。语义字段包括：

```ts
type TerminalSelectionDraft = {
  text: string;
  source: {
    session_id: string;
    generation: string;
    host: string;
    port: number;
    username: string;
  };
  edited: boolean;
  truncated: boolean;
  // 内部还需能判断预览内容是否改变；不是服务器真实性签名。
};
```

选择来源从当前 xterm 对应的会话快照取得，不能根据提示符文字或选段中声明的主机名推断。选段与普通聊天草稿都不写进终端偏好存储。

### 3.2 本次同意与发送核验

确认绑定当前预览内容、固定来源、目标聊天、模型 `revision`／`destination`。编辑后撤销确认；模型重载、换目标或重连后，旧确认不能沿用。实现可使用本地修订或等价状态比较，不应把秘密裸哈希放入可见 DTO。

提交前核验：

1. 当前在线 SSH session／generation 与草稿一致。
2. 当前聊天来源与该目标相符且不是恢复的只读历史。
3. 当前模型配置与本次核对版本／地址一致。
4. 最终内容不为空、不超限，聊天容量和执行占用允许发送。
5. 本次同意仍有效，提交没有被重复启动。

复用现有 `modelBridge.send(chatId, text)` 和普通聊天 IPC；不为本版新增选段专用 IPC。发送文本带明确来源与“不可信、可能经用户编辑的终端摘录”标签。界面确认及来源绑定由选段发送流程承担，不能宣称普通字符串 IPC 已独立认证文字的远端来源。

宿主继续对普通聊天作现有模型接收方、容量、运行占用和脱敏检查。应在用户提交时固定要发送的文本与配置相关状态，防止异步回调读取后来变化的草稿。选段中的伪造指令不能改变普通聊天没有执行工具这一边界。

本流程不会调用 `start_terminal_task`，也不调用 `sshBridge.write`；用户另行明确启动终端任务时，才进入原有执行授权流程。

## 4. 终端工具与偏好契约

### 4.1 搜索

使用与当前 xterm 版本兼容的搜索插件，在本地终端 buffer 查询。搜索实例与 terminal instance 同生共死；generation 切换销毁旧实例时，旧命中和事件回调一并失效。

按键分流只拦截明确的搜索／复制粘贴快捷键，保持 `Ctrl+F` 和 `Ctrl+C` 的终端语义。搜索框的输入事件不能进入 `TerminalInputQueue`。

### 4.2 偏好

本机 key 为 `xyzaishell.terminal.v1`，版本 `1`；至少保存字号与回滚行数。解析必须先验证对象形状、版本和有效数值范围，禁止执行、插值或加载任意脚本。

- 字号：10–28 px，默认 14。
- 回滚：500–20000 行，默认 5000。
- 解析失败采用安全默认值；持久化失败提示实际未保存，终端不崩溃。
- 应用设置修改 xterm options，再沿用既有 fit／resize，不重建 SSH 或新建 PTY。
- 存储内容不得包含模型配置、选择草稿、终端输出、任务摘要或秘密。

### 4.3 剪贴板与输入

复制从当前选择取文本，剪贴板写入仅响应用户主动操作。粘贴读取本机剪贴板后进入预览；确认绑定 session／generation，最后通过已有手动输入队列发送，不额外附加回车。

危险动作审批门仍只管理 AI 提议动作；用户手动粘贴的后果须在 UI 明示。不要把手动粘贴伪装成经过 AI 风险分类的安全动作。原始回车／控制字符如何显示、是否禁止特定控制字符，需由代码与针对性测试一致限定；最低要求是用户能核对会发送的内容，且不能在确认失效后写到新会话。

## 5. 模型诊断接口

沿用 `test_model_config`，扩展请求和结果。旧客户端缺省 `kind` 时按文字连通测试处理。

```ts
type ModelTestKind = 'chat_connectivity' | 'task_capability';
type ModelTestRequest = {
  test_id: string;
  file_revision: string;
  destination: string;
  model: string;
  consent: boolean;
  kind?: ModelTestKind;
};
type ModelTestResult = {
  test_id: string;
  file_revision: string;
  destination: string;
  model: string;
  kind: ModelTestKind;
  outcome: 'supported' | 'unsupported' | 'inconclusive';
  success: boolean;
  error: ApiError | null;
};
```

`success` 仅表示所选测试本次满足条件；`outcome` 给出细分判断。UI 不能只看 `success: false` 就显示“不支持”。所有结果与原测试 ID、类型和已保存配置绑定，晚到的旧结果不能覆盖当前配置的结果。

`task_capability` 使用固定提示和模拟工具声明，检查恰好一次完整调用、精确名称、严格参数 schema、结束原因及响应上限。模拟工具没有执行实现，不接入 `AgentManager`／`ExecutionGate`／`SessionManager`，不复用真实终端动作作为“测试执行”。

分类原则：有效完整调用为 `supported`；完整工具响应违反适配器要求为该次 `unsupported`；纯文字未调用、认证／网络／超时／取消等为 `inconclusive`。流式未完成不能被部分参数提前判为成功。这里检验的是实际任务适配器经 SDK 归一化后的响应，不是原始 OpenAI wire protocol 认证；例如 SDK 可能把带完整工具调用的原始 `stop` 结束原因归一化为 `ToolCalls`，应与真实适配器一致处理。缺少有效结束、长度截断或内容过滤不能当作通过。测试取消、取消先于启动和重复提交继续使用现有测试 ID 机制，不自动重试网络请求。

## 6. Shell 诊断契约

保留 `IntegrationStatus` 结构、阶段和 revision 语义，通过现有 `error` 返回特定原因。已知 bootstrap 拒绝原因映射为具体 `SHELL_UNSUPPORTED_*` 类错误码和安全说明；实际完整码名应与实现及测试一致。

至少区分：`unsupported_bash`、`namespace_conflict`、`unsupported_options`、`unsupported_trap`、`dynamic_prompt`、`unsupported_hook_attributes`、`missing_tools`。未知远端原因只映射为通用未知错误，不将任意远端文本直接放入用户提示或命令建议。

错误分类不修改 hooks 或 Shell 配置，不重启／重连，不开启日志。握手超时、证据缺失、旧 session、协议乱序仍按原状态机暂停自动输入。此诊断也不是远端环境的安全审计。

## 7. 兼容与变更控制

- 不改变一进程一 SSH、一 PTY、一任务写入者及明确危险批准。
- 对新增可选 DTO 字段处理缺失值；不能通过兼容默认值制造已通过测试、已有证据或有效同意。
- 工具 schema、参数解析和 UI 字段同步修改；未知字段／超限／畸形 JSON 失败关闭。
- 新偏好 key 独立于历史、日志、模型配置，不迁移或覆盖这些数据。
- 最终实现若改变上述字段、测试分类或数据去向，需要同时更新文档与用例；若改变已确认功能或安全范围，先获得确认。
