# 04 — Agent 循环与通用命令策略

日期：2026-10-16。性质：**v0.1.4 完整设计约定，尚未实现／尚未验收**。

本文约束后续实现，不代表当前软件已经支持这些能力。当前 `crates/xyz-core/src/risk.rs` 使用有限字面量解析器，并将未知程序名、带路径程序、部分解释器和工作流直接判为不可执行；`agent.rs`、`execution.rs` 与 Shell 集成也需要随本设计调整。不得通过改说明文字、放宽一个布尔值或取消版本检查宣布完成。

## 用户只需知道

连接服务器，填写模型地址／模型名／API Key，然后直接聊天。普通命令紧凑展示后执行；危险或未知操作只出现一张简短卡片：目标、命令、影响、批准／拒绝。首次能力和数据许可简短说明必要范围；技术细节、状态解释和配置帮助默认折叠。本文后续内容面向实现与验收，不要求用户阅读或配置这些内部状态。

## 1. 产品目标和范围

用户提出意图，Agent 理解目标、形成可调整计划、提出具体命令、读取真实结果，再决定下一步，直至目标得到验证、需要用户决定或遇到明确边界。不是要求用户逐条写命令，也不是只会执行一个程序名白名单的聊天界面。

固定约束：一个进程一条 SSH 连接、一个用户可见 PTY；所有命令在同一个交互 Shell 中顺序运行，共享当前目录和 Shell 状态。禁止第二条隐藏 SSH exec 通道、本机 Shell、SFTP/SCP 文件传输、后台文件写入工具和远端常驻 Agent。连接时在同一 PTY 进行受控 Shell 启动不等于额外执行通道。未来 SystemOne／Jevhook 仅保留扩展位置，本版不接入、不赋予执行权；不引入 Skills。

通用性来自“能表达的命令都有明确评估路径”，不是“任何文本都能运行”。普通命令自动执行的范围可以保守；超出自动范围但结构可表达的命令，应当进入精确审批，而非仅因程序名不认识就拒绝。

## 2. 从意图到下一步

### 2.1 一个输入框和一个持续任务上下文

1. 用户发送自然语言。问候、解释等可直接回答；模型不能从自由文本旁路写终端。
2. 宿主提供当前目标、可用能力、已确认事实及有界历史。未连接／准备中仍可聊天，不伪装为“终端忙”。
3. 模型给出简短目标和下一步；复杂任务可显示计划，但不要求每次启动任务确认，也不要求事先冻结所有命令。
4. 模型一次只提议一个完整命令。宿主独立验证结构、生命周期、风险、许可与展示绑定。
5. 命令卡先显示实际发送内容、服务器、目录证据及影响。满足自动策略则收到展示回执后执行；危险或未知风险命令等待本次批准。
6. 在同一 PTY 派发一次，采集与动作关联的输出；等待结束证据和独立 ready。程序事实进入结果卡。
7. 模型分析结果，更新事实／假设／剩余步骤，回答、继续提议、询问或停止。每个下一步重新通过全部检查。

连续追问可以引用前一轮结果，但新目标不会继承旧命令批准。多聊天共享一位 AI 写入者；其他聊天仍可阅读和解释。已有实现的轮次上限可继续作为预算（当前源码为 16），到达时显示已完成、未完成和继续入口，不把预算停止描述为目标完成。

### 2.2 三种不同的决定

| 层 | 问题 | 决定者 | 不能替代什么 |
| --- | --- | --- | --- |
| 任务意图 | 用户希望达到什么结果，是否还需下一步？ | 用户＋模型 | 不能证明命令实际成功，也不能批准危险动作 |
| 能力／数据许可 | 是否允许对此服务器自动执行普通命令，并向指定模型发送有限终端内容？ | 用户许可＋宿主校验 | 不能成为对所有危险动作的总批准 |
| 动作批准 | 是否运行这一条精确命令及其明确影响？ | 用户本次批准＋宿主绑定 | 不能使不支持的语法、无 ready 的会话变得可执行 |

首次启用、模型接收方或许可的服务器范围改变才重新询问能力／数据许可；每轮不重复启动弹窗。选区附件仍独立预览、编辑和同意发送；选区、日志、远端输出中的命令只是数据。已知秘密不进入普通历史／模型上下文。许可撤销立即阻止未派发动作，不能撤回已发送数据或已运行命令。

