//! Host classification of a bounded foreground AST, never an execution whitelist.
use crate::command_ast::{self, RedirectKind};
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Risk {
    Normal,
    Dangerous,
    Unknown,
}
#[derive(Clone, Debug)]
pub(crate) struct Assessment {
    pub risk: Risk,
    pub executable: bool,
    pub reason: String,
}
#[derive(Clone, Debug)]
enum Lifecycle {
    ForegroundAttempt,
    ManualRequired(String),
}
#[derive(Clone, Debug)]
struct StageAssessment {
    risk: Risk,
    lifecycle: Lifecycle,
    finite_source: bool,
    reason: String,
}
fn normal() -> StageAssessment {
    StageAssessment {
        risk: Risk::Normal,
        lifecycle: Lifecycle::ForegroundAttempt,
        finite_source: true,
        reason: "已识别只读检查或目录导航；远端程序不受沙箱隔离。".into(),
    }
}
fn dangerous() -> StageAssessment {
    StageAssessment {
        risk: Risk::Dangerous,
        lifecycle: Lifecycle::ForegroundAttempt,
        finite_source: true,
        reason: "可能修改数据、权限、服务、身份或传输数据；需要批准本次完整命令。".into(),
    }
}
fn unknown(reason: &str) -> StageAssessment {
    StageAssessment {
        risk: Risk::Unknown,
        lifecycle: Lifecycle::ForegroundAttempt,
        finite_source: false,
        reason: reason.into(),
    }
}
fn manual(reason: &str) -> StageAssessment {
    StageAssessment {
        risk: Risk::Unknown,
        lifecycle: Lifecycle::ManualRequired(reason.into()),
        finite_source: false,
        reason: reason.into(),
    }
}
fn merge(a: Risk, b: Risk) -> Risk {
    if a == Risk::Dangerous || b == Risk::Dangerous {
        Risk::Dangerous
    } else if a == Risk::Unknown || b == Risk::Unknown {
        Risk::Unknown
    } else {
        Risk::Normal
    }
}

fn flags(args: &[String], short: &str, long: &[&str], operands: bool) -> bool {
    let mut after_options = false;
    for argument in args {
        if !after_options && argument == "--" {
            after_options = true;
            continue;
        }
        if !after_options && argument.starts_with("--") {
            if !long.contains(&argument.as_str()) {
                return false;
            }
        } else if !after_options && argument.starts_with('-') && argument != "-" {
            if !argument[1..].chars().all(|c| short.contains(c)) {
                return false;
            }
        } else if !operands || argument == "-" {
            return false;
        }
    }
    true
}
fn has_file_operand(args: &[String]) -> bool {
    let mut after_options = false;
    for argument in args {
        if !after_options && argument == "--" {
            after_options = true;
            continue;
        }
        if (after_options || !argument.starts_with('-')) && argument != "-" {
            return true;
        }
    }
    false
}
fn count_args(args: &[String]) -> bool {
    let mut index = 0;
    let mut operand = false;
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "--" {
            return (operand || !args[index + 1..].is_empty())
                && args[index + 1..].iter().all(|v| v != "-");
        }
        if matches!(arg, "-n" | "-c" | "--lines" | "--bytes") {
            index += 1;
            if args
                .get(index)
                .is_none_or(|n| n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()))
            {
                return false;
            }
        } else if arg.starts_with('-') {
            if !matches!(arg, "-q" | "-v" | "--quiet" | "--verbose") {
                return false;
            }
        } else {
            operand = true;
        }
        index += 1;
    }
    operand
}
fn find_args(args: &[String]) -> bool {
    if args.is_empty() {
        return true;
    }
    let mut index = 0;
    while index < args.len() && !args[index].starts_with('-') {
        index += 1;
    }
    while index < args.len() {
        match args[index].as_str() {
            "-print" | "-ls" | "-empty" => {}
            "-name" | "-iname" | "-path" => {
                index += 1;
                if args.get(index).is_none() {
                    return false;
                }
            }
            "-type" => {
                index += 1;
                if args
                    .get(index)
                    .is_none_or(|v| !matches!(v.as_str(), "f" | "d" | "l"))
                {
                    return false;
                }
            }
            "-maxdepth" | "-mindepth" => {
                index += 1;
                if args
                    .get(index)
                    .is_none_or(|v| v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()))
                {
                    return false;
                }
            }
            _ => return false,
        }
        index += 1;
    }
    true
}

