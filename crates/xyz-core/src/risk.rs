//! Deliberately bounded host policy for literal, single-command Bash input.
//! This is not a shell sandbox: remote binaries, startup state and lifecycle
//! metadata are cooperative. Unknown syntax is never made safe by model labels.
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
    pub reason: &'static str,
}
fn normal() -> Assessment {
    Assessment { risk: Risk::Normal, executable: true, reason: "Host policy recognized a literal inspection or directory-navigation command and supported arguments. Requires the same integrated shell-ready snapshot; remote programs and concurrent manual input are not sandboxed." }
}
fn dangerous() -> Assessment {
    Assessment { risk: Risk::Dangerous, executable: true, reason: "This recognized operation can change data, permissions, services, identity or transmit data. Review the full command and target. Approval applies to this exact action and shell snapshot once." }
}
fn unknown(executable: bool, reason: &'static str) -> Assessment {
    Assessment {
        risk: Risk::Unknown,
        executable,
        reason,
    }
}

/// Tokenize a literal command, not arbitrary shell. Quoting is accepted only
/// when it cannot expand. Operators, substitutions, globbing, assignments,
/// comments, history expansion and control characters are outside the grammar.
fn words(command: &str) -> Option<Vec<String>> {
    let mut result = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut started = false;
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        if c.is_control() {
            return None;
        }
        if quote == Some('\'') {
            if c == '\'' {
                quote = None;
            } else {
                word.push(c);
            }
            continue;
        }
        if quote == Some('"') {
            match c {
                '"' => quote = None,
                '$' | '`' | '!' | '\\' => return None,
                _ => word.push(c),
            }
            continue;
        }
        match c {
            ' ' => {
                if started {
                    result.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            '\'' | '"' => {
                quote = Some(c);
                started = true;
            }
            '\\' => {
                let next = chars.next()?;
                if !matches!(next, ' ' | '\'' | '"' | '\\') {
                    return None;
                }
                word.push(next);
                started = true;
            }
            ';' | '&' | '|' | '<' | '>' | '$' | '`' | '(' | ')' | '{' | '}' | '*' | '?' | '['
            | ']' | '~' | '!' | '#' => return None,
            _ => {
                word.push(c);
                started = true;
            }
        }
    }
    if quote.is_some() {
        return None;
    }
    if started {
        result.push(word);
    }
    if result.is_empty() || result[0].contains('=') {
        None
    } else {
        Some(result)
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
fn systemctl_query(args: &[String]) -> bool {
    let Some(command) = args.iter().position(|v| !v.starts_with('-')) else {
        return false;
    };
    args[..command]
        .iter()
        .all(|v| matches!(v.as_str(), "--no-pager" | "--plain" | "--all" | "--full"))
        && args[..command].iter().any(|v| v == "--no-pager")
        && matches!(
            args[command].as_str(),
            "status"
                | "show"
                | "is-active"
                | "is-enabled"
                | "list-units"
                | "list-unit-files"
                | "list-jobs"
        )
        && args[command + 1..].iter().all(|v| !v.starts_with('-'))
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
fn literal_command(words: &[String]) -> String {
    words
        .iter()
        .map(|word| format!("'{}'", word.replace('\'', "'\\''")))
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn classify(command: &str, shadowed: &[String]) -> Assessment {
    classify_inner(command, shadowed, 0)
}
fn classify_inner(command: &str, shadowed: &[String], depth: u8) -> Assessment {
    if depth > 4 {
        return unknown(
            false,
            "Unknown risk: nested command prefixes exceed the supported bounded grammar.",
        );
    }
    if command.starts_with(char::is_whitespace) {
        return unknown(false, "Unknown risk: leading whitespace can suppress Bash command history and break exact lifecycle correlation. Remove the leading whitespace or use the manual terminal.");
    }
    let Some(tokens) = words(command) else {
        return unknown(false, "Unknown risk: unsupported shell syntax or expansion. Automatic execution and approval-through-this-adapter are unavailable; inspect or run this operation manually.");
    };
    let name = tokens[0].as_str();
    if name.is_empty() || name.contains('/') || shadowed.iter().any(|n| n == name) {
        return unknown(false, "Unknown risk: this command is a path, alias or shell function, so its actual effects are not established. Use the manual terminal after review.");
    }
    let args = &tokens[1..];
    // Prefixes are parsed explicitly. `command` bypasses aliases/functions
    // only when that builtin name itself was not shadowed in this ready epoch.
    if name == "command" {
        let mut inner = args;
        if inner
            .first()
            .is_some_and(|v| matches!(v.as_str(), "-p" | "--"))
        {
            inner = &inner[1..];
        }
        if inner.is_empty() || inner[0].starts_with('-') {
            return unknown(
                false,
                "Unknown risk: unsupported command-builtin mode or missing literal subcommand.",
            );
        }
        return classify_inner(&literal_command(inner), &[], depth + 1);
    }
    if name == "sudo" {
        let mut index = 0;
        while index < args.len() && args[index].starts_with('-') {
            match args[index].as_str() {
                "--" => { index += 1; break; },
                "-n" | "--non-interactive" | "-H" | "--set-home" => {},
                "-u" | "--user" | "-g" | "--group" => {
                    index += 1;
                    if args.get(index).is_none_or(|v| v.is_empty() || v.starts_with('-')) { return unknown(false, "Unknown risk: sudo requires a literal user/group and a supported standalone subcommand."); }
                }
                _ => return unknown(false, "Unknown risk: this sudo mode can enter a shell, read a script or change an unsupported execution context. Use the manual terminal."),
            }
            index += 1;
        }
        if index == args.len() {
            return unknown(
                false,
                "Unknown risk: sudo needs an explicit supported standalone subcommand.",
            );
        }
        let nested = classify_inner(&literal_command(&args[index..]), &[], depth + 1);
        return if nested.executable {
            dangerous()
        } else {
            unknown(false, "Unknown risk: the sudo subcommand is a script, nested interpreter, interactive workflow or unsupported program. Perform it manually after review.")
        };
    }
    if name == "su" {
        return unknown(false, "Unknown risk: identity switching creates a new shell context. Perform it manually and establish the new shell separately.");
    }
    if name == "find"
        && args.iter().any(|a| {
            matches!(
                a.as_str(),
                "-exec" | "-execdir" | "-ok" | "-okdir" | "-fprint" | "-fprint0" | "-fprintf"
            )
        })
    {
        return unknown(false, "Unknown risk: this find operation invokes another program or writes through an unsupported action. Use the manual terminal after reviewing its complete effects.");
    }
    if name == "find" && args.iter().any(|a| a == "-delete") {
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
    if name == "tail"
        && args
            .iter()
            .any(|a| a == "-f" || a == "-F" || a.starts_with("--follow"))
    {
        return unknown(false, "Unknown risk: a following stream has no ordinary completion boundary. Use the manual terminal for this interactive or long-lived workflow.");
    }
    if name == "systemctl" && systemctl_query(args) {
        return normal();
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
            | "sudo"
            | "su"
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
            | "scp"
            | "rsync"
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
    let safe = match name {
        "pwd" => flags(args, "LP", &[], false),
        "echo" => !args.first().is_some_and(|arg| arg.starts_with('-')),
        "printf" => printf_args(args),
        "ps" => ps_args(args),
        "ss" => flags(args, "46aenoptulrxHm", &["--all", "--listening", "--numeric", "--processes", "--tcp", "--udp", "--unix", "--no-header"], false),
        "journalctl" => journal_args(args),
        "whoami" => args.is_empty(),
        "id" => flags(args, "ugGnrz", &["--user", "--group", "--groups", "--name", "--real", "--zero"], true),
        "uname" => flags(args, "asnrvmpio", &["--all", "--kernel-name", "--nodename", "--kernel-release", "--kernel-version", "--machine", "--processor", "--hardware-platform", "--operating-system"], false),
        "hostname" => flags(args, "fisd", &["--fqdn", "--short", "--ip-address", "--domain"], false),
        "uptime" => flags(args, "ps", &["--pretty", "--since"], false),
        "date" => args.is_empty() || (args.len() == 1 && (matches!(args[0].as_str(), "-u" | "--utc" | "-I" | "-R" | "--iso-8601") || args[0].starts_with('+'))),
        "ls" => flags(args, "aAbBcCdDfFghHiIklLmnopqQrRsStTuUvwxX1Z", &["--all", "--almost-all", "--directory", "--human-readable", "--reverse", "--recursive", "--size", "--color=never", "--group-directories-first"], true),
        "df" => flags(args, "ahiHklmPT", &["--all", "--human-readable", "--inodes", "--local", "--print-type"], true),
        "du" => flags(args, "abcDhkLlmPsSx", &["--all", "--bytes", "--total", "--human-readable", "--summarize", "--one-file-system"], true),
        "free" => flags(args, "bkmghwlt", &["--bytes", "--kibi", "--mebi", "--gibi", "--human", "--wide", "--total"], false),
        "cat" => has_file_operand(args) && flags(args, "AbEenstTuv", &["--show-all", "--number", "--number-nonblank", "--squeeze-blank", "--show-ends", "--show-tabs", "--show-nonprinting"], true),
        "head" | "tail" => count_args(args),
        "wc" => has_file_operand(args) && flags(args, "clmwL", &["--bytes", "--chars", "--lines", "--words", "--max-line-length"], true),
        "stat" => has_file_operand(args) && flags(args, "Lf", &["--dereference", "--file-system"], true),
        "grep" => args.iter().filter(|v| !v.starts_with('-')).count() >= 2 && flags(args, "EFGivwnxclLhHsqroabU", &["--extended-regexp", "--fixed-strings", "--ignore-case", "--line-number", "--invert-match", "--word-regexp", "--line-regexp", "--count", "--files-with-matches", "--files-without-match", "--no-messages", "--recursive"], true),
        "find" => find_args(args),
        "cd" => args.len() == 1 && (!args[0].starts_with('-') || args[0] == "-") || args.len() == 2 && args[0] == "--" && args[1] != "-",
        _ => return unknown(false, "Unknown risk: this program, interpreter or interactive workflow is outside the host's supported command policy. Review and use the manual terminal; model-provided safety labels cannot authorize it."),
    };
    if safe {
        normal()
    } else {
        unknown(true, "Unknown risk: the literal program is recognized, but these options or operands are outside the normal policy. Review the full behavior before approving this exact action once; approval does not classify it as safe.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn common_literal_arguments_are_normal() {
        for command in [
            "pwd -P",
            "ls -lah /var/log",
            "ls -- '/tmp/a b'",
            "cat /etc/os-release",
            "head -n 20 /var/log/app.log",
            "tail -n 50 app.log",
            "wc -l README.md",
            "grep -n 'hello world' file.txt",
            "find /tmp -maxdepth 2 -name '*.log' -type f",
            "cd -- '/tmp/a b'",
            "df -h /",
            "uname -a",
            "date '+%Y-%m-%d'",
            "echo 'literal text'",
            "printf '%s' 'literal text'",
            "ps aux",
            "ps -o pid,cmd -p 1",
            "ss -lntup",
            "systemctl --no-pager status nginx",
            "journalctl --no-pager -n 50 -u nginx",
        ] {
            assert_eq!(classify(command, &[]).risk, Risk::Normal, "{command}");
        }
    }
    #[test]
    fn effects_and_unknowns_remain_distinct() {
        for command in [
            "rm -rf /tmp/example",
            "chmod 600 config",
            "curl https://example.test/data",
            "mkdir /tmp/new",
            "date -s 2030-01-01",
            "find /tmp -delete",
            "systemctl restart nginx",
            "sudo systemctl restart nginx",
            "sudo -n -u root id",
        ] {
            assert_eq!(classify(command, &[]).risk, Risk::Dangerous);
        }
        for command in [
            "pwd; rm /tmp/x",
            "echo $(id)",
            "ls > output",
            "ls *",
            "X=1 ls",
            "bash -c 'pwd'",
            "python app.py",
            "exit",
            "exec ls",
            "./script",
            "git status",
            "sudo -s",
            "sudo -- sh -c pwd",
            "command bash script",
            "sudo bash -c 'rm /tmp/x'",
            "tail -f app.log",
            "find /tmp -exec sh script",
            "ls # comment",
        ] {
            let assessment = classify(command, &[]);
            assert_eq!(assessment.risk, Risk::Unknown, "{command}");
            assert!(!assessment.executable, "{command}");
        }
        for command in [
            "ls --unsupported",
            "head --unsupported file",
            "printf '%n' PATH",
            "printf -v PATH changed",
        ] {
            let assessment = classify(command, &[]);
            assert_eq!(assessment.risk, Risk::Unknown, "{command}");
            assert!(assessment.executable);
        }
    }
    #[test]
    fn aliases_functions_and_quoted_expansion_fail_closed() {
        assert_eq!(classify("ls -la", &["ls".into()]).risk, Risk::Unknown);
        for command in [
            "ls \"$HOME\"",
            "ls `pwd`",
            "ls \"a\\b\"",
            "ls 'unterminated",
            "ls \\x",
            "ls a\nwhoami",
        ] {
            assert_eq!(classify(command, &[]).risk, Risk::Unknown);
        }
        assert_eq!(classify("ls '/tmp/$literal'", &[]).risk, Risk::Normal);
    }
    #[test]
    fn explicit_command_prefix_bypasses_only_nested_shadowed_names() {
        assert_eq!(
            classify("command ls -lah /tmp", &["ls".into()]).risk,
            Risk::Normal
        );
        assert_eq!(
            classify("command -p ls -- '/tmp/a b'", &["ls".into()]).risk,
            Risk::Normal
        );
        assert_eq!(
            classify("command ls -la", &["command".into()]).risk,
            Risk::Unknown
        );
        assert_eq!(
            classify("sudo systemctl restart nginx", &["sudo".into()]).risk,
            Risk::Unknown
        );
        assert_eq!(
            classify("command sudo -n id", &["sudo".into()]).risk,
            Risk::Dangerous
        );
    }
    #[test]
    fn stdin_readers_need_an_explicit_file_for_normal_execution() {
        for command in [
            "cat",
            "cat -n",
            "cat --",
            "wc -l",
            "wc --",
            "head --",
            "tail --",
            "head -n 20",
            "tail -n 20",
        ] {
            assert_eq!(classify(command, &[]).risk, Risk::Unknown, "{command}");
        }
        for command in [
            "cat -n -- file",
            "wc -l -- file",
            "head -- file",
            "tail -- file",
        ] {
            assert_eq!(classify(command, &[]).risk, Risk::Normal, "{command}");
        }
    }
}