## 3. 三层命令判断，禁止合并为 executable 布尔值

### 3.1 结构可表达性

本版最低交付语法为 `foreground_ast_v1`：完整、有界、单行，由字面量命令组成的前台管道／条件和顺序列表，支持静态文件重定向。不是完整 Bash parser，也不是程序名白名单。支持普通空格分词和经过测试的字面量引号／转义；拒绝 NUL、控制字符、未闭合引号、前导空白及超长输入。原载荷上限 8192 UTF-8 字节，最多 16 个 simple-command、单条 pipeline 最多 8 段、每个 simple-command 最多 8 个重定向；所有上限必须有边界测试。

拟定 AST 语法（保留 token 原文位置；AST 只用于检查，不用于重打印命令）：

```text
action   := and_or (';' and_or)*
and_or   := pipeline (('&&' | '||') pipeline)*
pipeline := simple ('|' simple)*
simple   := literal_word (literal_word | redirection)*
redirection := ('>' | '>>' | '<') literal_path
```

`&&`／`||` 同优先级、左结合，pipeline 优先，`;` 最外层；语义跟随 Bash，不按程序员熟悉的布尔优先级另解释。literal_path 是不含展开的明确字面量路径，可绝对或相对，不是设备描述符表达式；初版拒绝数字 fd 前缀、`>&`／`<&`、`&>`、`>|`、here-string、`/dev/fd/*`、`/dev/std*`、`/dev/tcp/*`／`/dev/udp/*` 等特殊流目的地。文件类型／软链接目标仍可能变化，不能将静态路径当成无副作用证明。simple 必须有命令，拒绝只有重定向的命令和空管道段；暂不支持末尾分隔符。

词法门槛必须与 Bash 实际解析一致：拒绝未加引号的保留字／控制字（包括 `!`、`time`、`if`、`for`、`while`、`case`、`function` 等）、注释起始、花括号／通配展开、反引号、特殊 `$'…'`／`$"…"` 引号、赋值前缀和空命令。仅引号内明确字面量字符作为数据保留；未闭合、前后串接歧义或无法证明字面量的词法形式拒绝。`name=value` 在普通程序的后续参数位置可作为字面量，不能因此允许 Shell 赋值前缀。测试必须包含 AST 与实际 Bash 解释的差分案例，不能把解析器认为的“参数”交给 Bash 后变成另一种结构。

例如 `ps aux | grep nginx`、`cd -- /srv/app && pwd -P`、`printf '%s' 'enabled=true' > /tmp/app.conf` 均有结构支持路径。整段 AST 的所有分支都评估，不因短路分支“可能不执行”而免审；任何输出重定向至少危险，未知输入路径／未知程序至少未知风险，整段不低于其任一组成部分。管道后段的 stdin 已由前段连接，不能把 `ps aux | grep nginx` 中的 grep 按“缺少文件参数、等待终端输入”误拦；低风险判定需按 AST 的 stdin 来源和整条有限性分析，不能逐段复用孤立命令布尔值。最前段仍可能读取终端，未知生产者也可能无限运行；这些风险保留。输入重定向只有专门验证过的读取规则可自动；默认审批。任何节点生命周期不支持则整段转手动，批准不能跳过该节点。

不得执行宿主解析后重建的另一条命令。解析仅用于判断和展示；原载荷逐字节保留，由下节的受控顶层信封承载。卡片默认显示原命令，展开可查看实际发送信封；两者分别摘要并绑定同一批准。当前 `# xyz-action:<id>` 与 history 相关性机制不足以证明 4.2 任意语法 preexec，不能作为新设计的通用 AI 开始证据。

不在本版最低语法内：嵌套子 Shell／命令分组、后台 `&`、作业控制、here-document、换行脚本、命令替换、进程替换、变量展开、通配展开、赋值前缀、函数定义、循环／条件等控制结构。`~` 展开初版也不支持，应另提议明确路径而不是宿主悄悄展开。引号中的相同字符是字面量，例如 `grep -n 'a|b' app.log` 不是管道。

