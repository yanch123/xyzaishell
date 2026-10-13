# 03 接口与事件契约（design-v0.1）

> 状态：拟议设计，不是可编译 SDK、已实现 API 或用户冻结协议。下面的类型片段用于对齐 Rust、TypeScript 与测试；实现前仍需评审命名、容量、IPC 编码及依赖版本。已确认客户端目标为 Windows 10/11 x64 与 Ubuntu 22.04/24.04 x64；提案技术栈为 Tauri 2、Rust/Tokio、React/TypeScript、xterm.js、russh 与 Rig，共享核心与平台适配分离。

相关文档：[架构](02-architecture.md)、[配置](04-configuration.md)、[安全边界](05-security.md)、[测试](06-testing.md)。

## 1. 必须保持的不变量

1. 一个应用实例最多连接一个服务器，只有一个可见 SSH shell PTY。AI 与人工输入作用于这个相同 PTY，不能用后台 `exec` 通道代替可见终端执行。
2. 可有多个聊天；同一时刻最多一个 AI 任务持有写入资格。其他聊天可继续讨论，但不能同时派发终端动作。任务隔离不等于远端 shell 的 cwd、用户、环境变量或进程隔离。
3. 用户手动输入不纳入 AI 任务优先级、自动暂停/恢复或接管机制。软件不负责推断用户何时停止操作，不承诺人工优先；用户自行避免冲突。
4. SSH 写传输只有一个拥有者，通过有界队列按已接受的帧顺序写入。串行传输只避免本机并发写对象损坏，**不保证人工半行与 AI 命令在远端不会混合**。
5. AI 只能提出受限动作；每次实际写入都经过 prepare → 本机策略/必要批准 → dispatch。模型不能获取原始写句柄、调用人工输入接口或批准自己的动作。
6. 高风险批准是单次、动作绑定的后端记录。它不由 prompt、终端输出、模型 tool-call ID 或前端随手传来的 `approved: true` 授予。
7. “停止 AI”和“中断当前远端程序”是不同操作。任何本地取消都不能伪称已停止远端进程或撤销副作用。
8. 断线、超时、崩溃等造成结果不明时返回 `unknown`，不自动重放命令；重连生成新的连接代次，旧动作与批准不得复用。
9. v1 不解决任意交互终端的可靠状态判断。未知时，只能让用户处理或确认一个明确的下一动作；不能以确认按钮制造成功、完成或退出码。
10. 任务机密只由后端按所属任务及快照解析。模型看到字段名/是否存在；没有任意文件读取、读机密值或盲目 shell 模板替换工具。

## 2. 基础类型与边界

所有跨 IPC 数据拟使用 JSON 可序列化的 DTO，字段统一 `snake_case`；二进制输出编码按后续性能验证决定，不能在未定义时把字节误当 UTF-8 字符串。本文以 base64 表示二进制字段，不要求使用 Rust 底层类型名作为公开 API。

```ts
// 文档类型示意；不是仓库中的实现。
type OpaqueId = string;         // 后端生成；不作为客户端可指定的文件路径
type Revision = string;         // 十进制非负整数字符串，避免 JS u64 精度丢失
type UtcTime = string;          // RFC 3339 UTC，不能用于事件排序

type SessionRef = {
  session_id: OpaqueId;
  generation: Revision;        // 每次新建 SSH/PTY 连接递增
};

type ApiError = {
  code: string;
  safe_message: string;
  retry: "never" | "safe_read_only" | "after_user_action";
  request_id: OpaqueId;
  category?: "model_config" | "ssh_config" | "task_config";
  field_path?: string;
  line?: number;              // 仅位置，不带源行内容
  column?: number;
};

type Reply<T> =
  | { ok: true; request_id: OpaqueId; value: T }
  | { ok: false; request_id: OpaqueId; error: ApiError };

type TerminalSize = { cols: number; rows: number };
type SecretFieldStatus = { name: string; present: boolean };
type SecretRef = {
  task_id: OpaqueId;
  field_name: string;
  snapshot_id: OpaqueId;
};
```

- `session_id`、`task_id`、`run_id`、`action_id`、`approval_id`、快照 ID 均由后端生成。v1 拟采用随机 UUID；不得用模型提供的名称创建磁盘路径。
- 客户端可以提供一次请求的 `request_id` 用于关联和去重；它没有授权含义。相同 ID、不同请求体返回 `REQUEST_CONFLICT`。去重状态仅在本进程规定的保留窗口有效；进程重启后不能据此保证远端恰好执行一次。
- `SessionRef` 必须与当前活动连接匹配。旧 UI、旧工具结果与迟到异步回调不得写入新连接。
- 普通前端 DTO 不包含 API Key、SSH 密码、私钥内容、解密机密、原始认证错误或完整配置文件。
- 模型工具调用的任务身份由后端运行上下文绑定；不能让模型通过参数中的另一个 `task_id` 获得跨任务能力。

