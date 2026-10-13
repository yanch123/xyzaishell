# 04 配置与机密边界（design-v0.1）

> 状态：供开发评审的设计草案。用户确认了“三类配置分开”，但本文的 TOML 字段、路径、环境变量语义、平台安全存储方案和数值默认值都是工程提案，尚未冻结。本仓库没有配置解析器或可运行应用；示例通过 TOML 语法检查不代表运行时验证或安全机制已实现。

相关文档：[接口契约](03-interfaces.md)、[架构](02-architecture.md)、[安全设计](05-security.md)。

## 1. 已确认的三类配置

| 类别 | 保存内容 | 谁维护 | 模型可见边界 |
| --- | --- | --- | --- |
| 全局模型配置 | Base URL、API Key、模型名称 | 用户在本地独立文件中填写 | 模型请求使用指定模型；Key 仅用于向选定服务认证，不能进入 prompt、工具参数或终端 |
| SSH 连接配置 | 服务器、端口、账号、密码或私钥路径 | 应用单独管理 | 不把 SSH 密码、私钥内容或私钥口令交给模型 |
| 每任务配置 | 该任务需要的密码、token 等字段 | 应用创建文件，AI 提示用户自行填写 | 仅字段名和 `present` 布尔值，不提供实际值 |

全局模型 Key 与 SSH 登录凭据都不属于任务机密。每个聊天对应稳定的任务身份与一个文件；同一任务的多次执行 run 复用该任务文件，不为每次运行另建任务。不同对话/任务不能凭相同字段名互相取值。机器环境变量、连接信息、配置文件内容不会因为存在于本机而自动导出到远程 shell。

## 2. 路径和优先级提案

Windows 与 Linux 使用同一三类配置 schema，但路径由平台适配器解析，不能在共享核心硬编码 Windows 目录。以下均为建议目录；不是从当前工作目录推导，也不是应用支持 shell 插值。

| 数据 | Windows 提案 | Linux 提案 |
| --- | --- | --- |
| 模型／SSH／任务配置与 known_hosts | `%APPDATA%\xyzaishell` | `$XDG_CONFIG_HOME/xyzaishell`，默认 `$HOME/.config/xyzaishell` |
| 非秘密 SQLite、历史／审计状态 | `%LOCALAPPDATA%\xyzaishell\state` | `$XDG_STATE_HOME/xyzaishell`，默认 `$HOME/.local/state/xyzaishell` |
| 可丢弃缓存 | `%LOCALAPPDATA%\xyzaishell\cache` | `$XDG_CACHE_HOME/xyzaishell`，默认 `$HOME/.cache/xyzaishell` |