执行的是原始完整 action，一次审批、一次信封、一个最终退出码；绝不能把 AST 拆成独立终端提交。不能按 `|` 或 `;` 盲拆，也不能去掉 `sudo`、重定向、引号或“不安全片段”后运行。模型可提出不同的多步骤方案，每一步都是新提议，并说明语义差异；例如将服务器管道换成有界日志读取和模型分析可能改变截断、隐私和过滤位置，不能声称等价。

管道退出码沿用当前 Shell 的 `pipefail` 设置，默认通常取末段；列表沿用 Bash 实际最终执行命令的状态。宿主不偷偷 `set -o pipefail`，不伪造每段退出码。若任务要求知道上游是否失败，模型必须提出可验证的额外检查；退出 0 不能证明全部分支执行或全部阶段成功。管道中的 `cd` 等 builtin 可能在子 Shell 执行、不会改变父 Shell 目录；这是原命令语义，不由宿主“修正”。顺序列表的目录变化及后续相对路径影响必须作为整段审批说明。

### 3.2 生命周期兼容性

结构可表达后再判断：是否会保持受控父 Shell、能否使用现有开始／结束／ready 协议观察前台执行、是否要求未提供的输入或另一执行环境。

- 普通外部程序、带路径的可执行文件和脚本可进入评估；名称不熟悉本身不是生命周期不支持证据。`cd` 与经检查的 `export KEY=value` 属于保持父 Shell 状态的专门 builtin 路径；export 不等于赋值前缀语法。涉及保留命名空间／协议变量／关键解析和历史环境的修改转手动，普通环境变量修改默认审批，并测试下一条命令确实继承。
- 已知破坏当前 Shell 的模式，如顶层 `exit`、`exec`、`logout`，或在当前 Shell 执行任意代码的 `eval`、`.`／`source`，不提供“一次批准即可运行”的假入口。它们需要专门生命周期设计，本版转手动。
- `su`、`sudo -s/-i`、嵌套交互 Shell、编辑器／全屏 TUI 进入手动接管，不向它们连续发送 Agent 命令。
- 别名／函数／特殊 builtin 解析不能假设等于同名二进制。无法建立解析及生命周期边界时显示具体原因；可另提议明确绝对路径或经验证的 `command` 形式，但必须重新展示／批准，不能偷偷改写。
- `bash -c '…'`、`python script.py` 等以外部子进程运行的字面量命令，不因解释器名字永久封禁。其载荷是**不透明代码**：不提供自动低风险判定，必须显示完整载荷、按未知／危险风险审阅，并满足非交互前台契约。不得随意额外套 `sh -c` 或使用临时 `eval` 绕过检查；唯一允许的内部 `builtin eval` 是下节定义、显式绑定的统一传输信封。这里的前台尝试是操作约定：结构受支持、派发身份明确且不是已识别的 Shell 替换／交互／作业控制形式。不要求证明任意程序一定结束或绝不读取输入；这些未知性必须经批准，运行时一旦阻塞或破坏协议就停止自动推进。不能靠模型说“安全”取得自动权限。
- 任意程序都可能阻塞、读取 stdin、修改终端或破坏协议。批准只允许尝试，不保证能结束、不保证隔离；出乎预期时停止自动推进并保留未知状态。

现有 Shell 协议是合作式相关性证据，不是抵御恶意远端程序的安全沙箱。终端已有主机权限不会因 Agent 分类而降低。

### 3.3 选定机制：同一父 Shell 的显式动作信封

**实现决策：AI 动作采用宿主拥有的顶层 framing，不再依赖 DEBUG 在任意语法之前触发。** Bash 4.2／现代版本使用同一 AI 路径；手动命令记录继续由独立适配器观察。该机制仍只运行在已经连接的同一可见 PTY，维持父 Shell 的 cwd，不启动第二条 SSH 命令或独立工作目录容器。

信封的语义顺序如下（这是设计伪步骤，不是可复制执行的脚本）：