fn printf_args(args: &[String]) -> bool {
    let Some(format) = args.first().filter(|value| !value.starts_with('-')) else {
        return false;
    };
    // Bash printf's %n can assign shell variables even without -v. Only
    // literal text, %% and %s belong to the automatic output-only subset.
    let mut characters = format.chars();
    while let Some(character) = characters.next() {
        if character == '%' && !matches!(characters.next(), Some('%' | 's')) {
            return false;
        }
    }
    true
}
fn printf_mutates(args: &[String]) -> bool {
    if args.first().is_some_and(|a| a.starts_with("-v")) {
        return true;
    }
    let args = if args.first().is_some_and(|a| a == "--") {
        &args[1..]
    } else {
        args
    };
    let Some(format) = args.first() else {
        return false;
    };
    let mut chars = format.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            for spec in chars.by_ref() {
                if spec == 'n' {
                    return true;
                }
                if "diouxXfFeEgGaAcCsSbqQT%".contains(spec) {
                    break;
                }
            }
        }
    }
    false
}
fn ps_args(args: &[String]) -> bool {
    let mut index = 0;
    while index < args.len() {
        let argument = args[index].as_str();
        if matches!(
            argument,
            "-o" | "--format" | "-p" | "--pid" | "-u" | "--user" | "--sort"
        ) {
            index += 1;
            if args
                .get(index)
                .is_none_or(|v| v.is_empty() || v.starts_with('-'))
            {
                return false;
            }
        } else if let Some(options) = argument.strip_prefix('-') {
            if options.is_empty() || !options.chars().all(|c| "aAdeEfFlLwWxy".contains(c)) {
                return false;
            }
        } else if argument.is_empty() || !argument.chars().all(|c| "auxfwl".contains(c)) {
            return false;
        }
        index += 1;
    }
    true
}
fn journal_args(args: &[String]) -> bool {
    if !args.iter().any(|v| v == "--no-pager") {
        return false;
    }
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--no-pager" | "--utc" | "--reverse" | "-r" | "--quiet" | "-q" | "--system"
            | "--user" | "-b" => {}
            "-n" | "--lines" => {
                index += 1;
                if args
                    .get(index)
                    .is_none_or(|v| v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()))
                {
                    return false;
                }
            }
            "-u" | "--unit" | "--since" | "--until" | "-p" | "--priority" => {
                index += 1;
                if args
                    .get(index)
                    .is_none_or(|v| v.is_empty() || v.starts_with('-'))
                {
                    return false;
                }
            }
            _ => return false,
        }
        index += 1;
    }
    true
}
fn systemctl_query(args: &[String]) -> bool {
    let options = ["--no-pager", "--plain", "--all", "--full"];
    let words: Vec<_> = args
        .iter()
        .filter(|v| !options.contains(&v.as_str()))
        .collect();
    args.iter().any(|v| v == "--no-pager")
        && words.first().is_some_and(|v| {
            matches!(
                v.as_str(),
                "status"
                    | "show"
                    | "is-active"
                    | "is-enabled"
                    | "list-units"
                    | "list-unit-files"
                    | "list-jobs"
            )
        })
        && words.iter().skip(1).all(|v| !v.starts_with('-'))
}
fn grep_args(args: &[String], input: bool) -> bool {
    let mut i = 0;
    let mut pattern = false;
    let mut files = false;
    let mut options = true;
    while i < args.len() {
        let a = args[i].as_str();
        if options && a == "--" {
            options = false;
        } else if options
            && matches!(
                a,
                "-A" | "-B" | "-C" | "--after-context" | "--before-context" | "--context"
            )
        {
            i += 1;
            if args
                .get(i)
                .is_none_or(|v| v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()))
            {
                return false;
            }
        } else if options && a.starts_with('-') && a != "-" {
            if a.len() > 2
                && matches!(a.as_bytes().get(1), Some(b'A' | b'B' | b'C'))
                && a[2..].bytes().all(|b| b.is_ascii_digit())
            {
            } else if !flags(
                &[a.into()],
                "EFGivwnxclLhHsqroabU",
                &[
                    "--extended-regexp",
                    "--fixed-strings",
                    "--ignore-case",
                    "--line-number",
                    "--invert-match",
                    "--word-regexp",
                    "--line-regexp",
                    "--count",
                    "--files-with-matches",
                    "--files-without-match",
                    "--no-messages",
                    "--recursive",
                ],
                false,
            ) {
                return false;
            }
        } else if !pattern {
            pattern = true;
        } else if a == "-" {
            if !input {
                return false;
            }
        } else {
            files = true;
        }
        i += 1;
    }
    pattern && (files || input)
}
fn reserved_variable(key: &str) -> bool {
    key.starts_with("__xyz")
        || key.starts_with("BASH")
        || key.starts_with("HIST")
        || key.starts_with("PS")
        || matches!(
            key,
            "PATH"
                | "IFS"
                | "ENV"
                | "SHELLOPTS"
                | "PROMPT_COMMAND"
                | "CDPATH"
                | "GLOBIGNORE"
                | "FUNCIGNORE"
                | "SHELL"
        )
}
fn docker(args: &[String]) -> StageAssessment {
    let mut i = 0;
    while let Some(arg) = args.get(i).filter(|a| a.starts_with('-')) {
        if matches!(arg.as_str(), "--context" | "--host" | "-H" | "--config") {
            if args.get(i + 1).is_none() {
                return unknown("Docker 全局目标参数不完整。");
            }
            i += 2;
        } else if ["--context=", "--host=", "--config="]
            .iter()
            .any(|p| arg.starts_with(p))
            || matches!(arg.as_str(), "--debug" | "-D" | "--tls" | "--tlsverify")
        {
            i += 1;
        } else {
            return unknown("Docker 全局选项未知，需要核对目标与完整命令。");
        }
    }
    let mut decision = docker_subcommand(&args[i..]);
    if i > 0 && decision.risk == Risk::Normal {
        decision = unknown("Docker 全局选项改变 context、目标或连接配置；需要核对目标并批准。");
    }
    decision
}
fn docker_subcommand(args: &[String]) -> StageAssessment {
    let Some(sub) = args.first() else {
        return unknown("Docker 子命令未知，需要核对完整参数。");
    };
    let rest = &args[1..];
    if (matches!(sub.as_str(), "exec" | "run" | "attach")
        && rest.iter().any(|a| {
            a.starts_with("--interactive")
                || a.starts_with("--tty")
                || a.starts_with('-') && !a.starts_with("--") && a[1..].contains(['i', 't'])
        }))
        || (sub == "logs" && rest.iter().any(|a| a.starts_with("--follow") || a == "-f"))
    {
        return manual("Docker 交互、附着或持续日志需要手动终端。");
    }
    match sub.as_str() {
        "attach" => manual("Docker attach 需要持续交互，需手动终端。"),
        "stats" if !rest.iter().any(|a| a == "--no-stream") => manual("Docker stats 未禁用持续流，需手动终端或重新提议 --no-stream。"),
        "restart" | "stop" | "rm" | "kill" | "start" | "pause" | "unpause" => dangerous(),
        "ps" if flags(rest, "aqs", &["--all", "--quiet", "--size", "--no-trunc"], false) => normal(),
        "inspect" if has_file_operand(rest) && flags(rest, "", &["--size"], true) => normal(),
        "stats" if flags(rest, "a", &["--all", "--no-stream", "--no-trunc"], true) => normal(),
        "logs" => {
            let mut i = 0; let mut target = false;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--timestamps" | "-t" | "--details" => {},
                    "--tail" => { i += 1; if rest.get(i).is_none_or(|v| v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit())) { return unknown("Docker 日志数量参数未知，需要批准。"); } },
                    "--since" | "--until" => { i += 1; if rest.get(i).is_none() { return unknown("Docker 日志时间参数不完整。"); } },
                    a if !a.starts_with('-') && !target => target = true,
                    _ => return unknown("Docker 日志选项未知，需要核对完整命令。"),
                }
                i += 1;
            }
            if target { normal() } else { unknown("Docker 日志目标不明确。") }
        }
        _ => unknown("Docker 的此子命令、context、选项或容器载荷未经只读分类；批准仅允许一次前台尝试，不证明容器任务完成。"),
    }
}

