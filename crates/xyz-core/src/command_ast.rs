//! Bounded literal foreground grammar. Source spans are UTF-8 byte offsets;
//! this AST is for admission only and must never be used to reprint a payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Word {
    pub literal: String,
    pub start: usize,
    pub end: usize,
    quoted: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RedirectKind {
    Read,
    Write,
    Append,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Redirect {
    pub kind: RedirectKind,
    pub path: Word,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Simple {
    pub words: Vec<Word>,
    pub redirects: Vec<Redirect>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Pipeline {
    pub commands: Vec<Simple>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Conditional {
    And,
    Or,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AndOr {
    pub first: Pipeline,
    pub rest: Vec<(Conditional, Pipeline)>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Action {
    pub items: Vec<AndOr>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ParseError {
    ControlBytes,
    Incomplete,
    Unsupported,
    Expansion,
    Limit,
}
#[derive(Clone, Debug)]
enum Token {
    Word(Word),
    Pipe,
    And,
    Or,
    Semi,
    Redirect(RedirectKind),
}

fn lex(text: &str) -> Result<Vec<Token>, ParseError> {
    if text.len() > 8192 {
        return Err(ParseError::Limit);
    }
    if text.chars().any(char::is_control) {
        return Err(ParseError::ControlBytes);
    }
    if text.is_empty() || text.starts_with(char::is_whitespace) {
        return Err(ParseError::Incomplete);
    }
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut i = 0;
    let mut tokens = Vec::new();
    while i < chars.len() {
        let (start, c) = chars[i];
        if c == ' ' {
            i += 1;
            continue;
        }
        if matches!(c, '|' | '&' | ';' | '<' | '>') {
            let next = chars.get(i + 1).map(|p| p.1);
            let (token, count) = match (c, next) {
                ('|', Some('|')) => (Token::Or, 2),
                ('&', Some('&')) => (Token::And, 2),
                ('>', Some('>')) => (Token::Redirect(RedirectKind::Append), 2),
                ('>', Some('&' | '|')) | ('<', Some('<' | '&')) | ('|', Some('&')) | ('&', _) => {
                    return Err(ParseError::Unsupported)
                }
                ('|', _) => (Token::Pipe, 1),
                (';', _) => (Token::Semi, 1),
                ('<', _) => (Token::Redirect(RedirectKind::Read), 1),
                ('>', _) => (Token::Redirect(RedirectKind::Write), 1),
                _ => unreachable!(),
            };
            if matches!(token, Token::Redirect(_))
                && matches!(tokens.last(), Some(Token::Word(w)) if w.end == start && !w.quoted && w.literal.bytes().all(|b| b.is_ascii_digit()))
            {
                return Err(ParseError::Unsupported);
            }
            tokens.push(token);
            i += count;
            continue;
        }
        let mut value = String::new();
        let mut quote = None;
        let mut quoted = false;
        while i < chars.len() {
            let c = chars[i].1;
            if quote == Some('\'') {
                if c == '\'' {
                    quote = None;
                } else {
                    value.push(c);
                }
                i += 1;
                continue;
            }
            if quote == Some('"') {
                match c {
                    '"' => quote = None,
                    '$' | '`' | '!' => return Err(ParseError::Expansion),
                    '\\' => {
                        i += 1;
                        let next = chars.get(i).ok_or(ParseError::Incomplete)?.1;
                        if !matches!(next, '$' | '`' | '"' | '\\') {
                            value.push('\\');
                        }
                        value.push(next);
                    }
                    _ => value.push(c),
                }
                i += 1;
                continue;
            }
            match c {
                ' ' | '|' | '&' | ';' | '<' | '>' => break,
                '\'' | '"' => {
                    quote = Some(c);
                    quoted = true;
                }
                '\\' => {
                    i += 1;
                    value.push(chars.get(i).ok_or(ParseError::Incomplete)?.1);
                    quoted = true;
                }
                '$' | '`' | '*' | '?' | '[' | ']' | '~' | '{' | '}' => {
                    return Err(ParseError::Expansion)
                }
                '(' | ')' | '!' | '#' => return Err(ParseError::Unsupported),
                _ => value.push(c),
            }
            i += 1;
        }
        if quote.is_some() {
            return Err(ParseError::Incomplete);
        }
        let end = chars.get(i).map_or(text.len(), |p| p.0);
        tokens.push(Token::Word(Word {
            literal: value,
            start,
            end,
            quoted,
        }));
    }
    Ok(tokens)
}

pub(crate) fn parse(text: &str) -> Result<Action, ParseError> {
    let tokens = lex(text)?;
    struct Parser<'a> {
        tokens: &'a [Token],
        i: usize,
        simple_count: usize,
        text: &'a str,
    }
    impl Parser<'_> {
        fn simple(&mut self) -> Result<Simple, ParseError> {
            let Some(Token::Word(first)) = self.tokens.get(self.i) else {
                return Err(ParseError::Incomplete);
            };
            if first.literal.is_empty() {
                return Err(ParseError::Incomplete);
            }
            if !first.quoted
                && matches!(
                    first.literal.as_str(),
                    "if" | "then"
                        | "else"
                        | "elif"
                        | "fi"
                        | "for"
                        | "while"
                        | "until"
                        | "do"
                        | "done"
                        | "case"
                        | "esac"
                        | "in"
                        | "select"
                        | "function"
                        | "time"
                        | "coproc"
                )
            {
                return Err(ParseError::Unsupported);
            }
            let raw = &self.text[first.start..first.end];
            if let Some((key, _)) = raw.split_once('=') {
                if !key.is_empty()
                    && !key.starts_with(|c: char| c.is_ascii_digit())
                    && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                {
                    return Err(ParseError::Unsupported);
                }
            }
            self.simple_count += 1;
            if self.simple_count > 16 {
                return Err(ParseError::Limit);
            }
            let mut result = Simple {
                words: vec![first.clone()],
                redirects: vec![],
            };
            self.i += 1;
            loop {
                match self.tokens.get(self.i) {
                    Some(Token::Word(w)) => {
                        result.words.push(w.clone());
                        self.i += 1;
                    }
                    Some(Token::Redirect(kind)) => {
                        let Some(Token::Word(path)) = self.tokens.get(self.i + 1) else {
                            return Err(ParseError::Incomplete);
                        };
                        if matches!(
                            path.literal.as_str(),
                            "/dev/zero"
                                | "/dev/random"
                                | "/dev/urandom"
                                | "/dev/tty"
                                | "/proc/kmsg"
                        ) || path.literal.is_empty()
                            || [
                                "/dev/fd",
                                "/dev/std",
                                "/dev/tcp",
                                "/dev/udp",
                                "/proc/self/fd",
                            ]
                            .iter()
                            .any(|p| path.literal.starts_with(p))
                        {
                            return Err(ParseError::Unsupported);
                        }
                        result.redirects.push(Redirect {
                            kind: *kind,
                            path: path.clone(),
                        });
                        if result.redirects.len() > 8 {
                            return Err(ParseError::Limit);
                        }
                        self.i += 2;
                    }
                    _ => break,
                }
            }
            Ok(result)
        }
        fn pipeline(&mut self) -> Result<Pipeline, ParseError> {
            let mut commands = vec![self.simple()?];
            while matches!(self.tokens.get(self.i), Some(Token::Pipe)) {
                self.i += 1;
                commands.push(self.simple()?);
                if commands.len() > 8 {
                    return Err(ParseError::Limit);
                }
            }
            Ok(Pipeline { commands })
        }
        fn and_or(&mut self) -> Result<AndOr, ParseError> {
            let first = self.pipeline()?;
            let mut rest = vec![];
            while let Some(token @ (Token::And | Token::Or)) = self.tokens.get(self.i) {
                let operator = if matches!(token, Token::And) {
                    Conditional::And
                } else {
                    Conditional::Or
                };
                self.i += 1;
                rest.push((operator, self.pipeline()?));
            }
            Ok(AndOr { first, rest })
        }
    }
    let mut p = Parser {
        tokens: &tokens,
        i: 0,
        simple_count: 0,
        text,
    };
    let mut items = vec![p.and_or()?];
    while matches!(tokens.get(p.i), Some(Token::Semi)) {
        p.i += 1;
        items.push(p.and_or()?);
    }
    if p.i != tokens.len() {
        return Err(ParseError::Unsupported);
    }
    Ok(Action { items })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn precedence_spans_and_literal_operators() {
        let text = "printf '%s' '中|文' | cat && pwd || whoami; cd -- /tmp";
        let ast = parse(text).unwrap();
        assert_eq!(ast.items.len(), 2);
        assert_eq!(ast.items[0].first.commands.len(), 2);
        assert_eq!(
            ast.items[0].rest.iter().map(|v| v.0).collect::<Vec<_>>(),
            vec![Conditional::And, Conditional::Or]
        );
        let word = &ast.items[0].first.commands[0].words[2];
        assert_eq!(word.literal, "中|文");
        assert_eq!(&text[word.start..word.end], "'中|文'");
        assert_eq!(
            parse("echo a\\ b \"a\\q\" 'it'\\''s'").unwrap().items[0]
                .first
                .commands[0]
                .words
                .iter()
                .map(|w| w.literal.as_str())
                .collect::<Vec<_>>(),
            vec!["echo", "a b", "a\\q", "it's"]
        );
    }
    #[test]
    fn syntax_rejections() {
        for s in [
            "",
            " pwd",
            "pwd\n",
            "pwd\0",
            "echo $(id)",
            "echo `id`",
            "echo $'x'",
            "echo \"$HOME\"",
            "echo *",
            "echo {a,b}",
            "ls ~",
            "X='a' pwd",
            "if true",
            "time pwd",
            "(pwd)",
            "pwd &",
            "pwd |& cat",
            "pwd;",
            "pwd &&",
            "pwd || | cat",
            "echo 'x",
            "echo # hi",
            "> out",
            "echo 2>out",
            "echo >&2",
            "cat <<EOF",
            "cat <<<a",
            "echo >|out",
            "echo &>out",
            "cat </dev/fd/0",
            "cat </dev/stdin",
            "cat </dev/zero",
            "cat </dev/tty",
        ] {
            assert!(parse(s).is_err(), "{s}");
        }
        for s in [
            "echo '$(id); x&'",
            "echo name=value",
            "'if' literal",
            "echo 2 > out",
            "echo '2'>out",
            "echo \\$HOME",
            "printf '%s' a > x >> y < z",
        ] {
            assert!(parse(s).is_ok(), "{s}");
        }
    }
    #[test]
    fn exact_limits() {
        assert!(parse(&format!("echo {}", "a".repeat(8187))).is_ok());
        assert_eq!(
            parse(&format!("echo {}", "a".repeat(8188))),
            Err(ParseError::Limit)
        );
        assert_eq!(
            parse(&format!("echo {}", "中".repeat(2730))),
            Err(ParseError::Limit)
        );
        assert!(parse(&["pwd"; 16].join(";")).is_ok());
        assert!(parse(&["pwd"; 17].join(";")).is_err());
        assert!(parse(&["pwd"; 8].join("|")).is_ok());
        assert!(parse(&["pwd"; 9].join("|")).is_err());
        assert!(parse(&format!("echo x{}", " > out".repeat(8))).is_ok());
        assert!(parse(&format!("echo x{}", " > out".repeat(9))).is_err());
    }
    #[test]
    #[cfg(unix)]
    fn actual_bash_argv_agrees() {
        let bash = std::env::var("XYZ_TEST_BASH").unwrap_or_else(|_| "/bin/bash".into());
        for suffix in [
            r#"'a b' 'a|b;$(id)' "中 文""#,
            r#"a\ b \$HOME \; \|"#,
            r#""a\q" 'it'\''s' ab"cd"''"#,
            r#""a\\b" "\$HOME" "\`id\`""#,
            r#"'if' 'time' '2' name=value ''"#,
        ] {
            let source = format!("printf '%s\\0' {suffix}");
            let ast = parse(&source).unwrap();
            let expected: Vec<_> = ast.items[0].first.commands[0].words[2..]
                .iter()
                .map(|w| w.literal.as_bytes())
                .collect();
            let output = std::process::Command::new(&bash)
                .args(["--noprofile", "--norc", "-c", &source])
                .env_clear()
                .output()
                .expect("required actual Bash runtime");
            assert!(output.status.success(), "{source}");
            let mut actual: Vec<_> = output.stdout.split(|b| *b == 0).collect();
            assert_eq!(actual.pop(), Some(&b""[..]));
            assert_eq!(actual, expected, "{source}");
        }
    }
}