1. 宿主把原始命令按已测试的 Bash 单引号字面量编码赋给保留命名空间中的载荷变量；不对载荷做展开、拼接或剥离。
2. 设置本动作 framing 标记，验证会话 nonce／动作标识／集成完整性，发出本动作 Begin。
3. 在**顶层当前交互 Shell** 执行 `builtin eval -- "$__xyz_action_payload"`，只对原载荷解释一次。
4. 紧接 eval 保存 `$?`，在保存前不得调用日志、printf、hook 或另一命令；发出带动作标识的 End，清理本动作临时变量和 framing 标记。
5. Shell 返回提示符时单独发 ready；不能由信封里的 End 同时伪造 ready。

变量名、内部函数、builtin 解析、信封版本及 nonce 都在准备阶段验证；发现用户定义冲突／篡改则禁用 Agent，不覆盖原定义。载荷赋值与外围信封必须有独立编码测试，尤其单引号、美元符号、反引号、反斜杠、非 ASCII 和协议分隔符；不依赖命令文本拼接猜转义。信封传输内容和原命令均以 UTF-8 摘要批准，序列号由宿主管理，只有本次已预留动作的事件可关联。

不得把整个 payload 放入 Shell 函数、子 Shell、新 `bash -c` 或本地进程：函数会引入 local／return 作用域，子 Shell 会丢失父 Shell 的 cwd／环境变更。事件输出 helper 可以是固定内部函数，但 eval 和退出码保存必须留在顶层；内部事件错误不能冒充用户命令退出码。

**不能声称等同用户逐字键入。** eval 是额外解析上下文，BASH_COMMAND、DEBUG／ERR 行为、历史记录、调用诊断和错误位置可能不同；交互别名在 eval 时的解析也必须测试。`return`、`break`／`continue`、`exit`／`exec`、当前 Shell `source`／`eval`、修改 trap／hook／关键 options 等控制行为不在最低可执行能力中。宿主的 eval 不是对用户任意 eval 免审的许可。`errexit`、自定义 trap 等可能跳过结束步骤的环境仍按能力检查拒绝。

信封不解决所有恶意／偶发情况：程序可以写终端控制序列、杀父进程或通过副作用破坏协议。缺失 End／ready 则结果未知并停止，不自动补发信封。输出事件仍是合作式协议，不是可信远端审计。

AI 活动期间手动日志适配器不得将信封内每个简单命令、eval 内部命令或原载荷重复登记为多次用户操作；保存一条原载荷动作及其真实输出。用户实际可见 PTY 不隐藏信封或伪造终端原始输出，UI 以清楚的宿主命令卡解释包装；默认不展开实现细节。手动终端独立使用时，4.2 无法精确观察的复合语法必须标明记录缺口／未知，不妨碍它继续作为手动终端，也不能自动绑定到旧 AI 动作。

### 3.4 风险和授权

| 风险 | 含义 | 默认动作 |
| --- | --- | --- |
| `low` | 宿主识别的特定只读／目录导航形式，参数和目标满足窄规则 | 能力许可有效＋展示后自动运行 |
| `dangerous` | 已识别的数据修改、服务影响、权限／身份变化、网络传输等 | 展示具体影响，精确单次批准 |
| `unknown` | 命令结构可表达，但效果／参数／程序不能充分分类 | 显示未知点，精确单次批准；不标为安全 |

`unknown risk` 不等于 `unsupported`。前者可以是可审批命令；后者说明具体结构或生命周期能力缺失，审批不能修复。宿主可以以参数级 allowlist 决定哪些形式免确认，但不能把这份清单当成整个 Agent 的执行能力清单。

审批卡必须包含完整命令、目标身份、目录证据、分类原因、可能影响及未知点。删除文件、重启服务、修改配置等不能只显示“需要确认”。大范围破坏、认证／安全策略变化等还必须经过产品安全规则；危险审批不是绕过独立禁止项的万能钥匙。拒绝后可以解释或提议更小动作，不自动改写后重试。

## 4. 提议接口与宿主记录（拟定，非现有 API）

以下为应落到版本化 Rust／TypeScript DTO 的语义草案。未知字段、未知枚举、多个并行调用、不完整工具响应失败关闭；模型不能填写宿主事实或批准记录。