pub(crate) fn classify(command: &str, shadowed: &[String]) -> Assessment {
    let ast = match command_ast::parse(command) {
        Ok(ast) => ast,
        Err(error) => return Assessment { risk: Risk::Unknown, executable: false, reason: format!("语法不支持（{error:?}）：仅支持有界字面量前台命令、管道、条件列表和静态文件重定向；未发送任何内容。") },
    };
    let mut risk = Risk::Normal;
    let mut reasons = vec![];
    let mut lifecycle = Lifecycle::ForegroundAttempt;
    for item in &ast.items {
        for pipeline in std::iter::once(&item.first).chain(item.rest.iter().map(|v| &v.1)) {
            let mut upstream_finite = false;
            for (index, simple) in pipeline.commands.iter().enumerate() {
                let input_file = simple
                    .redirects
                    .iter()
                    .any(|r| r.kind == RedirectKind::Read);
                let input = index > 0 || input_file;
                let tokens: Vec<_> = simple.words.iter().map(|w| w.literal.clone()).collect();
                let mut stage = classify_stage(
                    &tokens,
                    shadowed,
                    input,
                    index > 0 && upstream_finite && !input_file,
                    0,
                );
                for redirect in &simple.redirects {
                    match redirect.kind {
                        RedirectKind::Read => {
                            stage.risk = merge(stage.risk, Risk::Unknown);
                            stage.finite_source = false;
                            reasons.push(format!(
                                "输入文件 {} 的类型与内容未验证，需要批准。",
                                redirect.path.literal
                            ));
                        }
                        RedirectKind::Write | RedirectKind::Append => {
                            stage.risk = Risk::Dangerous;
                            reasons.push(format!(
                                "输出重定向可能创建、覆盖或追加文件 {}。",
                                redirect.path.literal
                            ));
                        }
                    }
                }
                risk = merge(risk, stage.risk);
                if let Lifecycle::ManualRequired(_) = stage.lifecycle {
                    lifecycle = stage.lifecycle;
                }
                reasons.push(format!("{}：{}", tokens[0], stage.reason));
                upstream_finite = stage.finite_source && (index == 0 || upstream_finite);
            }
        }
    }
    if let Lifecycle::ManualRequired(reason) = lifecycle {
        return Assessment {
            risk: Risk::Unknown,
            executable: false,
            reason: format!("生命周期或策略不支持，需要手动处理：{reason}"),
        };
    }
    Assessment {
        risk,
        executable: true,
        reason: reasons.join(" "),
    }
}
fn classify_stage(
    tokens: &[String],
    shadowed: &[String],
    input: bool,
    finite_input: bool,
    depth: u8,
) -> StageAssessment {
    if depth > 4 {
        return manual("命令前缀嵌套超过能力上限。");
    }
    let name = tokens[0].as_str();
    let args = &tokens[1..];
    if shadowed.iter().any(|n| n == name) {
        return manual("此名称被别名或函数覆盖，无法建立实际派发与生命周期边界。");
    }
    if tokens.iter().any(|s| s.contains("__xyz")) {
        return manual("命令引用保留协议命名空间，不能通过批准修改或干扰动作封装。");
    }
    if tokens.iter().any(|s| {
        matches!(
            s.as_str(),
            "/dev/zero" | "/dev/random" | "/dev/urandom" | "/dev/tty" | "/dev/stdin"
        ) || s.starts_with("/dev/fd/")
            || s.starts_with("/proc/self/fd/")
    }) {
        return manual("特殊设备或终端输入可能持续输出或等待交互，需要手动处理。");
    }
    let base = name.rsplit('/').next().unwrap_or(name);
    if matches!(
        base,
        "exit"
            | "exec"
            | "logout"
            | "eval"
            | "."
            | "source"
            | "return"
            | "break"
            | "continue"
            | "trap"
            | "set"
            | "let"
            | "getopts"
            | "shift"
            | "shopt"
            | "alias"
            | "unalias"
            | "function"
            | "builtin"
            | "enable"
            | "declare"
            | "typeset"
            | "local"
            | "readonly"
            | "unset"
            | "history"
            | "fc"
            | "hash"
            | "read"
            | "readarray"
            | "mapfile"
            | "bind"
            | "stty"
            | "reset"
            | "fg"
            | "bg"
            | "disown"
            | "wait"
            | "suspend"
            | "su"
    ) {
        return manual("此操作会改变当前 Shell、输入模式或控制协议，需要专门生命周期支持。");
    }
    if matches!(
        base,
        "vim"
            | "vi"
            | "nano"
            | "emacs"
            | "less"
            | "more"
            | "top"
            | "htop"
            | "watch"
            | "ssh"
            | "sftp"
            | "tmux"
            | "screen"
            | "yes"
    ) {
        return manual("已知交互、全屏或持续运行命令，需要手动终端。");
    }
    if matches!(base, "scp" | "rsync") {
        return dangerous();
    }
    if name == "command" {
        let inner = if args
            .first()
            .is_some_and(|v| matches!(v.as_str(), "-p" | "--"))
        {
            &args[1..]
        } else {
            args
        };
        if inner.is_empty() || inner[0].starts_with('-') {
            return manual("command 模式未受支持或缺少明确子命令。");
        }
        return classify_stage(inner, &[], input, finite_input, depth + 1);
    }
    if name == "sudo" {
        let mut i = 0;
        while i < args.len() && args[i].starts_with('-') {
            match args[i].as_str() {
                "--" => {
                    i += 1;
                    break;
                }
                "-n" | "--non-interactive" | "-H" | "--set-home" => {}
                "-u" | "--user" | "-g" | "--group" => {
                    i += 1;
                    if args
                        .get(i)
                        .is_none_or(|v| v.is_empty() || v.starts_with('-'))
                    {
                        return manual("sudo 用户或组参数不完整。");
                    }
                }
                _ => return manual("此 sudo 模式可能建立交互上下文或读取输入，需要手动终端。"),
            }
            i += 1;
        }
        if i == args.len() {
            return manual("sudo 缺少明确子命令。");
        }
        let nested = classify_stage(&args[i..], &[], input, finite_input, depth + 1);
        return if matches!(nested.lifecycle, Lifecycle::ManualRequired(_)) {
            nested
        } else {
            let mut decision = dangerous();
            decision.finite_source = nested.finite_source;
            decision
                .reason
                .push_str(&format!(" 子命令：{}", nested.reason));
            decision
        };
    }
    if name == "export" {
        if args.is_empty() || args.iter().any(|a| !a.contains('=')) {
            return manual("仅支持明确的 export NAME=value 形式。");
        }
        for arg in args {
            let (key, _) = arg.split_once('=').unwrap();
            if key.is_empty()
                || key.starts_with(|c: char| c.is_ascii_digit())
                || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                || reserved_variable(key)
            {
                return manual(
                    "此环境变量可能改变 Shell 解析、查找、历史或协议状态，需要手动处理。",
                );
            }
        }
        return unknown("修改普通环境变量，影响后续命令；需要精确批准。");
    }
    if matches!(
        base,
        "bash" | "sh" | "dash" | "zsh" | "ksh" | "python" | "python3" | "node" | "ruby" | "perl"
    ) {
        if args.is_empty()
            || args.iter().any(|a| {
                a == "--interactive"
                    || a == "-"
                    || (a.starts_with('-') && !a.starts_with("--") && a[1..].contains('i'))
            })
            || !args.iter().any(|a| !a.starts_with('-'))
        {
            return manual("解释器缺少明确非交互载荷或要求交互输入，需要手动终端。");
        }
        return unknown("外部解释器运行不透明代码；需要批准完整载荷，批准不保证结束或安全。");
    }
    if base == "tail"
        && args.iter().any(|a| {
            a.starts_with("--follow")
                || a.starts_with('-') && !a.starts_with("--") && a[1..].contains(['f', 'F'])
        })
    {
        return manual("持续跟随日志没有普通完成边界，需要手动终端。");
    }
    if base == "journalctl"
        && args.iter().any(|a| {
            a == "--follow" || a.starts_with('-') && !a.starts_with("--") && a[1..].contains('f')
        })
    {
        return manual("journalctl 跟随输出需要手动终端。");
    }
    if base == "systemctl" && args.iter().any(|a| matches!(a.as_str(), "edit" | "--wait")) {
        return manual("systemctl 编辑器或持续等待模式需要手动终端。");
    }
    if base == "tee" && !finite_input {
        return manual("tee 的输入没有已识别的有限来源，可能等待终端或持续流，需要手动处理。");
    }
    if matches!(base, "cat" | "wc" | "head" | "tail" | "grep") && !input && !has_file_operand(args)
    {
        return manual("此命令可能等待终端输入，请提供文件或有限管道来源。");
    }
    if !input
        && ((matches!(base, "head" | "tail") && !count_args(args) && {
            let mut probe = args.to_vec();
            probe.push("input".into());
            count_args(&probe)
        }) || (base == "grep" && !grep_args(args, false) && grep_args(args, true)))
    {
        return manual(
            "没有明确受支持的文件输入，已知读取程序可能等待终端；请改用文件或有限管道。",
        );
    }
    if name.contains('/') {
        return unknown(
            "带路径的外部程序未经效果分类；可以批准一次前台尝试，程序可能失败、阻塞或修改数据。",
        );
    }
    if name == "docker" {
        return docker(args);
    }
    if name == "printf" && printf_mutates(args) {
        return manual("printf 变量赋值可能修改 Shell 协议状态，此模式需要手动处理。");
    }
    if name == "find" && args.iter().any(|a| matches!(a.as_str(), "-ok" | "-okdir")) {
        return manual("find 确认式执行需要交互输入。");
    }
    if name == "find"
        && args
            .iter()
            .any(|a| matches!(a.as_str(), "-delete" | "-fprint" | "-fprint0" | "-fprintf"))
    {
        return dangerous();
    }
    if name == "date"
        && args
            .iter()
            .any(|a| a == "-s" || a == "--set" || a.starts_with("--set="))
    {
        return dangerous();
    }
    if name == "hostname" && args.iter().any(|a| !a.starts_with('-')) {
        return dangerous();
    }
    if name == "systemctl" && systemctl_query(args) {
        return normal();
    }
    if name == "systemctl"
        && args.iter().any(|a| {
            matches!(
                a.as_str(),
                "status" | "list-units" | "list-unit-files" | "list-jobs"
            )
        })
        && !args.iter().any(|a| a == "--no-pager")
    {
        return manual("systemctl 查询可能打开分页器，请明确 --no-pager 或手动运行。");
    }
    if name == "journalctl" && !args.iter().any(|a| a == "--no-pager") {
        return manual("journalctl 可能打开分页器，请明确 --no-pager。");
    }
    if name == "systemctl" {
        return if args.iter().any(|a| {
            matches!(
                a.as_str(),
                "restart"
                    | "stop"
                    | "start"
                    | "reload"
                    | "reload-or-restart"
                    | "enable"
                    | "disable"
                    | "mask"
                    | "unmask"
                    | "daemon-reload"
                    | "daemon-reexec"
                    | "kill"
                    | "reboot"
                    | "poweroff"
            )
        }) {
            dangerous()
        } else {
            unknown("systemctl 的此操作或参数未经分类，需要核对影响并批准。")
        };
    }
    if matches!(
        name,
        "rm" | "rmdir"
            | "mv"
            | "cp"
            | "install"
            | "chmod"
            | "chown"
            | "chgrp"
            | "kill"
            | "killall"
            | "pkill"
            | "systemctl"
            | "service"
            | "reboot"
            | "shutdown"
            | "mount"
            | "umount"
            | "curl"
            | "wget"
            | "tee"
            | "dd"
            | "truncate"
            | "touch"
            | "mkdir"
            | "ln"
            | "apt"
            | "apt-get"
            | "dnf"
            | "yum"
    ) {
        return dangerous();
    }
    if name == "sed"
        && args
            .iter()
            .any(|a| a == "--in-place" || a.starts_with("--in-place=") || a.starts_with("-i"))
    {
        return dangerous();
    }
    let mut file_args = args.to_vec();
    if finite_input {
        file_args.retain(|a| a != "-");
    }
    let a = file_args.as_slice();
    let safe = match name {
        "pwd" => flags(args, "LP", &[], false),
        "true" | "false" | "whoami" => args.is_empty(),
        "echo" => !args.first().is_some_and(|arg| arg.starts_with('-')),
        "printf" => printf_args(args),
        "ps" => ps_args(args),
        "ss" => flags(
            args,
            "46aenoptulrxHm",
            &[
                "--all",
                "--listening",
                "--numeric",
                "--processes",
                "--tcp",
                "--udp",
                "--unix",
                "--no-header",
            ],
            false,
        ),
        "journalctl" => journal_args(args),
        "id" => flags(
            args,
            "ugGnrz",
            &[
                "--user", "--group", "--groups", "--name", "--real", "--zero",
            ],
            true,
        ),
        "uname" => flags(
            args,
            "asnrvmpio",
            &[
                "--all",
                "--kernel-name",
                "--nodename",
                "--kernel-release",
                "--kernel-version",
                "--machine",
                "--processor",
                "--hardware-platform",
                "--operating-system",
            ],
            false,
        ),
        "hostname" => flags(
            args,
            "fisd",
            &["--fqdn", "--short", "--ip-address", "--domain"],
            false,
        ),
        "uptime" => flags(args, "ps", &["--pretty", "--since"], false),
        "date" => {
            args.is_empty()
                || args.len() == 1
                    && (matches!(
                        args[0].as_str(),
                        "-u" | "--utc" | "-I" | "-R" | "--iso-8601"
                    ) || args[0].starts_with('+'))
        }
        "ls" => flags(
            args,
            "aAbBcCdDfFghHiIklLmnopqQrRsStTuUvwxX1Z",
            &[
                "--all",
                "--almost-all",
                "--directory",
                "--human-readable",
                "--reverse",
                "--recursive",
                "--size",
                "--color=never",
                "--group-directories-first",
            ],
            true,
        ),
        "df" => flags(
            args,
            "ahiHklmPT",
            &[
                "--all",
                "--human-readable",
                "--inodes",
                "--local",
                "--print-type",
            ],
            true,
        ),
        "du" => flags(
            args,
            "abcDhkLlmPsSx",
            &[
                "--all",
                "--bytes",
                "--total",
                "--human-readable",
                "--summarize",
                "--one-file-system",
            ],
            true,
        ),
        "free" => flags(
            args,
            "bkmghwlt",
            &[
                "--bytes", "--kibi", "--mebi", "--gibi", "--human", "--wide", "--total",
            ],
            false,
        ),
        "cat" => {
            (has_file_operand(a) || finite_input)
                && flags(
                    a,
                    "AbEenstTuv",
                    &[
                        "--show-all",
                        "--number",
                        "--number-nonblank",
                        "--squeeze-blank",
                        "--show-ends",
                        "--show-tabs",
                        "--show-nonprinting",
                    ],
                    true,
                )
        }
        "head" | "tail" => {
            let mut a = a.to_vec();
            if finite_input {
                a.push("__pipe_input__".into());
            }
            count_args(&a)
        }
        "wc" => {
            (has_file_operand(a) || finite_input)
                && flags(
                    a,
                    "clmwL",
                    &[
                        "--bytes",
                        "--chars",
                        "--lines",
                        "--words",
                        "--max-line-length",
                    ],
                    true,
                )
        }
        "stat" => {
            has_file_operand(args) && flags(args, "Lf", &["--dereference", "--file-system"], true)
        }
        "grep" => grep_args(args, finite_input),
        "find" => find_args(args),
        "cd" => {
            args.len() == 1 && (!args[0].starts_with('-') || args[0] == "-")
                || args.len() == 2 && args[0] == "--" && args[1] != "-"
        }
        _ => {
            return unknown(
                "未知程序或参数效果；需要批准完整命令的一次前台尝试，不保证结束、只读或成功。",
            )
        }
    };
    if safe {
        normal()
    } else {
        unknown("程序已识别，但参数、输入来源或操作效果不在自动只读规则内；需要精确批准。")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn known_queries_and_finite_pipelines_are_automatic() {
        for command in [
            "pwd -P",
            "ls -lah /var/log",
            "ls -- '/tmp/a b'",
            "cat /etc/os-release",
            "head -n 20 app.log",
            "tail -n 50 app.log",
            "wc -l README.md",
            "grep -n 'hello world' file.txt",
            "grep -A 2 -B3 -C 4 error app.log",
            "find /tmp -maxdepth 2 -name '*.log' -type f",
            "cd -- '/tmp/a b' && pwd -P",
            "df -h /",
            "uname -a",
            "date '+%Y-%m-%d'",
            "printf '%s' 'literal text'",
            "ps aux",
            "ps -o pid,cmd -p 1",
            "ss -lntup",
            "systemctl --no-pager status nginx",
            "systemctl status nginx --no-pager",
            "journalctl --no-pager -n 50 -u nginx",
            "ps aux | grep nginx",
            "printf '%s' 'a|b' | grep -C 2 'a|b' | wc -l",
            "printf '%s' x | cat - | head -n 1",
            "docker ps -a",
            "docker logs --tail 50 app",
            "docker logs -t app",
            "docker inspect app",
            "docker stats --no-stream app",
        ] {
            let a = classify(command, &[]);
            assert_eq!(a.risk, Risk::Normal, "{command}: {}", a.reason);
            assert!(a.executable, "{command}");
        }
    }
    #[test]
    fn all_branches_and_redirects_are_reviewed() {
        for command in [
            "rm -rf /tmp/example",
            "scp -o BatchMode=yes local remote:/tmp/",
            "rsync --dry-run source/ remote:/tmp/",
            "rsync source/ remote:/tmp/",
            "chmod 600 config",
            "curl https://example.test/data",
            "mkdir /tmp/new",
            "date -s 2030-01-01",
            "find /tmp -delete",
            "systemctl restart nginx",
            "sudo -n -u root id",
            "false && rm /tmp/x",
            "true || rm /tmp/x",
            "pwd; rm /tmp/x",
            "printf '%s' 'enabled=true' > /tmp/app.conf",
            "echo x >> out",
            "echo x | tee out",
            "docker restart app",
            "docker stop app",
            "docker rm app",
            "sed -i 's/a/b/' config",
        ] {
            let a = classify(command, &[]);
            assert_eq!(a.risk, Risk::Dangerous, "{command}: {}", a.reason);
            assert!(a.executable, "{command}");
        }
        let a = classify("echo x > first >> second", &[]);
        assert!(a.reason.contains("first") && a.reason.contains("second"));
        let a = classify("mystery && rm x", &[]);
        assert_eq!(a.risk, Risk::Dangerous);
        assert!(a.reason.contains("未知"));
    }
    #[test]
    fn unknown_is_approvable_not_a_program_whitelist() {
        for command in [
            "/opt/company/check-health --summary",
            "/tmp/command ls",
            "./script",
            "git status",
            "custom-tool --check",
            "bash -c 'pwd'",
            "python /srv/app/maintenance.py --check",
            "make -C /srv/app test",
            "sudo bash -c 'rm /tmp/x'",
            "command bash script",
            "ls --unsupported",
            "grep -中 file",
            "head --unsupported file",
            "cat < input.txt",
            "export APP_MODE=production",
            "docker exec app id",
            "docker run -d app",
            "docker build .",
            "docker --context production ps",
            "docker ps --unknown",
            "systemctl --no-pager status nginx --unknown",
            "mystery | grep x",
        ] {
            let a = classify(command, &[]);
            assert_ne!(a.risk, Risk::Normal, "{command}");
            assert!(a.executable, "{command}: {}", a.reason);
        }
    }
    #[test]
    fn unsupported_lifecycles_cannot_be_approved() {
        for command in [
            "cat",
            "cat -n",
            "wc -l",
            "head -n 20",
            "tail -n 20",
            "grep pattern",
            "cat /dev/zero",
            "head /dev/random",
            "cat /dev/tty",
            "echo $(id)",
            "ls *",
            "X=1 ls",
            "exit",
            "exec ls",
            "sudo -s",
            "tail -f app.log",
            "tail -nf app.log",
            "journalctl --no-pager -f",
            "vim /etc/app.conf",
            "systemctl edit nginx",
            "systemctl status nginx",
            "journalctl -n 20",
            "tee out",
            "mystery | tee out",
            "mystery | cat file | tee out",
            "sudo -n mystery | tee out",
            "python",
            "bash -i",
            "bash",
            "read value",
            "eval 'pwd'",
            "source file",
            "trap '' DEBUG",
            "export PATH=/tmp",
            "export PROMPT_COMMAND=bad",
            "export __xyz_nonce=bad",
            "printf -v PATH changed",
            "printf '%n' PATH",
            "printf '%10n' PATH",
            "printf -- '%n' PATH",
            "printf -vPATH changed",
            "let IFS=1",
            "getopts a PATH",
            "true || exit",
            "echo x | vim",
            "docker exec -it app sh",
            "docker run -dit app",
            "docker run --interactive app",
            "docker attach app",
            "docker logs -f app",
            "docker --context prod logs -f app",
            "docker stats",
        ] {
            let a = classify(command, &[]);
            assert!(!a.executable, "{command}: {}", a.reason);
            assert_eq!(a.risk, Risk::Unknown);
        }
    }
    #[test]
    fn shadowing_and_literal_data_are_preserved() {
        assert!(!classify("ls -la", &["ls".into()]).executable);
        assert!(!classify("echo x | grep x", &["grep".into()]).executable);
        assert_eq!(
            classify("command ls -lah /tmp", &["ls".into()]).risk,
            Risk::Normal
        );
        assert!(!classify("command ls", &["command".into()]).executable);
        assert!(!classify("sudo id", &["sudo".into()]).executable);
        assert_eq!(classify("ls '/tmp/$literal'", &[]).risk, Risk::Normal);
        assert_eq!(classify("grep -n 'a|b;$(id)' file", &[]).risk, Risk::Normal);
        assert_eq!(classify("/bin/ls", &["ls".into()]).risk, Risk::Unknown);
    }
}