### 2.1 平台适配契约

以下是后端边界的文档示意，不是模型工具或开放文件系统 API：

```ts
type Platform = "windows" | "linux";
type CredentialBackend =
  | "dpapi_current_user" | "linux_secret_service" | "session_only";
type CredentialAvailability =
  | "available" | "locked" | "unavailable" | "access_denied";
type CredentialPurpose = "ssh_password" | "ssh_key_passphrase";
type PlatformStatus = {
  platform: Platform;
  credential_backend: CredentialBackend;
  credential_availability: CredentialAvailability;
  persistent_credentials: boolean;
};
```

| 后端端口 | 输入／结果 | 约束 |
| --- | --- | --- |
| `AppPaths.resolve` | 平台和启动环境 → 已验证的配置／状态／缓存根目录 | Windows 系统目录；Linux XDG；不按工作目录搜索，也不执行本地 shell |
| `RestrictedFileAccess.open` | 受控配置对象或用户选定私钥 → 后端文件句柄 | Windows 绝对路径／ACL／重解析点；Linux POSIX 路径／UID／mode／链接与文件类型；模型不能传任意路径 |
| `CredentialStore.status` | 无 → `PlatformStatus` | 区分可用、锁定、缺失／不可达和拒绝；状态不是解密成功保证 |
| `CredentialStore.store/resolve/remove` | 连接 ID、`CredentialPurpose`、受控凭据更新／内部存储描述符 → 仅后端秘密句柄或安全结果 | 身份与用途绑定；Windows DPAPI 描述符或 Linux keyring 引用不能互换，不对模型／通用 UI 暴露 |
| `CredentialSession.set/clear` | 专用认证 UI 输入、连接 ID、用途 → 仅进程内存凭据 | 仅用户显式选会话内存模式；不进入 TOML／SQLite／日志／IPC 广播 |
| `ConfigFileStore.save` | 文件类别、`expected_revision`／内容版本、新内容 → 新修订或安全错误 | 稳定跨进程锁、受限同目录临时文件、平台替换／同步、冲突与保存结果未知分开 |