```typescript
type AgentStep =
  | { kind: 'answer'; text: string }
  | { kind: 'propose_command'; command_text: string;
      purpose: string; expected_observation: string }
  | { kind: 'ask_user'; question: string }
  | { kind: 'finish'; summary: string; evidence_action_ids: string[] };

// Host-created AST; literal values never replace original command_text.
type CommandAst = { kind: 'list'; items: AndOr[] };
type AndOr = { first: Pipeline;
  rest: { operator: '&&' | '||'; pipeline: Pipeline }[] };
type Pipeline = { commands: SimpleCommand[] };
type SimpleCommand = { words: { literal: string; start: number; end: number }[];
  redirects: { operator: '>' | '>>' | '<'; path: string;
    start: number; end: number }[] };
// All source ranges are UTF-8 byte offsets; preserve redirection order.
type SyntaxDecision =
  | { kind: 'representable'; grammar: 'foreground_ast_v1'; parser_version: string }
  | { kind: 'unsupported'; code: 'CONTROL_BYTES' | 'INCOMPLETE_SYNTAX'
      | 'UNSUPPORTED_GRAMMAR' | 'EXPANSION' | 'LIMIT'; detail: string };
type LifecycleDecision =
  | { kind: 'foreground_attempt'; profile: string; caveats: string[] }
  | { kind: 'manual_required'; code: 'SHELL_REPLACEMENT' | 'SHELL_MUTATION'
      | 'INTERACTIVE_CONTEXT' | 'UNRESOLVED_DISPATCH'; detail: string }
  | { kind: 'unavailable'; code: 'NOT_READY' | 'UNSUPPORTED_SHELL'
      | 'UNKNOWN_STATE' | 'DISCONNECTED' };
type RiskDecision = { level: 'low' | 'dangerous' | 'unknown';
  policy_version: string; reasons: string[]; affected_targets: string[] };
type Authorization =
  | { kind: 'auto'; scope_id: string }
  | { kind: 'approval_required'; challenge_id: string }
  | { kind: 'approved_once'; approval_id: string; binding_hash: string }
  | { kind: 'denied' | 'expired' | 'policy_blocked'; reason: string };

type ActionBinding = {
  action_id: string; task_id: string; run_id: string;
  target_id: string; host_key_fingerprint: string; remote_user: string;
  session_id: string; generation: string; shell_id: string;
  ready_sequence: number; ready_revision: number;
  cwd: { value: string; source: 'shell_ready'; revision: number } | null;
  command_text: string; command_utf8_sha256: string;
  transport_utf8_sha256: string; display_revision: string;
  model_revision: string; destination: string;
  scope_id: string; policy_version: string; capability_version: string;
};
type CommandResult = {
  action_id: string;
  dispatch: 'not_sent' | 'sent' | 'delivery_unknown';
  outcome: 'not_started' | 'running' | 'exited' | 'unknown';
  exit_code: number | null;
  completion_sequence: number | null;
  ready_after: boolean;
  output: { text: string; truncated: boolean; redacted: boolean;
    byte_limit: number; source: 'same_visible_pty' };
  interruption: 'none' | 'requested' | 'observed';
};
```

`cwd` 尚无可信观测时必须为 `null`，不能使用模型记忆的目录充当事实。交付应在 ready 元数据中提供有界目录证据，或以同一 PTY 的显式 `pwd -P` 动作建立观测，并在任何手动写入后失效；若使用后一方案，接口的 source 要按实际观测扩展，不能伪称来自 ready。相对路径动作在未知目录时先确认目录，不隐式 `cd`。

审批摘要应覆盖 `ActionBinding` 的规范序列化和实际发送字节；不以字符串拼接歧义编码。展示回执绑定同一摘要。批准只消费一次；命令任何字节、目标、会话、ready epoch、模型接收方、许可、策略或能力版本变化即失效。即使用户批准期间手动执行了一条 `cd`，也不能使用旧批准。

派发前在同一宿主临界区检查当前 session、ready、单写入者、许可和批准；预留动作并将其标为已尝试，再进行唯一写入。网络写入失败但不能证明零字节发送时记录 `delivery_unknown`，不重发。摘要用于绑定和审计，不证明远端程序内容未变，也不是沙箱。

## 5. 执行状态、用户输入和中断

### 5.1 状态机