Linux 的 XDG 变量未设置或为空时采用上表默认；相对值按 XDG 规范视为无效并忽略，使用默认值且在设置 UI 给出提示。解析后的默认目录仍须通过权限与本地文件系统检查；不向系统级配置目录或当前工作目录继续搜寻秘密。此规则与下节 `XYZAI_MODEL_CONFIG` 的显式错误语义不同。[XDG Base Directory 规范](https://specifications.freedesktop.org/basedir/latest/)

```text
<平台配置根目录>/
  model.toml                  用户维护的全局模型配置
  connections.toml            软件管理的 SSH 连接配置
  known_hosts                 独立的 SSH 主机密钥信任记录
  tasks/
    <task_id>.toml             每个任务一份；该任务的不同 run 复用
```

上图 `/` 只表示层级，实现使用各平台路径 API。`known_hosts` 是信任记录，不是第四种机密配置；状态与缓存也不改变三类配置的划分。Linux 安全存储内的 SSH 凭据由其服务管理，不在上述文件中以明文保存。不得把这些运行时文件纳入 Git。

### 2.1 `XYZAI_MODEL_CONFIG` 的拟定语义

1. 若应用启动时存在非空 `XYZAI_MODEL_CONFIG`，它只表示模型配置文件的**绝对本地路径**，例如 Windows 的 `C:\Users\EXAMPLE\AppData\Roaming\xyzaishell\model.toml` 或 Linux 的 `/home/EXAMPLE/.config/xyzaishell/model.toml`。
2. 变量未设置时读取默认 `model.toml`。已设置但为空、相对路径、目录、无权限、格式错误或文件缺失时，显式报错；不能静默退回其他模型服务。
3. 不将该变量解释成 TOML 内容、API Key 或远程路径；不递归展开字符串中的 `%VAR%`、`$VAR` 或 `~`。
4. v1 不同时读取 `OPENAI_API_KEY`、`OPENAI_BASE_URL`、`XYZAI_API_KEY` 等后备变量，防止 SDK 默认环境读取覆盖已选文件。必须向模型适配器显式传入配置快照。
5. v1 不另设 SSH/任务路径环境变量，也不按当前工作目录搜寻配置。改变变量后重启应用选择新路径；“重新加载”只读取本次进程已选定的路径。
6. 环境变量名称和“变量指向文件而非直接填三个值”的语义仍待用户/工程评审确认，不能描述为已确认需求。

本地路径校验分别处理：

- Windows 要求盘符绝对路径；默认拒绝 UNC、设备命名空间和网络共享，检查目录归属、ACL 与重解析点。
- Linux 要求以 `/` 开头的绝对路径；区分大小写，不将反斜杠当目录分隔符，不展开 `~`／`$HOME`。检查实际文件类型、UID、权限、符号链接／硬链接及目标归属，避免路径替换竞态；拒绝设备文件、FIFO 和其他非普通配置／私钥文件。首版仅支持通过验证的本地文件系统，不因 POSIX 路径形式就认定 NFS／共享挂载安全。
- 任务路径只能由后端根据任务 ID 在所选根目录生成，模型不能传入路径或 `../`。路径校验与打开文件须形成受控边界；不能仅检查字符串前缀，也不能让跨平台导入自动转换成不明本机路径。

Windows 与 Linux GUI 从桌面启动时取得的环境可能与交互 shell 不同；设置 UI 应显示本次选定的配置路径。环境变量说明不得暗示应用会执行 `.bashrc` 等文件。

### 2.2 默认与覆盖规则

- 文件内字段优先于本文规定的非机密默认值；必需机密字段没有默认值。
- `schema_version = 1` 是每个文件的必填整数；更高/未知版本拒绝加载，不能猜测兼容。
- 未知字段、重复字段、错误类型必须报错，避免 `api_kye` 一类拼写被静默忽略。
- TOML 使用 UTF-8；无模板引擎，无字符串插值，无 `include`，无 shell 执行。
- 配置中的金额、权限或审批字段不能改变应用的安全策略；本草案没有 `skip_approval`、`allow_all` 或 `auto_accept_host_key` 开关。

## 3. 模型配置 schema

完整示例：[model.example.toml](../../config/examples/model.example.toml)。示例域名与 Key 只是占位，运行时必须拒绝把占位配置当作可用连接。

| 路径 | 类型 / 默认值 | 校验与含义 |
| --- | --- | --- |
| `schema_version` | 整数，必填 `1` | 文档 schema 版本，与模型版本无关 |
| `model.protocol` | 字符串，必填 `openai_chat_completions` | v1 目标协议；不自动猜测其他 API |
| `model.base_url` | 字符串，必填 | API 根地址，如 `https://api.example.invalid/v1`；不是完整 `/chat/completions` URL |
| `model.api_key` | 非空字符串，必填 | 仅由后端用于该目标的认证；去除误粘贴首尾空白应先提示，不静默改变 Key |
| `model.name` | 非空字符串，必填 | 精确传给服务；不根据品牌自动替换模型 |
| `capabilities.streaming` | `required`，默认同值 | 本应用要求 SSE 流式输出，并不证明服务支持 |
| `capabilities.tool_calling` | `required`，默认同值 | 自动执行要求工具调用经兼容验证 |
| `request.connect_timeout_seconds` | 整数，默认 `10`，范围 `1..120` | 建连上限 |
| `request.request_timeout_seconds` | 整数，默认 `120`，范围 `1..3600` | 一次模型请求总期限 |
| `request.stream_idle_timeout_seconds` | 整数，默认 `30`，范围 `1..300` | 无有效 SSE 数据的期限 |
| `request.max_output_tokens` | 整数，默认 `4096`，范围 `1..131072` | 提案本地上限；仍受实际模型限制，不能据此推断其上下文容量 |
| `transport.allow_loopback_http` | 布尔，默认 `false` | 仅显式启用后允许 `http://localhost`、`127.0.0.1` 或 `[::1]`；不放开远程 HTTP |

URL 校验要求：绝对 URL，不带用户名/密码、fragment 或 query；规范化末尾斜杠后只拼接一次 `/chat/completions`，不猜测是否需要 `/v1`。HTTPS 是默认要求；不能为了“兼容”关闭证书验证。回环例外只接受上述显式本机主机名/地址，不接受任意域名解析到回环的推断。v1 不跟随 API 重定向，也不把认证头转交给新 origin。代理设置如需支持，应另行显式设计，不能意外继承 SDK 的服务地址或凭据。

改变 Base URL 会改变 API Key 和请求上下文的接收方。UI 必须显示规范化目标，并要求用户在使用新目标前确认；该确认不是对任意未来目标的授权。Key 不下发到普通前端状态、遥测、错误详情或 prompt；认证头由后端 HTTP 层添加。

### 3.1 兼容性验证

“OpenAI-compatible”是待验证目标，不能理解为每个相似 URL 都支持本应用所需能力。提供 `probe_model_compatibility`，使用不含任务/终端/SSH 信息的最小测试请求，并清楚显示会向哪个服务发送认证及测试内容。记录按 `{base_url, model.name, config_revision, adapter_version}` 绑定的结果：

- HTTP/认证与基础 Chat Completions 响应是否成功。
- SSE 分片、UTF-8 边界、结束、超时和取消是否按预期处理。
- 工具名称、JSON 参数、tool-call ID、工具结果回传和多轮行为是否可用。
- 无工具能力时显示“仅聊天可用”或兼容性错误；不得把正文代码块当作工具执行，也不静默打开另一种协议。
- 测试通过只能证明已测路径，不是服务任意模型/版本的安全或兼容保证。服务变化后可失效。

Rig 拟承担 provider/model/tool/streaming 的适配；应用仍显式提供 Key、限制注册工具，并独立管理最终执行策略。不能因为 Rig 支持某能力就假定第三方服务也支持。参考 [Rig 核心概念](https://rig.rs/docs/concepts/)。

### 3.2 模型 Key 的本地存储风险

用户可编辑的 TOML 内 `api_key` 拟以明文保存；这不是加密保险库。默认目录/文件应限制到必要本机账户（Windows ACL；Linux 应用目录 `0700`、秘密文件 `0600`，并检查已有目录的归属与权限），提示不要放在同步盘、共享目录或 Git 仓库。编辑器备份、崩溃转储、恶意同用户进程仍可能暴露它；后续若引入系统凭据存储，必须新增明确 schema/迁移设计，不能声称当前示例已经加密。

## 4. SSH 连接 schema 与凭据保管

完整示例：[Windows 连接](../../config/examples/connections.example.toml)、[Linux 连接](../../config/examples/connections.linux.example.toml)。可保存多个连接档案，但一个应用实例同一时刻只连接一个服务器、一个可见 SSH PTY；这不等于在同一实例内增加多服务器标签页。

| 路径 | 类型 | 约束 |
| --- | --- | --- |
| `schema_version` | 整数 `1` | 必填 |
| `revision` | 非负整数 | 软件写入的并发修订号；更新前比较，不能把它当作可信授权 |
| `storage.secret_protection` | `dpapi_current_user`、`linux_secret_service` 或 `session_only` | 必填；Windows 持久存储用前者，Linux 持久存储用第二项；会话内存模式必须用户显式选择 |
| `connections` | 对象数组 | 各连接 `id` 唯一，由软件生成；不用于任意文件路径 |
| `connections[].id` | 不透明字符串 | 实际档案拟采用后端 UUID；示例 ID 不可直接运行 |
| `connections[].label` | 非空字符串 | 只作 UI 标签，不拼接 shell |
| `connections[].host` | 主机名或 IP 字符串 | 不含 scheme、账号或 shell 片段；端口独立 |
| `connections[].port` | 整数 `1..65535` | 默认 `22` |
| `connections[].username` | 非空字符串 | 不含 NUL/控制字符；由 SSH 库按协议传递 |
| `connections[].auth.kind` | `password` 或 `private_key` | 两种认证 schema 互斥 |
| `auth.protected_password` | DPAPI 密文编码字符串 | 仅 `dpapi_current_user` + `password` 必需；不得放明文密码 |
| `auth.password_secret_ref` | 不透明引用字符串 | 仅 `linux_secret_service` + `password` 必需；指向本机安全存储，不是秘密、路径或 D-Bus 任意对象入口 |
| `auth.private_key_path` | 绝对本地路径 | `private_key` 模式必需；仅读取用户选定文件，不扫描其他私钥 |
| `auth.protected_passphrase` | 可选 DPAPI 密文编码字符串 | 仅 `dpapi_current_user` + `private_key`；不保存口令时省略 |
| `auth.passphrase_secret_ref` | 可选不透明引用字符串 | 仅 `linux_secret_service` + `private_key`；不保存口令时省略 |

`storage.secret_protection` 对整个连接文件生效，不能在同一文件混用两平台的机密字段。 改变该模式是文件级变更，必须展示所有受影响档案并以整份文件的期望修订提交，不能由保存单个档案悄悄切换；转为 `session_only` 需移除所有持久机密字段，但不会默认删除原安全存储条目。未知认证／保护模式拒绝处理；在错误平台加载持久保护模式返回 `CREDENTIAL_BACKEND_MISMATCH`，不能尝试其他后端或把密文当密码。Windows 保留 `protected_*` 字段，Linux 使用 `*_secret_ref`，两组互斥；`session_only` 禁止这两组所有字段。密码模式不能带私钥路径／私钥口令字段；私钥模式不能带登录密码字段。前端只收到 `credential_present`、档案展示字段和安全错误码；读取配置接口不能返回 `protected_*`、安全存储引用字符串或解密结果。设置凭据的专用 UI 请求可以短暂把用户输入送往 Rust 后端，但不得进入聊天、通用事件或模型工具。

DPAPI CurrentUser 仅是拟定的静态保护机制：后端在当前 Windows 用户上下文保护/解保护密码与私钥口令，不使用 `CRYPTPROTECT_LOCAL_MACHINE`。DPAPI 通常依赖原用户登录身份及机器环境；复制文件到其他环境不保证可解密。它不保护已被控制的同用户进程、解密后的内存或用户选定的私钥文件本身。参考 [Microsoft CryptProtectData](https://learn.microsoft.com/en-us/windows/win32/api/dpapi/nf-dpapi-cryptprotectdata)。

Linux 拟通过 Secret Service 兼容适配器把 SSH 密码／私钥口令交给本机安全存储。引用由后端生成并校验应用命名空间、连接 ID 和用途（密码／口令）；配置中的任意字符串不能读取其他应用的 keyring 条目。安全存储的持久化、解锁及访问控制取决于实际提供者，不能把它描述为 DPAPI 的同等保证或宣称所有 Linux 都有可用 keyring。[Secret Service API](https://specifications.freedesktop.org/secret-service/latest/)

- 服务未安装／不可达、无可用桌面 D-Bus 会话、keyring 锁定、用户取消解锁或访问被拒时，返回区别明确的安全错误；不自动新建明文凭据文件，也不改写模型／任务密钥文件。
- 可请用户修复／解锁，或显式选择 `session_only`。后者从专用 SSH 认证 UI 手工输入，仅在当前进程内存保留；关闭进程后需重填，重启和重连不会重放历史输入。连接文件只保留非秘密档案及该模式，禁止写入秘密、密文或持久引用。
- 已有持久凭据的读取失败不能自动切换模式；用户选择会话内存模式时应明确是否保留旧的安全存储条目。默认不删除旧条目，删除另行确认。
- 加密私钥缺少保存的口令时，连接前由专用 UI 请求一次性口令；不能盲目往尚未建立的 PTY 输入 SSH 登录凭据。这与任务运行中的远端交互密码输入是不同路径。
- Windows DPAPI 密文不能跨平台使用；Linux 引用复制到另一台机器也不保证有效。迁移只复用重新校验的非秘密信息，重新选择私钥并重新输入认证材料；不提供明文凭据导出作为兼容手段。

私钥仍保留在原路径，不自动复制到应用目录。口令错误、文件格式不支持、读取失败应分别给出安全错误；不能自动回退到其他账号/认证方法。SSH 密码保存同样不构成对远程 `sudo`、数据库或网站口令的可复用授权。

主机密钥单独验证：首次未知主机需用户核对并明确接受；已记录指纹改变时阻止本次连接并要求处理。没有“连接失败就自动接受新指纹”的回退。更新连接密码或 host 不会自动修改已有主机信任记录。

## 5. 每任务配置 schema

完整示例：[task.example.toml](../../config/examples/task.example.toml)。

| 路径 | 类型 | 约束 |
| --- | --- | --- |
| `schema_version` | 整数 `1` | 必填 |
| `task.id` | 后端生成 UUID 字符串 | 必须与被加载的任务及文件名一致，不信任文件自称属于其他任务 |
| `secrets.<name>.required` | 布尔 | 默认 `true`，用于本地缺失项提示；不影响动作授权 |
| `secrets.<name>.value` | 字符串 | 用户填写；空字符串表示缺失，保留其他值的原始字符，不自动 trim |

字段名拟限制为 `[a-z][a-z0-9_]{0,63}`；每任务最多 64 项、每值 UTF-8 最多 16 KiB、整个文件最多 1 MiB，均为待压力测试的工程上限。机密值里的引号、空格、美元符、反斜杠、Unicode 和换行应按 TOML 正确解析，不得在解析时执行。某种执行适配器不支持这些字符时，应在具体使用处拒绝，不能悄悄删改。

拟定流程：

1. AI 通过任务工具提出字段名和必要性。工具不接受用户真实值，也不能覆盖已填值。
2. 后端为该任务创建/扩展本机文件；仅追加缺失字段，展示本机绝对路径给用户。模型只收到字段列表，不收到文件路径或文件内容。
3. AI 提示：“请在该任务的本地配置中填写 `deploy_password`，保存后点击重新加载。”不能让用户把密码粘到聊天里。
4. 用户编辑后主动重新加载。后端解析、校验，建立新的不可变机密快照，向模型返回 `[{"name":"deploy_password","present":true}]`。
5. 值只在已通过应用策略的具体动作里由后端解析引用；缺失或使用方式不支持时明确停止该动作。

`task.id` 由后端创建，任务之间不复用。示例中的 `GENERATED_BY_APPLICATION` 不是实际 ID，必须由应用生成；用户无需自行取名，也不能靠重命名文件跨任务取值。删除对话不应默默销毁用户配置；任务机密保留/删除需要明确 UI 行为，安全删除与备份清除不作虚假保证。

### 5.1 机密引用不是字符串模板

内部机密引用拟为结构化 `{task_id, field_name, snapshot_id}`，不使用 `${PASSWORD}`、`{{secret}}` 等直接替换任意 shell 文本的方案。

- `resolve_secret` 仅限后端执行层，模型工具列表里没有“读取机密值”。任务 owner、快照和动作目标必须匹配。
- 机密使用需要已测试的数据边界，例如受支持的交互输入适配器，或明确 shell/命令语义的参数编码器。批准界面显示机密字段名、用途和发送目标，不显示实际值。
- 不把“把值包一层引号”视为通用 shell 注入防护；POSIX、PowerShell、不同命令解析器及远程 raw mode 的语义不同。
- 未知密码提示、无法确认输入对象、控制字符/多行输入不受支持，或没有安全编码器时，返回 `SECRET_INJECTION_UNSUPPORTED`，让用户在同一终端自行处理。v1 不要求先造完整 Bash 适配器来假装覆盖这些场景。
- 不默认将所有任务机密 `export` 成远程环境变量，不把机密拼在命令行或临时脚本里。需要特定方式传输时，必须把目标/用途/可见性作为该动作评审的一部分。
- 远程终端可能回显机密、记录 shell 历史，目标程序也可能记录输入。已知值过滤只是降低风险；未知机密、编码/截断/变换后的值及跨片段内容不能保证全被识别。不得宣称“模型永远不可能看到任何泄露输出”。

### 5.2 用户可编辑明文的风险

任务文件拟保留明文以便用户自己填写，这与 SSH 软件管理的密文／安全存储引用是不同取舍。首次生成/打开时应提示：不要提交、同步或分享；本机编辑器可能生成备份；离开任务后按需清除文件。应用生成的临时文件、备份也必须采用同等访问限制（Windows ACL；Linux `0600` 文件／`0700` 私有目录），不能为了容错把明文另存宽权限目录。模型上下文、审计、诊断包默认不包含原文件。

## 6. 加载、快照与并发写入

### 6.1 显式加载，按类别生效

- 初次加载顺序：确定路径 → 受限读取 → TOML 解析 → schema/语义校验 → 建立不可变快照 → 对外返回脱敏摘要。
- 不在每次工具调用时重读磁盘，也不静默热加载。可以提示“文件有变更”，但只有用户显式重新加载后才使用新值。
- 模型快照绑定一次执行 `run_id`；重新加载的模型配置用于后续 run，包括同一聊天/任务的下一次执行。正在运行的 run 仍使用旧快照，并显示这一点；若用户要立即更换，先取消当前 run，再显式启动新 run。稳定 `task_id` 和任务配置文件不变，取消不会替代远程中断。
- SSH 快照绑定建立的连接；修改档案只影响下一次连接，不悄悄换服务器或认证身份。
- 任务机密可在当前任务的安全动作边界显式加载；先完成或拒绝当前 dispatch，替换快照后使所有尚未派发的旧 prepared action/批准失效并重新准备。不能在已经审核后、真正写入前换成另一组机密。
- 加载失败保留当前已验证快照，但 UI 显示“修改未生效”；从未加载成功则禁止相关操作。不能把错误配置当成功或清空旧凭据。
- 对外修订号/快照 ID 不应是明文机密的裸哈希，避免泄露低熵密码的离线猜测验证器；使用后端不透明 ID，必要内部摘要仅在本地受保护范围使用。

### 6.2 多进程与外部编辑

允许用户启动不同实例连接不同服务器，不依赖“全系统只能有一个进程”解决文件并发。共享保存契约为：稳定锁对象的跨进程锁 → 重读修订号 → 比较 `expected_revision`／内容版本 → 创建同目录受限临时文件 → 写入并同步 → 平台替换 → 复核并返回修订。锁对象不得随被替换文件的 inode／句柄变化而失效；不能锁旧文件后误以为新文件也受同一锁保护。

- Windows：由适配器选取并验证替换／首次创建流程，处理共享模式、文件占用、ACL 继承及替换错误；不能先删除旧文件再写新文件。[Microsoft ReplaceFileW](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-replacefilew)
- Linux：临时文件位于同一文件系统的目标目录，以受限权限创建；同步内容后使用原子 rename，再同步父目录。rename 的可见性原子性与断电后的持久性是不同保证，不得把跨文件系统 copy+delete 当作原子替换。[Linux rename](https://man7.org/linux/man-pages/man2/rename.2.html)、[Linux fsync](https://man7.org/linux/man-pages/man2/fsync.2.html)
- 提交前失败应保持旧版可用；替换后同步失败或无法判定当前磁盘版本时返回 `CONFIG_SAVE_UNCERTAIN`，保留必要恢复信息并要求重读核对。不能在这种情况下声称“肯定没保存”，也不能自动重试覆盖。
- 版本冲突返回 `CONFIG_CONFLICT`，不静默采用最后写入者。任务追加字段同样核对期望内容版本，不覆盖已填值。外部编辑器不一定遵循锁，提交前比较不能消除全部竞态；编辑期间并发保存不在完全隔离保证内，应提示避免同时写入，保留受限恢复副本／冲突信息并进行故障测试。
- Linux keyring 更新与 TOML 保存不是单一事务。更新凭据先创建新条目而非覆盖旧条目，再提交新引用；配置保存确定后才处理旧条目清理。提交失败／状态未知时不可先删旧条目；未引用条目的恢复与清理只限本应用归属，并需避免删除其他实例仍使用的条目。不会用明文临时文件弥合这两个存储的事务差异。

并发写、进程退出、磁盘满、文件占用和断电边界必须在两平台分别注入验证。加载遇到编辑器写到一半的文件，只报告未加载成功，不执行半份配置。原子文件保存不等于远端命令可回滚或恰好执行一次。

## 7. 错误、诊断和测试边界

错误结构参见 [接口错误](03-interfaces.md#9-错误契约)。配置错误沿用 `{code, safe_message, request_id, retry}`，可附 `{category, field_path, line?, column?}` 安全定位字段：不回显 TOML 源行、值、HTTP 认证头、SSH 握手机密或底层完整响应。用户本地 UI 可显示配置路径，但该路径不进入模型工具错误或遥测。日志只记类别、后端 ID、版本和安全原因码。

必测项目：

- 四份示例（共享模型／任务、Windows／Linux 连接）是有效 TOML；占位值不能被当成真实可用配置。
- 环境变量缺失/空值/相对路径/无权限/错误 TOML/版本不支持的结果明确。
- Key 只在所选 HTTPS 目标认证；重定向、目标变更和调试日志不泄漏。
- 模型/SSH/任务机密互不可读；跨任务引用失败。
- Windows DPAPI 在本用户成功，在不适用身份／环境失败时不回退明文；Linux keyring 正常、锁定、缺失、无 D-Bus 和拒绝解锁均有明确结果。
- Windows／Linux 路径格式、XDG 默认与错误值、大小写、中文／空格、链接替换、UID／ACL／mode 及不支持的文件系统均被校验。
- 各保护模式的必需／禁止字段、错误平台加载、跨连接引用和跨机器迁移失败不造成错误取密；会话内存模式不落盘。
- Windows 替换与 Linux rename／父目录同步、锁对象稳定性、提交状态未知、keyring／文件分步失败均有可恢复证据。
- 特殊字符、多行值、错误字段名、缺失机密、文件中途修改、并发写冲突不导致误执行。
- 显式 reload 的快照生效点、旧批准失效、正在执行动作不换值可复现。
- 脱敏错误不含实际值；过滤跨片段已知值有测试，同时记录不能覆盖未知变换的限制。

## 8. 待评审项

1. 是否接受 `XYZAI_MODEL_CONFIG` 指向绝对文件路径，以及当前默认目录方案。
2. 用户编辑的模型／任务明文文件与 SSH 平台安全存储的取舍、Linux 提供者选择及会话内存模式、重新录入式迁移 UX。
3. 回环 HTTP 是否进入 v1，及 UI 的风险提示。
4. 兼容性验证范围、超时/容量默认值与实际服务能力矩阵。
5. 首批可安全支持的机密注入方式；未验证方式保持人工处理。

本文仍使用 `schema_version = 1`，因为尚无已发布配置解析器；design-v0.1 内的 Windows／Linux 字段是同一待冻结草案。将来若有已发布用户数据，任何不兼容改动必须升级 schema 并定义迁移，不能借用此说明静默改变格式。

以上待评审项不影响“三类配置分开、任务机密不进入 prompt、应用策略控制实际发送”的设计底线。