`secret_protection` 与 `protected_*`／`*_secret_ref` 的持久字段约束以[配置规范](04-configuration.md#4-ssh-连接-schema-与凭据保管)为准。当前没有已发布 schema；不能把 Windows 密文从文件直接复制为 Linux 引用。

安全存储故障不自动选择 `session_only`；由专用设置／认证 UI 明确选择后才能使用，且不修改任务审批策略。SSH 登录密码在连接认证阶段进入 SSH 库，不通过尚未建立的 PTY 键入。任务秘密的远端交互使用仍遵循原有 `SecretRef`／安全输入适配器边界。

Windows／Linux 适配器运行同一组契约测试；平台能力缺失需显式返回错误，不能通过本地命令工具、明文文件或另一个账号绕开。

## 3. 前端 → 后端 IPC

下列命令是拟议方法名。Tauri 允许前端调用 Rust commands，持续数据可使用 Channels；实际权限仍要按窗口 capability 限制。参考 [Tauri 调用 Rust](https://v2.tauri.app/develop/calling-rust/)。

### 3.1 配置和连接

| 方法 | 关键请求 → 响应 | 前置条件 / 失败语义 |
| --- | --- | --- |
| `get_platform_status` | 无 → `PlatformStatus` 与设置 UI 专用路径摘要 | 无秘密或 keyring 内部引用；不是模型工具 |
| `get_config_status` | 无 → 三类配置的版本、状态和脱敏字段摘要 | 不返回机密值或可复用密文；本机路径仅给设置 UI |
| `reload_model_config` | `expected_snapshot_id?` → 新模型快照摘要 | 只读启动时选定路径；失败不替换旧快照，新快照用于后续 run（包括同一任务的新 run） |
| `probe_model_compatibility` | `model_snapshot_id` → 各项测试状态 | 使用已确认目标；不发送实际终端/任务数据；未测不能记为通过 |
| `list_connection_profiles` | 无 → 档案展示字段与 `credential_present` | 无密码/口令/私钥正文 |
| `save_connection_profile` | `expected_revision, profile, credential_update?` → 新修订号与档案 ID | 仅设置 UI；遵守已选文件级保护模式；秘密只在专用请求短暂传入；原子写／冲突／保存未知检测 |
| `set_credential_mode` | `expected_revision, mode` → 新修订与影响摘要 | 仅设置 UI；文件级显式变更，先展示受影响档案；不默认删除原凭据，失败不明文回退 |
| `set_session_credential` | `connection_id, purpose, credential` → 本进程可用状态 | 专用认证 UI；仅显式会话内存模式，不持久化、不广播；下次启动需重填 |
| `connect_server` | `connection_id, profile_revision, size` → `SessionRef, status=connecting` | 实例已有活动/连接中会话则 `SESSION_BUSY`；异步结果经事件返回 |
| `confirm_host_key` | `challenge_id, expected_fingerprint, decision` → 已接受或拒绝 | 仅信任确认 UI；指纹/主机/端口/挑战必须匹配；不能由 AI 调用 |
| `disconnect_server` | `SessionRef` → `closing` 或 `closed` | 不保证远端程序结束；未证实结果标未知 |
| `resize_terminal` | `SessionRef, size` → 已接受 | `cols/rows` 拟限制 `1..1000`；同连接待发送 resize 可合并 |

`connect_server` 内部顺序：校验档案 → 连接与主机密钥验证 → 认证 → 请求 PTY → 请求 shell → 发布 `session.connected`。任一步失败都不能发出伪成功事件。未知主机密钥和已知密钥变更需要不同 UI；变更不能被普通“重新连接”自动接受。

russh 提供 PTY、shell、字节数据与尺寸变化等通道操作，拟由单一 `SessionActor` 封装；模型和前端不得直接持有 russh 句柄。这里不假定某个未锁版本的 Rust 方法签名已可编译。参考 [russh Channel API](https://docs.rs/russh/latest/russh/struct.Channel.html)。

### 3.2 人工终端输入

| 方法 | 请求 → 响应 | 约束 |
| --- | --- | --- |
| `send_user_input` | `SessionRef, input_id, encoding, data` → `accepted, transport_seq` | 仅可信终端 UI，且确由其人工输入路径触发；有界数据，未接受时零字节入队 |
| `interrupt_remote` | `SessionRef, request_id` → `accepted, method` | 独立用户动作；v1 拟发送 Ctrl+C 对应字节，结果未保证 |

- xterm.js `onData` 转发普通键入/粘贴；`onBinary` 如启用必须保留字节语义。返回输出调用 `Terminal.write`，不能为了显示模型文本而调用会产生输入事件的 `Terminal.input`。参考 [xterm.js Terminal API](https://xtermjs.org/docs/api/terminal/classes/terminal/)。
- UI 不得把 AI 生成文字、远端控制序列、自定义链接或聊天里的“执行”伪装成手工 `onData`。IPC capability、固定 UI 路径和后端调用上下文共同隔离；布尔字段 `source=user` 不是可信证明。
- 后端不因人工输入要求释放 AI owner、不自动暂停/恢复 AI、不启动键盘静默计时器。每个按键也不会被解释成可靠的 shell 语义状态变更。
- 已接受帧按 FIFO 处理；无人工优先队列。传输可能分成多个 SSH 包，但不能由两个本机 writer 并行交织写同一对象。容量不足应明确提示，没有“看似输入成功但字符被静默丢弃”。
- 用户输入的 Ctrl+C、Enter 等就是正常终端输入。`interrupt_remote` 按钮只提供明确入口；不是自动取消任务的副作用。

### 3.3 聊天、任务与动作审核

| 方法 | 关键请求 → 响应 | 约束 |
| --- | --- | --- |
| `create_chat` | 可选展示标题 → `chat_id, task_id` | 同时创建稳定任务身份与专属配置；不创建新的 SSH shell |
| `send_chat_message` | `chat_id, text` → 消息 ID | 普通讨论不取得终端写资格；模型流式输出不直接执行 |
| `start_task` | `task_id, SessionRef, user_instruction` → `task_id, run_id, state` | 后端原子获取唯一 AI owner；已有 owner 则 `AI_OWNER_BUSY`，不隐藏排队后自动执行 |
| `get_task_status` | `task_id` → 当前/最近 run、阶段、owner、待办动作、字段状态 | 脱敏；每项远端结果都保留证据/未知状态 |
| `reload_task_config` | `task_id, expected_snapshot_id` → 新快照与字段状态 | 只载入该任务文件；安全边界切换快照，旧 prepared/批准失效 |
| `ack_action_display` | `action_id, display_revision` → 已记录 | 仅说明 UI 已展示精确预览，不构成执行批准 |
| `decide_action` | `approval_id, action_id, decision=approve/deny` → 单次批准状态 | 只接受实际审核 UI；不接受聊天文字/模型输出替代；批准绑定详见第 4 节 |
| `cancel_task` | `task_id, run_id` → `cancelling/cancelled` | 停止新模型步骤和后续 AI 派发，撤销未消费批准；不自动发 Ctrl+C |

`chat_id` 对应一个稳定 `task_id` 和任务配置文件。`start_task` 在该任务中创建一次 `run_id`，绑定本次启动时模型快照及 SSH 连接；同一聊天再次执行/显式重新启动仍使用原 `task_id` 与配置，只生成新 `run_id`。等待配置/批准仍在原 run 内；取消、结束或重连后不会自动恢复，新 run 也不会重放旧动作。执行阶段拟为 `preparing`、`running`、`waiting_config`、`waiting_approval`、`needs_user`、`cancelling`、`cancelled`、`completed`、`failed`。这些是 AI 流程状态，不能代替远程命令状态；`completed` 表示本次 run 流程结束，若仍有未知远端结果，摘要必须显示“远端结果待核实”，不能宣称目标成功。

owner 绑定 `(task_id, run_id)`，在等待用户/配置时仍属于原 run。旧 run 的迟到回调不得控制新 run。切换聊天页面不转移 owner；取消必须等本地 in-flight 写操作到可判定边界，再释放 owner，不能同时让新任务写入。上一动作的远端结果仍未知时，新任务也必须面对当前未知终端状态，不能把 owner 释放当成 shell 空闲。

## 4. AI 动作准备、批准与派发

### 4.1 动作类型

```ts
type ActionProposal =
  | { kind: "shell_command"; command_text: string }
  | { kind: "terminal_text"; text: string; append_enter: boolean }
  | { kind: "terminal_key"; key: "enter" | "tab" | "escape" | "ctrl_c" }
  | { kind: "secret_input"; secret: SecretRef; adapter_id: string };

// 模型只提供字段名；后端从本次 run 补全 SecretRef，不能由模型指定归属。
type ModelActionProposal =
  | Exclude<ActionProposal, { kind: "secret_input" }>
  | { kind: "secret_input"; field_name: string; adapter_id: string };

type PreparedAction = {
  action_id: OpaqueId;
  task_id: OpaqueId;
  run_id: OpaqueId;
  session: SessionRef;
  display_revision: Revision;
  redacted_preview: string;
  action_fingerprint: OpaqueId;   // 本地不透明绑定值，非公开机密裸哈希
  context_revision: Revision;
  model_snapshot_id: OpaqueId;
  task_secret_snapshot_id: OpaqueId;
  policy_revision: Revision;
  policy_result: "allow" | "require_approval" | "deny";
  reason_codes: string[];
  approval_id?: OpaqueId;
  expires_at: UtcTime;
};
```

`command_text` v1 拟只接受单行 UTF-8 文本，禁止 NUL、CR/LF、ESC 和其他直接终端控制字符；后端统一追加一次提交键。复合命令、重定向、解释器仍须风险评审，不能因为“单行”就认为安全。`terminal_text` 不接受控制字符，是否按 Enter 必须单独明确；`terminal_key` 枚举之外的键暂不开放。所有这些种类均经过相同审核，不能用逐字键入拼出被拒绝命令。

模型侧 `ModelActionProposal.secret_input` 只含 `field_name/adapter_id`；后端验证当前 run 后，补全所属任务与当前机密快照生成内部 `ActionProposal`，不接受模型提供的 task/snapshot ID。`secret_input` 只是受限扩展点：仅允许本版本明确支持、经过测试的 adapter；模型不能自定义 adapter 的编码程序。未知交互/密码提示或不支持的值返回 `SECRET_INJECTION_UNSUPPORTED`，由用户手动处理。没有“把 SecretRef 展开到任意命令字符串”的方法。

### 4.2 后端内部接口

| 内部方法 | 请求 → 响应 | 不能省略的检查 |
| --- | --- | --- |
| `prepare_action` | 绑定任务上下文、`ActionProposal`、观察引用 → `PreparedAction` | owner、连接、输入 schema、任务机密归属/存在、基线风险、状态未知边界 |
| `record_approval` | 审核 UI 事件与 `approval_id` → 后端单次授权记录 | 原始批准对象未变、未过期、未取消；拒绝也记录 |
| `dispatch_action` | `action_id` → `DispatchReceipt` | 重读不可变 prepared；确认显示；最新 owner/session/快照/策略；必要批准未消费 |
| `observe_terminal` | 绑定任务上下文、游标与上限 → 脱敏 `TerminalObservation` | 不返回原始机密/任意历史；标记截断、缺失、未知 |
| `cancel_pending_actions` | `task_id, run_id` → 已失效动作 ID | 不能撤销已送至 SSH 的字节 |

`dispatch_action` 不接受新的命令文本或机密值。变更动作必须创建新 `action_id` 并重新评审，不能在批准后修改参数。

### 4.3 必须可测试的顺序

1. 解析完整工具调用；不执行尚在 SSE 分片中的 JSON 或正文代码块。
2. `prepare_action` 固定动作、目标、机密快照与已知上下文；本机策略产生 `allow/require_approval/deny`。
3. 发送 `action.prepared`。UI 展示准确命令/输入、服务器、任务、机密引用与风险；`ack_action_display` 返回后才允许执行。自动低风险动作也展示，显示确认不要求用户点击。
4. 若要求批准，发送 `action.approval_required`。实际用户选择绑定该 `approval_id`，取消/拒绝不允许任何 AI 写入。
5. 紧邻派发，在同一 SessionActor 受控边界复核 owner、连接代次、动作、配置快照、策略版本、过期与已知上下文变化。
6. 原子保留有界队列容量并把批准标记消费，动作进入不可重复派发的待发送状态。容量不足且完全未入队时不消费批准；批准消费和入队不能出现可重复派发的间隙。
7. **出队后、真正写首个字节前再检查一次**取消状态、`task_id/run_id` owner、连接代次、过期、已知上下文和配置/策略快照。失效帧直接丢弃并报告 `not_sent/ACTION_STALE`，已消费批准不恢复；入队成功不是越过后续生命周期检查的许可证。取消/断线必须清除尚未写出的相关帧；最终校验与写入启动在同一 SessionActor 串行边界，不能被另一个 writer 绕过。
8. 校验通过后，`action.dispatch_started` 在本动作首个字节交给 SSH 前记录。之后即使写入失败或只写入一部分，也不能复用同一批准重试；返回未知发送/执行结果，重新核实。取消到达时若首字节已经交出，不能再报告为从未发送。
9. 返回发送收据，再关联后续观察。收据与 SSH 写成功都不证明远端命令已完成。

批准绑定 `{task_id, run_id, action_id, session_id, generation, action_fingerprint, context_revision, relevant_snapshot_ids, policy_revision, expires_at}`。有效期提案为 120 秒；审批等待可以更长，但过期后必须重建批准对象。批准不持久化为重启可用的令牌，模型也不收到令牌。

`context_revision` 只表示**已观察、已知与执行有关的变化**，例如明确的连接身份变化、受支持集成给出的上下文变更。它不是任意 shell 状态的完整指纹，不会通过键盘活动自动仲裁任务，也不会因每一行输出就重启审批。无法观测的人工 `cd`、身份或前台程序变化仍可能发生；审核界面必须说明动作将作用于当前实际终端，用户需自行避免干预。已知变更使待派发动作过期，不撤回已经发送的内容，不引入人工优先机制。

## 5. 模型工具面与 Rig 边界

| 模型可调用的拟议工具 | 输入 | 返回 |
| --- | --- | --- |
| `terminal_observe` | `after_output_seq?, max_bytes` | 当前任务允许的脱敏观察、范围/缺口、证据与未知状态 |
| `terminal_action` | `ModelActionProposal`、所依据的观察游标 | 准备/等待批准/拒绝/发送结果及后续观察；不能自行批准 |
| `task_require_fields` | `[{name, required}]` | 本任务字段名和 `present`；只追加缺失项 |
| `task_config_status` | 无 | 本任务字段名和 `present` |

这些工具不接受可自由指定的 session/task 路由。内部上下文已绑定当前 owner；跨任务 ID、任意路径、未知工具名一律拒绝。模型不能调用 `send_user_input`、`confirm_host_key`、`decide_action`、`save_connection_profile`、通用文件读取、任意 HTTP 或 SSH raw write。

Rig 拟用于构建 completion/agent、工具描述与流式适配；本产品只向它注册以上受控工具。是否使用 Rig 的 agent loop 或自行驱动 completion，取决于锁定版本后对取消、审批等待、事件顺序和串行调度的验证，不应绕过本机 Dispatcher。参考 [Rig 核心概念](https://rig.rs/docs/concepts/)。

- 同一模型回合返回多个 tool calls 时可先校验，但终端写动作必须逐个重新准备/派发，不能并行提交或把一个批准用于整批未知操作。
- tool-call ID 仅用于模型协议关联；执行去重仍使用后端动作 ID。
- 模型超时或 provider 重试不能自动重复已经派发的终端工具。恢复模型推理不等于恢复远端执行。
- 空闲流、畸形 JSON、未知工具、超长参数、工具未完成即断流都要返回明确状态；只执行完整且验证通过的调用。
- Provider 的“支持 tools”或 SSE 成功不是执行授权。Chat Completions + SSE + tools 需按选定服务/模型实测，详见 [配置兼容性](04-configuration.md#31-兼容性验证)。

## 6. 终端观察、结果与未知状态

```ts
type TerminalObservation = {
  session: SessionRef;
  first_output_seq: Revision;
  last_output_seq: Revision;
  redacted_text: string;
  truncated: boolean;
  gap_before: boolean;
  context_revision: Revision;
  mode_hint: "unknown" | "shell_prompt_hint" | "interactive_hint";
  evidence: { source: "pty_output" | "optional_shell_integration";
              summary: string }[];
};

type DispatchReceipt = {
  action_id: OpaqueId;
  delivery: "not_sent" | "write_accepted" | "unknown";
  remote_outcome: "unknown" | "observed_completed";
  // 只有能关联本动作的可靠证据才可填，不得猜测 0。
  exit_code?: number;
  evidence_ref?: OpaqueId;
};
```

`write_accepted` 只表示完整帧已被本机 SSH 写路径接受，不保证对端已接收或 shell 已执行。普通交互 shell 的 SSH channel exit-status 不是任意一条输入命令的退出码。`observed_completed` 需要与本动作可靠关联的证据；首版普通 PTY 流程通常保持 `unknown`，不能通过看到 `$`、`#` 或输出静默推断成功。

shell prompt、分页器、编辑器、确认对话、密码提示、长时间运行、全屏程序及混合手动输入都可能造成歧义。v1 只保留保守提示和输出片段，不要求实现完整 Bash 适配层。未来可选 shell integration 必须单独协商支持范围，防伪/关联能力未经验证前不能提升为权威完成证据。

未知状态的 UI 例子：“无法确认当前终端是否在等待输入。是否向这台服务器的当前终端发送一次 Enter？”用户确认仅授权该次 Enter，绑定同一个 prepare/approval/dispatch 契约；它不能把上一命令标成退出成功，也不允许后续自动回答所有问题。

原始 PTY 输出保留必要终端字节语义供当前终端显示；发送给模型、普通聊天结果和持久化记录必须经过单独的有界过滤视图。过滤不得破坏 UTF-8/终端分块；不能承诺未知或转换后的秘密全被过滤。输出始终是不可信数据，不能创建批准、更新配置或触发本机操作。

## 7. 事件、顺序和背压

### 7.1 事件封套

```ts
type AppEvent<T> = {
  stream_id: OpaqueId;       // 应用进程的事件流；重启后改变
  seq: Revision;            // 本事件流内严格递增
  time: UtcTime;
  kind: string;
  session?: SessionRef;
  task_id?: OpaqueId;
  run_id?: OpaqueId;
  action_id?: OpaqueId;
  payload: T;
};
```

拟定事件清单：

| 事件 | 核心 payload | 语义 |
| --- | --- | --- |
| `session.connecting/connected/closed` | 连接状态、安全原因码 | `connected` 仅在 PTY + shell 成功后；closed 不等于命令完成 |
| `session.host_key_required` | 挑战 ID、主机/端口、旧/新指纹状态 | 仅 UI 审核，不能由模型文本处理 |
| `terminal.output` | `output_seq, bytes_base64` | 当前终端渲染专用；不是可直接复制进 prompt 的字段 |
| `terminal.output_gap` | 缺失序号范围、原因 | 不补造内容；观察与 UI 均标明不完整 |
| `task.state_changed` | 新状态、状态修订、安全原因码 | 不能把任务阶段冒充远端成功 |
| `model.delta` | 消息 ID、片段序号、文本 | 纯展示；不得执行其 HTML/代码/命令 |
| `task.config_required` | 缺失字段名 | 机密值与文件正文不进入事件 |
| `action.prepared` | `PreparedAction` | 精确脱敏预览和策略结论 |
| `action.approval_required` | 批准 ID、失效时间、已知风险/未知点 | 由审核 UI 收集一次性选择 |
| `action.approval_decided` | 批准/拒绝/过期及安全原因码 | 后端状态事实，模型不能伪造 |
| `action.dispatch_started` | 动作 ID、传输序号 | 首字节前发布；并非远端开始证明 |
| `action.delivery_updated` | `DispatchReceipt` | 发送状态与远端结果分开 |
| `action.observation` | 关联的脱敏观察、证据、未知点 | 可以是观察区间，不承诺所有输出只属于该动作 |
| `config.reloaded` | 类别、新快照 ID、生效范围 | 无机密 |
| `error` | `ApiError` | 禁止透传底层原始错误 |

### 7.2 可依赖与不可依赖的顺序

- 事件序号由单一后端发布点分配；跨多个 IPC channel 的到达顺序不作保证。UI 使用序号/游标合并，不能按时间戳推断因果。
- 请求返回与其异步事件可能先后任意到达，均使用 request/task/run/action ID 关联。订阅应先建立，再读带游标的状态快照；需要补偿时 `get_events_since(stream_id, seq)` 返回有界重放或明确 gap。
- 同一动作必须先 `prepared`，再在需要时 `approval_required → approval_decided`，再 `dispatch_started`，再发送状态更新。允许输出先于 `write_accepted` 回执抵达，因为远端可能立即回显。
- `terminal.output` 使用独立 `output_seq` 保序；SSH 流分片不是命令边界。ASCII、UTF-8、ANSI 序列和敏感值都可能跨片段。
- 断线后旧 generation 的迟到输出可归档显示在旧会话范围，但不得驱动新会话决策。
- UI 销毁/重订阅不重放终端写操作；只有状态/事件重放。事件补发也不是重新执行动作。

### 7.3 有界资源提案

以下是压测前的工程默认，不是经过验证的性能指标：

| 项目 | 拟定上限 | 达到上限 |
| --- | --- | --- |
| 单个人工输入帧 | 16 KiB | 大粘贴由可信 UI 明确分块；失败部分不自动重发 |
| 单个 AI 动作文本/JSON | 16 KiB / 32 KiB | `INPUT_TOO_LARGE`，不截断命令后执行 |
| 待发送队列 | 128 帧且总计 256 KiB | `BACKPRESSURE`；全帧拒收，不丢中间字符 |
| 当前终端输出环缓冲 | 4 MiB | 受控背压，必要时丢最旧片段并发布 gap；不可无限增长 |
| 单次模型观察 | 64 KiB 脱敏文本 | 显式截断/缺口，不声称完整日志 |
| 事件重放缓存 | 2,000 条且总计 8 MiB | 最旧淘汰；消费者要求状态快照，不能无限缓存 |
| 活动 AI 动作 | 1 个待派发/观测步骤 | 串行下一动作，不并行调用写工具 |

SSH 读取、终端渲染、模型观察、脱敏日志分别设边界；不能因模型慢而无限积压，也不能把整段输出自动落盘作为未告知的“背压解决方案”。状态/批准这类控制事件丢失时，相关 AI 派发在重新同步前不得继续。断开 UI 显示通道后也不能继续不可见地自动发命令。

## 8. DecisionEvaluator 扩展点（Jev 未来可选）

Jev 在 v1 不启用，不要求安装/配置它，也不声称已经集成。先定义后端抽象，避免把服务响应当成应用权限。

```ts
type DecisionInput = {
  sanitized_action: { kind: string; redacted_preview: string };
  sanitized_state: {
    session_generation: Revision;
    context_revision: Revision;
    mode_hint: "unknown" | "shell_prompt_hint" | "interactive_hint";
    bounded_output: string;
    truncated: boolean;
  };
  task_metadata: { task_id: OpaqueId; run_id: OpaqueId; goal_summary: string;
                   secret_fields: SecretFieldStatus[] };
};

type DecisionAssessment = {
  assessment: "allow_candidate" | "require_approval" | "deny" | "unknown";
  confidence?: number;          // 若提供，必须有限且在 [0,1]；不是安全保证
  reason_codes?: string[];      // 受限长度、供解释/审计，不作执行指令
};

// 后端抽象：evaluate(input, cancellation) -> DecisionAssessment
```

输入要先过滤、限长并去除凭据；默认不含全终端历史、配置文件、实际秘密或批准令牌。未来将信息发送到新增服务需要明确的数据接收方说明和授权，不能因为已连接主模型就默认允许。

合并规则：

1. 应用基线策略先执行，始终有最终决定权。已知禁止项保持 `deny`；已知高风险保持至少 `require_approval`。
2. 可选 evaluator 的 `allow_candidate` 只是候选建议，不能降低基线要求、创造用户授权或证明远端状态。
3. `require_approval`/`deny` 可使应用更保守；错误、超时、取消、畸形输出或不支持字段按 `unknown` 处理，不能失败后放行。
4. v1 disabled evaluator 返回 `unknown`。这不意味着每条基线已知可处理的动作都必须等待模型：基线可以独立允许其已覆盖的低风险动作，也可以拒绝/请求批准。
5. **基线无法解决的风险或终端状态**加上 `unknown` 时，要求用户确认具体下一动作或自行处理；不自动派发，不要求本期补完复杂状态识别。
6. 不承诺 Jev 的固定准确率、低延迟、零误报、抗注入能力、API 字段或任何未验证服务保证。服务适配与能力测试属于后续版本工作。

## 9. 错误契约

所有错误使用稳定代码和脱敏消息；不能直接 `to_string()` 透传包含密码、TOML 原文、认证头或请求正文的第三方错误。`retry` 只建议 UI 下一步，不授权自动重发远端动作。

| 错误码 | 含义 | 处理 |
| --- | --- | --- |
| `CONFIG_NOT_FOUND / CONFIG_INVALID / CONFIG_VERSION_UNSUPPORTED` | 配置缺失、语义错误或版本不支持 | 用户修正并显式 reload；旧快照不假装更新 |
| `CONFIG_CONFLICT` | 多进程/外部编辑冲突 | 重读并让用户决定，不覆盖 |
| `CONFIG_SAVE_UNCERTAIN` | 替换后同步／复核失败，无法确认最终保存状态 | 停止后续覆盖，重读核对；不能假定旧版仍在或自动重试 |
| `LOCAL_PATH_INVALID / LOCAL_FILE_PERMISSION_DENIED` | 平台路径、文件类型、归属或访问限制不合格 | 用户修正；不搜寻其他目录／放宽权限 |
| `CREDENTIAL_BACKEND_MISMATCH / CREDENTIAL_REF_INVALID` | 保护模式不适用此平台，或凭据引用不匹配连接与用途 | 重新配置；不转换为明文或尝试其他应用凭据 |
| `CREDENTIAL_STORE_UNAVAILABLE / CREDENTIAL_STORE_LOCKED / CREDENTIAL_ACCESS_DENIED` | 平台凭据库缺失／不可达、锁定或拒绝访问 | 用户修复／解锁或显式选会话内存模式；不自动明文回退 |
| `SSH_CREDENTIAL_REQUIRED` | 没有可用的 SSH 登录凭据／私钥口令 | 专用认证 UI 手工填写；不是聊天或远端 PTY 输入 |
| `MODEL_AUTH_FAILED / MODEL_PROTOCOL_UNSUPPORTED / MODEL_STREAM_INVALID` | 服务认证或协议不兼容 | 安全错误；不换目标或执行文本兜底 |
| `SESSION_BUSY / SESSION_NOT_CONNECTED / SESSION_STALE` | 会话占用、不在线或代次失效 | 不切换到其他连接继续 |
| `HOST_KEY_UNTRUSTED / HOST_KEY_CHANGED` | 需要信任核验 | 明确用户核验；变更默认阻止 |
| `SSH_AUTH_FAILED / SSH_PTY_FAILED` | 认证或 PTY 请求失败 | 连接未成功，不调用模型执行 |
| `AI_OWNER_BUSY / TASK_NOT_OWNER / TASK_CANCELLED` | 写资格冲突、不是 owner 或已取消 | 不排队后悄悄执行 |
| `ACTION_DENIED / ACTION_STALE / APPROVAL_REQUIRED / APPROVAL_EXPIRED / APPROVAL_USED` | 策略/上下文/批准失败 | 不写；必要时新动作、新审核 |
| `TERMINAL_STATE_UNKNOWN` | 无法确定当前输入会产生什么作用 | 用户处理或批准一个具体动作 |
| `SECRET_MISSING / SECRET_SCOPE_MISMATCH / SECRET_INJECTION_UNSUPPORTED` | 字段缺失、跨任务或无安全适配器 | 填写/纠正或手动输入；不回显值 |
| `INPUT_INVALID / INPUT_TOO_LARGE / REQUEST_CONFLICT` | 输入不合法/超限/请求 ID 被复用 | 整帧拒绝，无部分截断执行 |
| `BACKPRESSURE / EVENT_GAP` | 容量不足/事件缺口 | 不静默丢输入；先重新同步 |
| `DELIVERY_UNKNOWN / REMOTE_OUTCOME_UNKNOWN` | 发送或远端结果不明确 | 明确未知，禁止自动重放 |

连接断开后的某次 `dispatch_action` 超时不是 `not_sent` 的证据；只有能证明从未向 SSH 交出任何字节时才返回 `not_sent`。进程崩溃后的记录也只能帮助核查，不能给出未经证实的执行结论。

## 10. 接口验收重点

- 同实例只能有一个活动服务器/PTY，AI 输入与用户看见的终端一致。
- 两聊天同时请求任务：只产生一个 AI owner，另一方获明确忙碌结果。
- 人工输入期间没有自动暂停/恢复或优先级行为；模型不能利用人工输入入口绕过策略。
- 显示动作早于首个 AI 写字节；不完整工具 JSON、正文代码块不能执行。
- 高风险动作无批准不派发；批准在使用一次、过期、取消、重连、改动作/相关配置后无效。
- 持有批准后队列满、入队后取消/过期/换配置、开始写后断线、部分发送、事件乱序/缺口、应用重启均不触发盲目重放。
- “停止 AI”不发送 Ctrl+C；单独远端中断也不声称成功回滚。
- 没有结束证据时不伪造 exit code；用户确认 Enter 不把上一动作标成成功。
- 大量输出、慢 UI、慢模型和多实例配置更新都保持内存/队列有界且错误可见。
- disabled evaluator 的 `unknown` 不破坏基线策略，也不使未解决场景自动放行。
- Windows／Linux 使用相同执行、授权及事件契约；路径、保护模式、keyring 状态、会话内存认证、保存结果未知和跨平台迁移各有反例测试。

这些是未来实现的验收约束。本次仅完成文档与配置示例，不存在接口测试、Windows／Linux 桌面集成测试或真实服务兼容测试已经通过的结论。