`planning → evaluating → awaiting_display → awaiting_approval? → dispatching → awaiting_begin → running → awaiting_ready → observing → planning/finished`。

独立阻塞／终止状态：`waiting_user_input`、`manual_control`、`cancelled_before_dispatch`、`agent_stopped_remote_active`、`outcome_unknown`、`unsupported`、`disconnected`。这些是拟定的类型化状态，不应继续靠任意字符串拼接 UI 状态。

- End 匹配本动作但尚无独立 ready：可以显示退出码，仍不能派发下一条。
- ready 不能补造缺失的 Begin／End。输出“成功”、提示符形状、长时间安静都不是结束证据。
- `exit_code=0` 是前台进程事实；目标完成是模型依据证据的判断。`grep` 退出 1 可能表示未匹配，`systemctl is-active` 非零可能表示服务未运行，不能一概写“工具坏了”。
- 输出被裁剪、脱敏、截断或连接丢失时显式标注；不能把“没有看到”说成“不存在”。
- 命令启动了守护进程／容器／服务，前台返回只证明提交过程结束。需要独立状态查询验证业务结果。

### 5.2 sudo、密码与交互

优先提议显式非交互形式，例如 `sudo -n systemctl restart nginx`。权限不足或需要密码时返回真实错误，再选择用户手动完成认证／操作，或使用经过单独验收的秘密输入流程。不得自动移除 `-n` 或重复发命令。

普通 `sudo` 形式若进入密码提示，Agent 进入等待用户状态，不能把下一条计划命令发进去。终端文本像密码提示并不能验证输入接收方；秘密输入必须绑定同一活动动作、会话、专用字段和用户确认的当前输入目标，且不进入模型／日志。未具备这些能力时只能用户在可见终端手动输入；不得承诺自动密码处理已可用。

未知 stdin 消费者一律不猜输入。没有出现可识别密码提示不代表可以发送数据；`read`、`cat` 无文件、安装器问题、远端脚本提示均不能接收模型生成的回车、`yes`、密码或下一条命令。用户接管期间暂停 Agent 写入；回到协议确认的 Shell ready 后重新建立绑定。用户手动操作产生的结果不冒充旧 AI 动作结果。

### 5.3 长任务、停止与重连

有限但耗时的前台命令可以执行，继续显示运行及增量有界输出；本地等待超时不证明远端失败，更不允许重放。无限跟随日志／持续监控优先改成明确有界查询；若用户确需前台流，须展示无自然结束和占用 PTY，显式确认后以观察模式运行，下一步必须等用户中断和真实 ready。本版若未验收该观察能力则标为手动，不用“未知程序”掩盖缺失能力。

“停止 AI”仅停止模型和后续派发；已运行远端命令继续，界面保持真实状态。Ctrl+C 是独立用户可见动作，绑定当前前台 session，发出后等待结束／ready；发出了中断不等于已中断。没有 active foreground 证据时不自动发送控制字符。不通过第二通道发 `kill` 或猜 PID。

断线、重连、窗口恢复、取消和迟到事件全部使旧批准／ready 失效。结果未知的动作不自动重试；重新连接后只可提出新的只读核实步骤，用户批准的修改也不能假定“没有执行”。恢复聊天历史仅供阅读。

## 6. 典型操作决策矩阵

以下为目标行为和验收样例，不是当前功能清单。所有自动运行还要求相关目标及数据许可、真实 ready、完整展示、无 shadowing 和参数级规则。

