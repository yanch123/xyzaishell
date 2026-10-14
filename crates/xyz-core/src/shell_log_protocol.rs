//! Opt-in, untrusted shell metadata. This module cannot execute commands or grant
//! AI permissions. The nonce correlates a stream; it does NOT authenticate it.
use base64::{engine::general_purpose::STANDARD, Engine};

pub const MAX_FRAME_BYTES: usize = 32 * 1024;
pub const MAX_COMMAND_BYTES: usize = 16 * 1024;
pub const MAX_CWD_BYTES: usize = 4 * 1024;
const PREFIX: &[u8] = b"\x1b]777;xyzlog;";
const SCRIPT: &str = include_str!("shell_log_bootstrap.bash");

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WireEvent {
    Closed,
    Malformed {
        reason: String,
    },
    Hello {
        shell_version: String,
    },
    Begin {
        sequence: u64,
        remote_time_s: u64,
        cwd: String,
        command: String,
    },
    End {
        sequence: u64,
        remote_time_s: u64,
        exit_code: u8,
    },
    Gap {
        sequence: u64,
        reason: String,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtocolError(pub &'static str);
impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for ProtocolError {}
fn valid_nonce(s: &str) -> bool {
    s.len() == 32
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
/// Send only after explicit user approval at an idle POSIX-family shell prompt.
/// Fish/csh and input owned by an application are not supported. Never auto-retry.
pub fn bootstrap_command(nonce: &str) -> Result<String, ProtocolError> {
    if !valid_nonce(nonce) {
        return Err(ProtocolError("invalid integration nonce"));
    }
    let script = SCRIPT.replace("@NONCE@", nonce);
    // Process substitution is parsed by the inner Bash, not the user's /bin/sh.
    // The temporary rc is a pipe FD, never a persistent remote file.
    let launcher = r#"if (( BASH_VERSINFO[0] < 4 || (BASH_VERSINFO[0] == 4 && BASH_VERSINFO[1] < 4) )); then printf '%s\n' 'xyzaishell: Bash 4.4 or newer is required'; exit 1; fi; command -v base64 >/dev/null && command -v tr >/dev/null || { printf '%s\n' 'xyzaishell: base64 and tr are required'; exit 1; }; exec bash --noprofile --rcfile <(builtin printf '%s' "$1") -i"#;
    let command = format!("env -u BASH_ENV -u ENV -u SHELLOPTS -u BASHOPTS bash --noprofile --norc -c {} xyzaishell {}\n", quote(launcher), quote(&script));
    if command.len() > 16 * 1024 {
        return Err(ProtocolError("bootstrap exceeds SSH input frame limit"));
    }
    Ok(command)
}

#[derive(Default, Debug)]
pub struct ParseOutput {
    /// Ordinary output with this protocol namespace removed, for model observation.
    /// Render the original SSH bytes in the visible terminal instead.
    pub visible: Vec<u8>,
    pub events: Vec<WireEvent>,
}
/// Incremental, bounded-memory scanner. Malformed metadata is dropped, never
/// interpreted as terminal input, a command, a permission, or model content.
pub struct ProtocolParser {
    nonce: String,
    prefix: Vec<u8>,
    frame: Option<Vec<u8>>,
    oversized: bool,
    frame_escape: bool,
}
impl ProtocolParser {
    pub fn new(nonce: &str) -> Result<Self, ProtocolError> {
        if !valid_nonce(nonce) {
            return Err(ProtocolError("invalid integration nonce"));
        }
        Ok(Self {
            nonce: nonce.into(),
            prefix: Vec::new(),
            frame: None,
            oversized: false,
            frame_escape: false,
        })
    }
    pub fn feed(&mut self, input: &[u8]) -> ParseOutput {
        let mut out = ParseOutput::default();
        for &byte in input {
            if self.frame.is_some() {
                let terminated = byte == 7 || (self.frame_escape && byte == b'\\');
                if terminated {
                    let mut frame = self.frame.take().unwrap();
                    if self.frame_escape && byte == b'\\' {
                        frame.pop();
                    }
                    if !self.oversized {
                        if let Some(event) = parse_frame(&self.nonce, &frame) {
                            out.events.push(event);
                        }
                    }
                    self.oversized = false;
                    self.frame_escape = false;
                    continue;
                }
                self.frame_escape = byte == 0x1b;
                if !self.oversized {
                    let frame = self.frame.as_mut().unwrap();
                    if frame.len() < MAX_FRAME_BYTES - PREFIX.len() - 2 {
                        frame.push(byte);
                    } else {
                        if expected_frame(&self.nonce, frame) {
                            out.events.push(WireEvent::Malformed {
                                reason: "oversize_frame".into(),
                            });
                        }
                        frame.clear();
                        self.oversized = true;
                    }
                }
                continue;
            }
            self.prefix.push(byte);
            while !PREFIX.starts_with(&self.prefix) {
                out.visible.push(self.prefix.remove(0));
            }
            if self.prefix == PREFIX {
                self.prefix.clear();
                self.frame = Some(Vec::new());
            }
        }
        out
    }
    /// Disconnect/reset: flush ordinary prefix bytes, discard incomplete metadata.
    /// Caller must independently finish any active command with unknown outcome.
    pub fn finish(&mut self) -> Vec<u8> {
        self.frame = None;
        self.oversized = false;
        self.frame_escape = false;
        std::mem::take(&mut self.prefix)
    }
}
fn number(s: &str) -> Option<u64> {
    if s.is_empty()
        || s.len() > 20
        || !s.bytes().all(|b| b.is_ascii_digit())
        || (s.len() > 1 && s.starts_with('0'))
    {
        return None;
    }
    s.parse().ok()
}
fn sequence(s: &str) -> Option<u64> {
    number(s).filter(|n| *n > 0)
}
fn timestamp(s: &str) -> Option<u64> {
    number(s).filter(|n| *n <= 253_402_300_799)
}
fn decoded(s: &str, max: usize) -> Result<String, &'static str> {
    if s.len() > max.div_ceil(3) * 4 {
        return Err("oversize");
    }
    let bytes = STANDARD.decode(s).map_err(|_| "invalid_base64")?;
    if bytes.len() > max {
        return Err("oversize");
    }
    if bytes.contains(&0) {
        return Err("invalid_encoding");
    }
    String::from_utf8(bytes).map_err(|_| "invalid_encoding")
}
fn expected_frame(nonce: &str, frame: &[u8]) -> bool {
    frame.split(|b| *b == b';').nth(1) == Some(nonce.as_bytes())
}
fn parse_frame(nonce: &str, frame: &[u8]) -> Option<WireEvent> {
    if !expected_frame(nonce, frame) {
        return None;
    }
    Some(
        parse_valid_frame(nonce, frame).unwrap_or_else(|| WireEvent::Malformed {
            reason: "invalid_frame".into(),
        }),
    )
}
fn parse_valid_frame(nonce: &str, frame: &[u8]) -> Option<WireEvent> {
    let text = std::str::from_utf8(frame).ok()?;
    let fields: Vec<_> = text.split(';').take(9).collect();
    if fields.len() < 3 || fields[0] != "1" || fields[1] != nonce {
        return None;
    }
    match fields[2] {
        "X" if fields.len() == 3 => Some(WireEvent::Closed),
        "H" if fields.len() == 4
            && !fields[3].is_empty()
            && fields[3].len() <= 64
            && fields[3].bytes().all(|b| b.is_ascii_graphic()) =>
        {
            Some(WireEvent::Hello {
                shell_version: fields[3].into(),
            })
        }
        "B" if fields.len() == 7 => {
            let sequence = sequence(fields[3])?;
            let remote_time_s = timestamp(fields[4])?;
            let cwd = decoded(fields[5], MAX_CWD_BYTES);
            let command = decoded(fields[6], MAX_COMMAND_BYTES);
            match (cwd, command) {
                (Ok(cwd), Ok(command)) if cwd.starts_with('/') && !command.is_empty() => {
                    Some(WireEvent::Begin {
                        sequence,
                        remote_time_s,
                        cwd,
                        command,
                    })
                }
                (Err(reason), _) | (_, Err(reason)) => Some(WireEvent::Gap {
                    sequence,
                    reason: reason.into(),
                }),
                _ => Some(WireEvent::Gap {
                    sequence,
                    reason: "invalid_content".into(),
                }),
            }
        }
        "E" if fields.len() == 6 => Some(WireEvent::End {
            sequence: sequence(fields[3])?,
            remote_time_s: timestamp(fields[4])?,
            exit_code: number(fields[5])?.try_into().ok()?,
        }),
        "G" if fields.len() == 5
            && matches!(
                fields[4],
                "history_settings" | "history_missing" | "cwd_unavailable" | "oversize"
            ) =>
        {
            Some(WireEvent::Gap {
                sequence: sequence(fields[3])?,
                reason: fields[4].into(),
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const N: &str = "0123456789abcdef0123456789abcdef";
    fn frame(body: &str) -> Vec<u8> {
        format!("\x1b]777;xyzlog;1;{N};{body}\x07").into_bytes()
    }
    #[test]
    fn every_split_and_bytewise_preserve_complete_text() {
        let command = "cat <<'END'\nhello\nEND\n";
        let bytes = frame(&format!(
            "B;1;123;{};{}",
            STANDARD.encode(b"/tmp/\npath"),
            STANDARD.encode(command)
        ));
        for split in 0..=bytes.len() {
            let mut p = ProtocolParser::new(N).unwrap();
            let mut first = p.feed(&bytes[..split]);
            let second = p.feed(&bytes[split..]);
            first.events.extend(second.events);
            first.visible.extend(second.visible);
            assert!(first.visible.is_empty());
            assert_eq!(
                first.events,
                vec![WireEvent::Begin {
                    sequence: 1,
                    remote_time_s: 123,
                    cwd: "/tmp/\npath".into(),
                    command: command.into()
                }]
            );
        }
        let mut p = ProtocolParser::new(N).unwrap();
        let all: Vec<_> = bytes.iter().flat_map(|b| p.feed(&[*b]).events).collect();
        assert_eq!(all.len(), 1);
    }
    #[test]
    fn foreign_output_retained_metadata_removed_and_st_accepted() {
        let mut p = ProtocolParser::new(N).unwrap();
        let mut bytes = b"abc\x1b]0;title\x07def".to_vec();
        let mut f = frame("E;1;123;17");
        f.pop();
        f.extend(b"\x1b\\");
        bytes.extend(f);
        bytes.extend(b"tail");
        let out = p.feed(&bytes);
        assert_eq!(out.visible, b"abc\x1b]0;title\x07deftail");
        assert_eq!(
            out.events,
            vec![WireEvent::End {
                sequence: 1,
                remote_time_s: 123,
                exit_code: 17
            }]
        );
    }
    #[test]
    fn malformed_hostile_and_oversized_inputs_are_bounded() {
        let mut p = ProtocolParser::new(N).unwrap();
        let mut hostile = PREFIX.to_vec();
        hostile.extend(vec![b'x'; MAX_FRAME_BYTES * 4]);
        assert!(p.feed(&hostile).events.is_empty());
        assert_eq!(p.frame.as_ref().unwrap().len(), 0);
        assert!(p.feed(b"\x07good").events.is_empty());
        assert_eq!(p.feed(&frame("E;1;123;0")).events.len(), 1);
        let mut expected_hostile = format!("\x1b]777;xyzlog;1;{N};B;").into_bytes();
        expected_hostile.extend(vec![b'x'; MAX_FRAME_BYTES * 2]);
        assert_eq!(
            p.feed(&expected_hostile).events,
            vec![WireEvent::Malformed {
                reason: "oversize_frame".into()
            }]
        );
        p.feed(b"\x07");
        assert_eq!(p.feed(&frame("X")).events, vec![WireEvent::Closed]);

        for body in [
            "E;0;123;0",
            "E;1;123;256",
            "E;01;123;0",
            "E;1;123;0;extra",
            "H;x\n",
            "B;1;123;Lw==;YQ==;extra",
            "E;18446744073709551616;1;0",
        ] {
            assert!(
                matches!(
                    p.feed(&frame(body)).events.as_slice(),
                    [WireEvent::Malformed { .. }]
                ),
                "{body}"
            );
        }
        let wrong = String::from_utf8(frame("E;1;123;0"))
            .unwrap()
            .replace(N, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let out = p.feed(wrong.as_bytes());
        assert!(out.events.is_empty());
        assert!(out.visible.is_empty());
    }
    #[test]
    fn invalid_utf8_and_control_bytes_never_become_executable_text() {
        let mut p = ProtocolParser::new(N).unwrap();
        assert_eq!(
            p.feed(&frame("B;1;123;Lw==;/w==")).events,
            vec![WireEvent::Gap {
                sequence: 1,
                reason: "invalid_encoding".into()
            }]
        );
        let cmd = "printf '\u{1b}]evil\u{7}'";
        let out = p.feed(&frame(&format!("B;2;123;Lw==;{}", STANDARD.encode(cmd))));
        assert!(matches!(&out.events[0], WireEvent::Begin { command, .. } if command == cmd));
        assert!(out.visible.is_empty());
    }
    #[test]
    fn reset_drops_partial_metadata_and_bootstrap_fits_one_input_frame() {
        let mut p = ProtocolParser::new(N).unwrap();
        p.feed(b"\x1b]777;xyzlog;unfinished");
        assert!(p.finish().is_empty());
        assert_eq!(p.feed(b"normal").visible, b"normal");
        let command = bootstrap_command(N).unwrap();
        assert!(command.len() < 16 * 1024);
        assert!(command.ends_with('\n'));
        assert!(!command.contains("@NONCE@"));
        assert!(bootstrap_command("'; evil").is_err());
    }
}