| 例子 | 结构／生命周期 | 风险和目标路径 | 验证与边界 |
| --- | --- | --- | --- |
| `pwd -P`；随后 `cd -- '/srv/app'` | 单条字面量；父 Shell 导航 | 可纳入低风险自动规则 | 下一条仍在同一 Shell；目录需实际证据 |
| `grep -n 'error' /var/log/app.log` | 有显式文件，前台 | 窄参数规则可自动 | 退出 1 是未匹配；大文件仍可能耗时 |
| `grep -n 'a\|b' app.log` | 引号内管道字符是数据 | 根据 grep 模式规则判断 | 不误拆管道 |
| `grep error`；`cat` | 语法可表达，可能等 stdin | 不纳入自动规则；提示输入边界／转手动 | 不发下一条命令或猜回车 |
| `docker ps --all` | 字面量外部程序 | 首版默认未知、可单次批准；以后只对已测子命令加窄规则 | Docker 不整体自动放行；无 daemon／权限错误是实际结果 |
| `docker run …`、`docker exec …`、`docker compose up -d` | 逐一评估参数，不能只看 docker 名字 | 修改／执行代码／外部访问，危险或未知，精确批准 | 卷挂载、特权、网络、目标 context 显示；`-it` 进入交互边界；容器就绪另查 |
| `docker logs --tail 100 app` | 有界日志请求 | 未分类时可审批；专门规则验收后才可自动 | `-f` 不继承有界规则；日志可能含秘密 |
| `systemctl --no-pager status nginx` | 显式禁分页，前台 | 已测参数形式可自动 | 非零状态如实解释；不偷偷添加参数 |
| `systemctl restart nginx` | 前台请求，可能触发认证 | 危险批准；优先重新提议明确认证方式 | 运行后另查服务状态；不能假称 restart 退出 0 即健康 |
| `/opt/company/check-health --summary` | 未知路径但完整字面量 | 未知风险，可单次批准 | 不因路径或程序名而一票否决；未找到／卡住照实报告 |
| `python /srv/app/maintenance.py --check` | 外部解释器，不透明文件载荷 | 未知风险批准，不能因 `--check` 自称只读而自动 | 脚本内容可变；不提供可靠终止保证 |
| `ps aux \| grep nginx` | 有界前台 pipeline，保持整段原文 | 经测试的只读管道规则可自动；否则未知审批 | 一次信封，不拆成两次执行；末段退出码不证明上游成功 |
| `printf '%s' 'enabled=true' > /etc/app.conf` | 静态输出重定向在语法内 | 文件创建／截断，危险单次批准 | 明确目标、内容、覆盖风险，随后验证；不偷换 tee 或上传文件 |
| `sed -i 's/old/new/' /etc/app.conf` | 单条字面量，嵌入 sed 语言是数据 | 未知／危险，显示文件、变更预期并审批 | 先读取、备份方案、具体变更、验证；不声称任意配置可编辑 |
| `vim /etc/app.conf`、`systemctl edit nginx` | 编辑器交互 | 手动接管 | Agent 不按键自动编辑，不向未知 stdin 粘贴内容 |
| `make -C /srv/app test` | 有限前台但可能耗时／脚本有副作用 | 未知风险批准 | 增量输出；超时不重放；退出结果与测试完整性分别判断 |
| `tail -f app.log` | 无自然结束 | 经过验收的显式观察模式，否则手动 | 结束靠用户中断；禁止自动继续下一命令 |
| `scp …`、`rsync …` 文件传输 | 即使语法可表达也违背本版范围 | 策略阻止，不提供批准绕过 | 无文件传输与单 SSH／PTY 约束独立于风险分级 |

配置编辑不承诺覆盖所有格式、事务或原子替换。仅当可在已有有界动作契约中给出明确且经批准的变更时执行；备份也是独立可见写操作，必须说明路径／覆盖影响。验证失败后提出回滚命令，再按其自身风险／授权判断，不静默回滚。复杂多行配置不能直接塞入换行脚本／heredoc；若不能用经验证的字面量命令与静态重定向准确表达，应交给手动终端，而非暗建文件工具。

## 7. Shell 版本与能力证据

Bash 4.2.46 是用户实际环境的必测目标，但不是唯一版本；Bash 5.1／5.2 路径也需回归。AI 采用上述显式动作信封，两版本路径必须分别证明每个动作只关联一次 Begin／End／ready。4.2 的 DEBUG trap 与现代 PS0 只承担其已测手动记录能力，不能承诺完整 Shell 语法的 preexec；不可用改版本号条件替代兼容性实现。已有 trap、动态 prompt、别名／函数、手动输入、启动脚本读取输入等应按自动准备设计的明确能力诊断处理。

当前 4.2 草稿在实际 4.2.46 上出现默认作业控制空 trap 被误拒绝的情况；该待修问题也证明不能把现有适配器写成已验收。仅可按实际能力诊断处理，不能顺带放行用户自定义非空 trap。

UI 和模型只拿到已验证的能力集，例如 `foreground_ast_v1`、`foreground_lifecycle_v1`、`cwd_observation_v1`；不得凭 Bash 版本号推导管道、脚本、交互输入或文件编辑已经支持。能力关闭时保留聊天和手动终端，不改用户 Shell 配置强行启用。

## 8. 验收清单与交付判据

以下全部是**待验收**，本文未运行或声明任何测试通过。结果必须绑定精确提交、实际 Shell／OpenSSH 版本、测试环境、期望和实际输出。现有测试中“git status／./script 一律不可执行”等断言不能作为新设计通过证据。

1. 自然语言提出检查需求，Agent 自行计划、执行低风险检查、读取结果并继续；同一输入框追问使用前文，问候不触发终端。
2. 未知程序名、绝对路径和脚本外部进程的完整字面量形式进入未知风险审批；批准后只写一次。未批准零写入。对不存在程序记录真实失败，不伪称结构不支持。
3. 结构不支持、生命周期不支持、策略禁止和未知风险有不同类型／说明；不显示不能成功使用的批准按钮。
4. 参数级测试覆盖 grep 模式／文件歧义、stdin、systemctl 分页、Docker 子命令／context／交互／特权模式；未知参数回到审批，不把整个工具自动放行。
5. 引号内特殊字符与真实操作符区分；控制字符、Unicode 字节长度、未闭合引号、信封注入、换行／替换／嵌套／后台拒绝；已支持管道／列表／静态重定向完整解析并保持原文，危险分支不能因短路而逃过审批。覆盖 `&&`／`||` 左结合、引号内操作符、多个重定向及路径歧义；无 split、strip 或隐式改写。内部唯一顶层 eval 的原载荷及传输字节一致性单独验收，不能出现任意旁路 eval。
6. 当前目录跨 `cd` 和多条命令保留；手动 `cd` 使旧展示／批准失效；未知目录不假造相对路径目标。别名／函数 shadowing 不走低风险路径。
7. 精确命令、运输字节、展示 revision、session、generation、ready epoch、身份、目标、许可变化逐项使旧批准失效；重复点击和迟到回执不能重复派发。
8. End 后 ready 前不派发；伪提示符、成功文字、安静输出不触发完成；退出 0 但业务检查失败时模型不能宣布目标达成。
9. sudo 非交互失败、密码提示、未知 stdin、用户接管、秘密输入取消均无误注入；未验收秘密输入功能不出现在可用能力中。
10. 长任务不断线仍运行、超时、Ctrl+C 被忽略、停止 AI 后命令继续、断线／写入结果未知、重连均不重放；中断只作为请求直至观察到证据。
11. 前台 `docker … -d`／服务管理命令结束后需要独立验证；持续日志观察不能霸占 Agent 循环且伪称完成。
12. Bash 4.2.46、5.1、5.2 的真实 PTY 和 OpenSSH 分别覆盖以上生命周期关键路径；不支持的 Shell／hook 配置有明确降级，模拟事件不能替代真实终端证据。
13. 信封在真实 4.2.46／5.1／5.2 下保持 cwd、单次 Begin／End、准确非零退出码和独立 ready；管道分别在用户原有 pipefail 开／关环境验证，列表验证短路和最终状态，不修改用户选项；手动日志不重复保存信封内部动作。验证保留变量冲突、eval 错误、hook 被破坏、父 Shell 被杀、Ctrl+C 发生在信封各阶段时均不误判／重放。记录 eval 与直接键入的非等价边界。
14. 检查 SSH channel 创建和写入记录，证明仅一个可见 PTY，无隐藏执行、文件传输、本机 Shell或新服务；未启用持久日志时不会额外保存命令秘密。
15. 达到轮数／输出预算、用户取消、授权撤回后均有明确未完成项，不伪造结论。SystemOne／Jevhook 和 Skills 不出现在本版已实现能力中。

完成标准：上述路径实现、负向用例通过、版本能力矩阵与真实证据公开记录，随后才可将本文件对应条目从“设计目标”升级为“已实现／已验收”。即使通过，仍需明确有界前台 AST 和合作式远端协议的边界，不宣传为任意 Shell 脚本的安全执行沙箱。
