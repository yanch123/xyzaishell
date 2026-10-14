//! Bounded, incremental observations of the visible SSH PTY.
//!
//! Capture starts only at `begin`; there is no terminal-history source here.
//! Terminal output remains untrusted after control filtering and exact, known-
//! secret redaction. Unknown secrets and encoded/transformed secrets are not
//! covered. This is a text stream, not a terminal emulator or completion proof.
use crate::ssh::{ApiError, SessionRef};
use serde::Serialize;
use std::sync::{Arc, Mutex, MutexGuard};
use zeroize::{Zeroize, Zeroizing};

pub const MAX_OBSERVATION_BYTES: usize = 16 * 1024;
const MAX_REDACT_TEXT_BYTES: usize = 64 * 1024;
const MAX_SECRET_BYTES: usize = MAX_OBSERVATION_BYTES;
const MAX_TOTAL_SECRET_BYTES: usize = 64 * 1024;
const MAX_SECRETS: usize = 128;
const SANITIZE_CHUNK_BYTES: usize = 4096;
const REDACTED: &str = "[REDACTED]";

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Observation {
    pub text: String,
    /// Some unread text was discarded to enforce the observation limit.
    pub truncated: bool,
    /// The observation is incomplete (overflow, invalid UTF-8, or fail-closed capture).
    pub gap: bool,
    /// Filtering never makes remote output a trusted instruction source.
    pub untrusted: bool,
}

#[derive(Clone, Default)]
pub struct ObservationBuffer {
    state: Arc<Mutex<State>>,
}

impl ObservationBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a fresh capture epoch for exactly this connection generation.
    ///
    /// Oversized secret inputs disable capture rather than silently dropping a
    /// secret from the filter. `read` then returns empty text with gap/truncation
    /// flags until the next valid `begin` or `reset`.
    pub fn begin(&self, session: SessionRef, known_secrets: Vec<String>) {
        let mut secrets = Zeroizing::new(known_secrets);
        let mut current = self.lock();
        let same_stream = current.stream_session.as_ref() == Some(&session);
        let mut context = Zeroizing::new(String::new());
        let mut state = State {
            session: Some(session.clone()),
            stream_session: Some(session),
            registered: std::mem::take(&mut current.registered),
            ..State::default()
        };
        if same_stream {
            state.sanitizer = std::mem::take(&mut current.sanitizer);
            state.sanitizer.epoch_pending = state.sanitizer.utf8_len > 0;
            context = std::mem::take(&mut current.redactor.pending);
        }
        state.blocked = !valid_secret_budget(
            state
                .registered
                .iter()
                .map(|s| s.as_str())
                .chain(secrets.iter().map(String::as_str)),
        );
        if !state.blocked {
            state.run_secrets = secrets
                .drain(..)
                .filter(|s| !s.is_empty())
                .map(Zeroizing::new)
                .collect();
            state.rebuild_redactor();
            // Preserve only parser/matching context across epochs. Old pending
            // bytes are excluded from output, even if a future chunk resolves
            // their prefix as ordinary text instead of a secret.
            state.capture_hidden(&context);
        }
        *current = state;
    }

    /// Register a connection credential independently of capture epochs. Empty
    /// values are ignored. Registration is intended to precede authentication;
    /// it cannot retract text that was already read. An active registration
    /// change discards unread text and signals a gap before restarting filters.
    pub fn add_known_secret(&self, secret: &str) -> Result<(), ApiError> {
        if secret.is_empty() {
            return Ok(());
        }
        let mut state = self.lock();
        if state.registered.iter().any(|s| s.as_str() == secret) {
            return Ok(());
        }
        if !valid_secret_budget(
            state
                .registered
                .iter()
                .chain(&state.run_secrets)
                .map(|s| s.as_str())
                .chain(std::iter::once(secret)),
        ) {
            state.blocked = true;
            state.tail = Tail::default();
            return Err(ApiError::new(
                "OBSERVATION_SECRET_LIMIT",
                "The known-secret filter limit was exceeded; capture is disabled.",
            ));
        }
        state.registered.push(Zeroizing::new(secret.to_owned()));
        state.rebuild_redactor();
        state.tail = Tail::default();
        state.tail.gap = state.session.is_some();
        // Keep the escape parser: a credential registered during an OSC/DCS
        // string must not expose the remaining payload as ordinary text.
        Ok(())
    }

    /// Independently sanitize/redact bounded UI or model text without consuming
    /// the PTY cursor. An unresolved secret prefix is masked, not disclosed.
    /// This is deliberately conservative when used to reject unsafe commands.
    pub fn redact_text(&self, text: &str) -> String {
        self.redact_inner(text, false)
    }

    /// Chat prose keeps line breaks and indentation. Terminal escape payloads
    /// and bidi controls remain suppressed; known secrets stay prefix-safe.
    pub fn redact_prose(&self, text: &str) -> String {
        self.redact_inner(text, true)
    }

    fn redact_inner(&self, text: &str, prose: bool) -> String {
        let state = self.lock();
        if state.blocked || text.len() > MAX_REDACT_TEXT_BYTES {
            return REDACTED.to_owned();
        }
        let mut redactor = Redactor {
            patterns: state
                .redactor
                .patterns
                .iter()
                .map(|p| Pattern::new(p.text.clone()))
                .collect(),
            ..Redactor::default()
        };
        if prose {
            for secret in state.registered.iter().chain(&state.run_secrets) {
                let mut sanitizer = Sanitizer {
                    preserve_whitespace: true,
                    ..Sanitizer::default()
                };
                let normalized: Zeroizing<String> =
                    Zeroizing::new(secret.bytes().filter_map(|b| sanitizer.push(b)).collect());
                if !normalized.is_empty()
                    && !redactor
                        .patterns
                        .iter()
                        .any(|p| p.text.as_str() == normalized.as_str())
                {
                    redactor.patterns.push(Pattern::new(normalized));
                }
            }
        }
        drop(state);
        let mut sanitizer = Sanitizer {
            preserve_whitespace: prose,
            ..Sanitizer::default()
        };
        let mut tail = Tail::default();
        tail.limit = MAX_REDACT_TEXT_BYTES;
        let mut chunk = Zeroizing::new(String::with_capacity(SANITIZE_CHUNK_BYTES + 4));
        for &byte in text.as_bytes() {
            if let Some(ch) = sanitizer.push(byte) {
                chunk.push(ch);
                if chunk.len() >= SANITIZE_CHUNK_BYTES {
                    redactor.push(&chunk, &mut tail);
                    chunk.zeroize();
                }
            }
        }
        redactor.push(&chunk, &mut tail);
        if !redactor.pending.is_empty() {
            tail.push(REDACTED);
        }
        std::mem::take(&mut tail.text)
    }

    /// Immutable-in-practice per-run redaction rules, detached from later SSH
    /// reconnect/reset. Contains no captured output or shared stream cursor.
    pub(crate) fn detached_redactor(&self) -> Self {
        let current = self.lock();
        let mut state = State {
            registered: current.registered.clone(),
            run_secrets: current.run_secrets.clone(),
            blocked: current.blocked,
            ..State::default()
        };
        if !state.blocked {
            state.rebuild_redactor();
        }
        Self {
            state: Arc::new(Mutex::new(state)),
        }
    }

    /// Feed the same bytes delivered by the visible SSH output path. Stale or
    /// unrelated sessions, and output before capture starts, are ignored.
    pub fn push(&self, session: &SessionRef, bytes: &[u8]) {
        let mut state = self.lock();
        if state.blocked
            || state
                .session
                .as_ref()
                .is_some_and(|active| active != session)
        {
            return;
        }
        if state.stream_session.as_ref() != Some(session) {
            state.stream_session = Some(session.clone());
            state.sanitizer = Sanitizer::default();
            state.rebuild_redactor();
        }
        let mut chunk = Zeroizing::new(String::with_capacity(SANITIZE_CHUNK_BYTES + 4));
        for &byte in bytes {
            if let Some(ch) = state.sanitizer.push(byte) {
                if state.sanitizer.historical {
                    let mut bytes = [0; 4];
                    state.capture_hidden(ch.encode_utf8(&mut bytes));
                    continue;
                }
                chunk.push(ch);
                if chunk.len() >= SANITIZE_CHUNK_BYTES {
                    state.capture(&chunk);
                    chunk.zeroize();
                }
            }
        }
        state.capture(&chunk);
        if state.sanitizer.invalid {
            state.tail.gap |= state.session.is_some();
            state.sanitizer.invalid = false;
        }
    }

    /// Consume the bounded unread text. A suffix that could still become a
    /// secret is withheld across reads; reading between SSH chunks cannot leak
    /// that prefix. An unfinished UTF-8 character is likewise retained.
    pub fn read(&self) -> Observation {
        let mut state = self.lock();
        Observation {
            text: std::mem::take(&mut state.tail.text),
            truncated: std::mem::take(&mut state.tail.truncated) || state.blocked,
            gap: std::mem::take(&mut state.tail.gap) || state.blocked,
            untrusted: true,
        }
    }

    /// Stop capture and discard all text, parser state, and known secrets.
    pub fn reset(&self) {
        *self.lock() = State::default();
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => {
                // A panic must not turn a partially updated filter into an
                // unredacted observation source. The poisoned lock stays shut.
                let mut state = poisoned.into_inner();
                *state = State {
                    blocked: true,
                    ..State::default()
                };
                state
            }
        }
    }
}

#[derive(Default)]
struct State {
    session: Option<SessionRef>,
    stream_session: Option<SessionRef>,
    blocked: bool,
    registered: Vec<Zeroizing<String>>,
    run_secrets: Vec<Zeroizing<String>>,
    sanitizer: Sanitizer,
    redactor: Redactor,
    tail: Tail,
}
fn valid_secret_budget<'a>(secrets: impl Iterator<Item = &'a str>) -> bool {
    let mut total = 0usize;
    let mut count = 0usize;
    for secret in secrets.filter(|s| !s.is_empty()) {
        total = total.saturating_add(secret.len());
        count += 1;
        if secret.len() > MAX_SECRET_BYTES || total > MAX_TOTAL_SECRET_BYTES || count > MAX_SECRETS
        {
            return false;
        }
    }
    true
}

impl State {
    fn rebuild_redactor(&mut self) {
        self.redactor = Redactor::default();
        for secret in self.registered.iter().chain(&self.run_secrets) {
            // Sanitize secrets by exactly the same rules as visible output.
            let mut sanitizer = Sanitizer::default();
            let mut normalized = Zeroizing::new(String::new());
            for &byte in secret.as_bytes() {
                if let Some(ch) = sanitizer.push(byte) {
                    normalized.push(ch);
                }
            }
            if !normalized.is_empty()
                && !self
                    .redactor
                    .patterns
                    .iter()
                    .any(|p| p.text.as_str() == normalized.as_str())
            {
                self.redactor.patterns.push(Pattern::new(normalized));
            }
        }
    }

    fn capture(&mut self, chunk: &str) {
        self.redactor.push(chunk, &mut self.tail);
        if self.session.is_none() {
            self.tail = Tail::default();
            self.redactor.excluded.fill(true);
        }
    }

    fn capture_hidden(&mut self, text: &str) {
        self.redactor.push(text, &mut Tail::default());
        self.redactor.excluded.fill(true);
    }
}

struct Tail {
    limit: usize,
    text: String,
    truncated: bool,
    gap: bool,
}
impl Default for Tail {
    fn default() -> Self {
        Self {
            limit: MAX_OBSERVATION_BYTES,
            text: String::new(),
            truncated: false,
            gap: false,
        }
    }
}
impl Tail {
    fn push(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if text.len() > self.limit {
            let mut start = text.len() - self.limit;
            while !text.is_char_boundary(start) {
                start += 1;
            }
            self.text.zeroize();
            self.text.push_str(&text[start..]);
            self.truncated = true;
            self.gap = true;
            return;
        }
        let excess = (self.text.len() + text.len()).saturating_sub(self.limit);
        if excess > 0 {
            let mut start = excess;
            while !self.text.is_char_boundary(start) {
                start += 1;
            }
            self.text.drain(..start);
            self.truncated = true;
            self.gap = true;
        }
        self.text.push_str(text);
    }
}
impl Drop for Tail {
    fn drop(&mut self) {
        self.text.zeroize();
    }
}

struct Pattern {
    text: Zeroizing<String>,
    failure: Vec<usize>,
    matched: usize,
}
impl Pattern {
    fn new(text: Zeroizing<String>) -> Self {
        let bytes = text.as_bytes();
        let mut failure = vec![0; bytes.len()];
        let mut matched = 0;
        for index in 1..bytes.len() {
            while matched > 0 && bytes[index] != bytes[matched] {
                matched = failure[matched - 1];
            }
            if bytes[index] == bytes[matched] {
                matched += 1;
            }
            failure[index] = matched;
        }
        Self {
            text,
            failure,
            matched: 0,
        }
    }
}

#[derive(Default)]
struct Redactor {
    patterns: Vec<Pattern>,
    pending: Zeroizing<String>,
    masked: Vec<bool>,
    excluded: Vec<bool>,
}
impl Redactor {
    fn push(&mut self, text: &str, tail: &mut Tail) {
        if text.is_empty() {
            return;
        }
        if self.patterns.is_empty() {
            tail.push(text);
            return;
        }
        let old_len = self.pending.len();
        self.pending.push_str(text);
        self.masked.resize(self.pending.len(), false);
        self.excluded.resize(self.pending.len(), false);
        // Streaming KMP state handles overlapping matches and makes work linear
        // in input bytes per bounded secret, even for adversarial repeated text.
        for index in old_len..self.pending.len() {
            let byte = self.pending.as_bytes()[index];
            for pattern in &mut self.patterns {
                let secret = pattern.text.as_bytes();
                while pattern.matched > 0 && secret[pattern.matched] != byte {
                    pattern.matched = pattern.failure[pattern.matched - 1];
                }
                if secret[pattern.matched] == byte {
                    pattern.matched += 1;
                }
                if pattern.matched == secret.len() {
                    self.masked[index + 1 - secret.len()..=index].fill(true);
                    pattern.matched = pattern.failure[secret.len() - 1];
                }
            }
        }
        let held = self.patterns.iter().map(|p| p.matched).max().unwrap_or(0);
        let ready = self.pending.len() - held;
        // A complete UTF-8 suffix matching a valid secret prefix starts and
        // ends on character boundaries. Masks also cover whole characters.
        debug_assert!(self.pending.is_char_boundary(ready));
        let mut start = 0;
        while start < ready {
            let masked = self.masked[start];
            let excluded = self.excluded[start];
            let mut end = start + 1;
            while end < ready && self.masked[end] == masked && self.excluded[end] == excluded {
                end += 1;
            }
            if excluded {
                // Matching context from before begin is never uploaded.
            } else if masked {
                tail.push(REDACTED);
            } else {
                tail.push(&self.pending[start..end]);
            }
            start = end;
        }
        // Do not leave consumed plaintext in spare String capacity.
        self.pending = Zeroizing::new(self.pending[ready..].to_owned());
        self.masked.drain(..ready);
        self.excluded.drain(..ready);
        debug_assert!(self.pending.len() < MAX_SECRET_BYTES);
    }
}

#[derive(Clone, Copy, Default)]
enum EscapeState {
    #[default]
    Ground,
    Escape,
    Intermediate,
    Csi,
    String {
        osc: bool,
        escape: bool,
    },
}

/// Constant-size decoder/parser: escape payloads are never accumulated. A
/// malformed or unclosed string remains suppressed until its actual terminator.
#[derive(Default)]
struct Sanitizer {
    state: EscapeState,
    preserve_whitespace: bool,
    utf8: [u8; 4],
    utf8_len: usize,
    utf8_expected: usize,
    epoch_pending: bool,
    historical: bool,
    invalid: bool,
}
impl Sanitizer {
    fn push(&mut self, byte: u8) -> Option<char> {
        self.historical = false;
        if self.utf8_len > 0 {
            if byte & 0xc0 == 0x80 {
                self.utf8[self.utf8_len] = byte;
                self.utf8_len += 1;
                if self.utf8_len < self.utf8_expected {
                    return None;
                }
                let decoded = std::str::from_utf8(&self.utf8[..self.utf8_len])
                    .ok()
                    .and_then(|text| text.chars().next());
                self.utf8_len = 0;
                self.historical = std::mem::take(&mut self.epoch_pending);
                if let Some(ch) = decoded {
                    return self.character(ch);
                }
                self.invalid = true;
                return None;
            }
            self.utf8_len = 0;
            self.epoch_pending = false;
            self.invalid = true;
            // Reprocess the interrupting byte, including ESC and ASCII text.
        }
        match byte {
            0x00..=0x7f => self.character(char::from(byte)),
            // Accept the terminal's single-byte C1 control form as well as
            // UTF-8-encoded C1, but never confuse a continuation with a control.
            0x80..=0x9f => self.character(char::from(byte)),
            0xc2..=0xf4 => {
                self.utf8[0] = byte;
                self.utf8_len = 1;
                self.utf8_expected = match byte {
                    0xc2..=0xdf => 2,
                    0xe0..=0xef => 3,
                    _ => 4,
                };
                None
            }
            _ => {
                self.invalid = true;
                None
            }
        }
    }

    fn character(&mut self, ch: char) -> Option<char> {
        if matches!(ch, '\u{18}' | '\u{1a}') {
            self.state = EscapeState::Ground;
            return None;
        }
        // C1 introducers are valid from CSI/ESC states too. Only an existing
        // control string treats them as payload. Ignoring an OSC introducer
        // inside an incomplete CSI would otherwise expose its title/URL bytes.
        if !matches!(self.state, EscapeState::String { .. }) {
            let introduced = match ch {
                '\u{1b}' => Some(EscapeState::Escape),
                '\u{9b}' => Some(EscapeState::Csi),
                '\u{9d}' => Some(EscapeState::String {
                    osc: true,
                    escape: false,
                }),
                '\u{90}' | '\u{98}' | '\u{9e}' | '\u{9f}' => Some(EscapeState::String {
                    osc: false,
                    escape: false,
                }),
                '\u{9c}' => Some(EscapeState::Ground),
                _ => None,
            };
            if let Some(state) = introduced {
                self.state = state;
                return None;
            }
        }
        match self.state {
            EscapeState::String { osc, escape } => {
                if ch == '\u{9c}' || (osc && ch == '\u{7}') || (escape && ch == '\\') {
                    self.state = EscapeState::Ground;
                } else {
                    self.state = EscapeState::String {
                        osc,
                        escape: ch == '\u{1b}',
                    };
                }
            }
            EscapeState::Csi => {
                if ch == '\u{1b}' {
                    self.state = EscapeState::Escape;
                } else if ('\u{40}'..='\u{7e}').contains(&ch) {
                    self.state = EscapeState::Ground;
                }
            }
            EscapeState::Escape => {
                self.state = match ch {
                    '[' => EscapeState::Csi,
                    ']' => EscapeState::String {
                        osc: true,
                        escape: false,
                    },
                    'P' | 'X' | '^' | '_' => EscapeState::String {
                        osc: false,
                        escape: false,
                    },
                    '\u{20}'..='\u{2f}' => EscapeState::Intermediate,
                    '\u{1b}' => EscapeState::Escape,
                    ch if ch.is_control() => EscapeState::Escape,
                    _ => EscapeState::Ground,
                };
            }
            EscapeState::Intermediate => {
                if ch == '\u{1b}' {
                    self.state = EscapeState::Escape;
                } else if ('\u{30}'..='\u{7e}').contains(&ch) {
                    self.state = EscapeState::Ground;
                }
            }
            EscapeState::Ground => match ch {
                '\n' | '\r' | '\t' if self.preserve_whitespace => return Some(ch),
                '\u{1b}' => self.state = EscapeState::Escape,
                '\u{9b}' => self.state = EscapeState::Csi,
                '\u{9d}' => {
                    self.state = EscapeState::String {
                        osc: true,
                        escape: false,
                    }
                }
                '\u{90}' | '\u{98}' | '\u{9e}' | '\u{9f}' => {
                    self.state = EscapeState::String {
                        osc: false,
                        escape: false,
                    }
                }
                // Also remove bidi overrides/isolates and invisible text
                // separators; preserve combining marks and emoji joiners.
                ch if !ch.is_control()
                    && !matches!(ch,
                        '\u{061c}' | '\u{200b}' | '\u{200e}' | '\u{200f}' |
                        '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}'
                    ) =>
                {
                    return Some(ch)
                }
                _ => {}
            },
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(generation: &str) -> SessionRef {
        SessionRef {
            session_id: "test-session".into(),
            generation: generation.into(),
        }
    }
    fn started(secrets: &[&str]) -> (ObservationBuffer, SessionRef) {
        let buffer = ObservationBuffer::new();
        let session = session("1");
        buffer.begin(
            session.clone(),
            secrets.iter().map(|s| (*s).into()).collect(),
        );
        (buffer, session)
    }

    #[test]
    fn captures_only_new_matching_session_output_and_reads_incrementally() {
        let buffer = ObservationBuffer::new();
        let first = session("1");
        buffer.push(&first, b"historical");
        buffer.begin(first.clone(), vec![]);
        buffer.clone().push(&first, b"current");
        buffer.push(&session("2"), b"wrong generation");
        let mut other = first.clone();
        other.session_id = "other".into();
        buffer.push(&other, b"wrong session");
        let read = buffer.read();
        assert_eq!(read.text, "current");
        assert!(read.untrusted);
        assert!(!read.gap || read.truncated);
        assert!(buffer.read().text.is_empty());
        buffer.push(&first, b"unread");
        buffer.begin(session("2"), vec![]);
        assert!(buffer.read().text.is_empty());
        buffer.reset();
        buffer.push(&session("2"), b"after reset");
        assert!(buffer.read().text.is_empty());
    }

    #[test]
    fn begin_mid_escape_or_secret_keeps_filter_state_without_uploading_history() {
        let buffer = ObservationBuffer::new();
        let session = session("1");
        buffer.add_known_secret("secret-value").unwrap();
        buffer.push(&session, b"historical text\x1b]hidden");
        buffer.begin(session.clone(), vec![]);
        buffer.push(&session, b"title\x1b\\visible!");
        assert_eq!(buffer.read().text, "visible!");

        buffer.push(&session, b"old text secret-");
        buffer.begin(session.clone(), vec![]);
        buffer.push(&session, b"value!");
        assert_eq!(buffer.read().text, "[REDACTED]!");

        buffer.push(&session, b"secret-");
        buffer.begin(session.clone(), vec![]);
        buffer.push(&session, b"other!");
        assert_eq!(buffer.read().text, "other!");
    }

    #[test]
    fn utf8_character_started_before_begin_is_not_uploaded_but_keeps_matching_context() {
        let buffer = ObservationBuffer::new();
        let session = session("1");
        buffer.add_known_secret("秘密").unwrap();
        let bytes = "秘密".as_bytes();
        buffer.push(&session, &bytes[..2]);
        buffer.begin(session.clone(), vec![]);
        buffer.push(&session, &bytes[2..]);
        assert_eq!(buffer.read().text, REDACTED);
        buffer.push(&session, &[0xe4]);
        buffer.begin(session.clone(), vec![]);
        buffer.push(&session, &[0xb8, 0xad, b'!']);
        assert_eq!(buffer.read().text, "!");
    }

    #[test]
    fn strips_split_sequences_all_c0_c1_and_osc_hyperlink_payloads() {
        let (buffer, session) = started(&[]);
        let bytes = b"A\x1b[31mB\x1b[0m\x1b]8;;https://hidden.invalid\x1b\\link\x1b]8;;\x07\x1bPsecret payload\x1b\\\x1b_secret apc\x1b\\\x1b^secret pm\x1b\\\x1bXsecret sos\x1b\\\x1b(B\x00\x01\x08\t\r\n\x7f\x9b32mC\x9dhidden\x9cD";
        for byte in bytes {
            buffer.push(&session, &[*byte]);
        }
        assert_eq!(buffer.read().text, "ABlinkCD");
        buffer.push(&session, "\u{9b}1mE\u{9d}hidden\u{9c}F\u{85}".as_bytes());
        assert_eq!(buffer.read().text, "EF");
    }

    #[test]
    fn c1_string_introducers_interrupt_incomplete_escape_sequences() {
        let (buffer, session) = started(&[]);
        for bytes in [
            b"a\x1b[1\x9dhidden title\x9cb".as_slice(),
            b"a\x1b\x90hidden dcs\x1b\\b".as_slice(),
            b"a\x1b(\x9dhidden title\x07b".as_slice(),
        ] {
            for byte in bytes {
                buffer.push(&session, &[*byte]);
            }
            assert_eq!(buffer.read().text, "ab");
        }
        buffer.push(&session, "a\u{202e}b\u{2066}c\u{200b}d\u{feff}".as_bytes());
        assert_eq!(buffer.read().text, "abcd");
    }

    #[test]
    fn escape_string_payload_is_never_buffered_even_when_unterminated() {
        let (buffer, session) = started(&[]);
        buffer.push(&session, b"visible\x1b]52;c;");
        buffer.push(&session, &vec![b'x'; 1024 * 1024]);
        buffer.push(&session, b"\x1b[31mstill hidden");
        assert_eq!(buffer.read().text, "visible");
        assert!(buffer.lock().tail.text.is_empty());
        buffer.push(&session, b"\x1b\\end");
        assert_eq!(buffer.read().text, "end");
    }

    #[test]
    fn split_unicode_is_preserved_without_treating_continuations_as_controls() {
        let (buffer, session) = started(&[]);
        let expected = "中文 café 🦀 e\u{301} 👩‍💻";
        let mut actual = String::new();
        for byte in expected.as_bytes() {
            buffer.push(&session, &[*byte]);
            actual.push_str(&buffer.read().text);
        }
        assert_eq!(actual, expected);
    }

    #[test]
    fn malformed_utf8_is_omitted_and_reports_a_consumable_gap() {
        let (buffer, session) = started(&[]);
        buffer.push(&session, b"a\xff\xf0\x80\x80\x80b\xe2");
        buffer.push(&session, b"c");
        let result = buffer.read();
        assert_eq!(result.text, "abc");
        assert!(result.gap);
        assert!(!result.truncated);
        assert!(!buffer.read().gap);
    }

    #[test]
    fn known_secrets_never_leak_between_chunks_or_reads() {
        let (buffer, session) = started(&["super-secret", "秘密", "abc", "bcdef"]);
        buffer.push(&session, b"prefix: super-");
        assert_eq!(buffer.read().text, "prefix: ");
        buffer.push(&session, b"secret; ");
        assert_eq!(buffer.read().text, "[REDACTED]; ");
        for byte in "秘密".as_bytes() {
            buffer.push(&session, &[*byte]);
            assert!(!buffer.read().text.contains('秘'));
        }
        buffer.push(&session, b"ab");
        assert!(buffer.read().text.is_empty());
        buffer.push(&session, b"c");
        assert_eq!(buffer.read().text, REDACTED);
        buffer.push(&session, b"def!");
        assert_eq!(buffer.read().text, "[REDACTED]!");
    }

    #[test]
    fn overlapping_secret_redaction_is_safe_at_every_two_chunk_boundary() {
        let source = b"before abcdef after!";
        for cut in 0..=source.len() {
            let (buffer, session) = started(&["abc", "bcdef"]);
            buffer.push(&session, &source[..cut]);
            let mut observed = buffer.read().text;
            buffer.push(&session, &source[cut..]);
            observed.push_str(&buffer.read().text);
            assert_eq!(
                observed.replace(REDACTED, ""),
                "before  after!",
                "cut {cut}"
            );
        }
    }

    #[test]
    fn incomplete_secret_prefix_is_released_only_after_disambiguation() {
        let (buffer, session) = started(&["token-value"]);
        buffer.push(&session, b"ok token-");
        assert_eq!(buffer.read().text, "ok ");
        assert!(buffer.read().text.is_empty());
        buffer.push(&session, b"other!");
        assert_eq!(buffer.read().text, "token-other!");
    }

    #[test]
    fn matches_overlap_and_normalized_secrets_across_control_sequences() {
        let (buffer, session) = started(&["ababa", "sec\nret", "token"]);
        for byte in b"abababa; sec\x1b[31mret; to\x00ken!" {
            buffer.push(&session, &[*byte]);
        }
        let result = buffer.read();
        assert!(!result.text.contains('a'));
        assert!(!result.text.contains('b'));
        assert!(!result.text.contains("secret"));
        assert!(!result.text.contains("token"));
        assert!(result.text.contains(REDACTED));
    }

    #[test]
    fn bounds_tail_with_utf8_safe_truncation_and_explicit_gap() {
        let (buffer, session) = started(&[]);
        buffer.push(&session, "中".repeat(MAX_OBSERVATION_BYTES).as_bytes());
        let result = buffer.read();
        assert!(result.text.len() <= MAX_OBSERVATION_BYTES);
        assert!(result.text.chars().all(|ch| ch == '中'));
        assert!(result.truncated && result.gap && result.untrusted);
        let next = buffer.read();
        assert!(next.text.is_empty());
        assert!(!next.truncated && !next.gap);
    }

    #[test]
    fn truncation_never_releases_a_secret_suffix_at_the_left_cut() {
        let secret = "x".repeat(MAX_SECRET_BYTES);
        let (buffer, session) = started(&[&secret]);
        buffer.push(&session, &vec![b'a'; MAX_OBSERVATION_BYTES * 2]);
        buffer.push(&session, &secret.as_bytes()[..secret.len() - 1]);
        let before = buffer.read();
        assert!(!before.text.contains('x'));
        assert!(before.truncated && before.gap);
        buffer.push(&session, b"x!");
        let after = buffer.read();
        assert!(!after.text.contains('x'));
        assert!(after.text.ends_with('!'));
        assert!(buffer.lock().redactor.pending.len() < MAX_SECRET_BYTES);
    }

    #[test]
    fn oversized_secret_sets_fail_closed_until_a_new_epoch() {
        for secrets in [
            vec!["x".repeat(MAX_SECRET_BYTES + 1)],
            vec!["x".to_owned(); MAX_SECRETS + 1],
            vec!["x".repeat(MAX_SECRET_BYTES); 5],
        ] {
            let buffer = ObservationBuffer::new();
            let session = session("1");
            buffer.begin(session.clone(), secrets);
            buffer.push(&session, b"do not expose anything");
            for _ in 0..2 {
                let result = buffer.read();
                assert!(result.text.is_empty());
                assert!(result.gap && result.truncated);
            }
            buffer.begin(session.clone(), vec![]);
            buffer.push(&session, b"new epoch");
            assert_eq!(buffer.read().text, "new epoch");
        }
    }

    #[test]
    fn reset_discards_incomplete_utf8_escape_and_redaction_state() {
        let (buffer, session) = started(&["secret"]);
        buffer.push(&session, b"sec\x1b]hidden\xe2");
        buffer.reset();
        buffer.begin(session.clone(), vec![]);
        buffer.push(&session, b"ret");
        assert_eq!(buffer.read().text, "ret");
    }

    #[test]
    fn registered_secrets_survive_begin_but_reset_clears_them() {
        let buffer = ObservationBuffer::new();
        let session = session("1");
        buffer.add_known_secret("ssh-password").unwrap();
        buffer.add_known_secret("").unwrap();
        buffer.add_known_secret("ssh-password").unwrap();
        assert_eq!(buffer.redact_text("ssh-password!"), "[REDACTED]!");
        for _ in 0..2 {
            buffer.begin(session.clone(), vec!["model-key".into()]);
            buffer.push(&session, b"ssh-password model-key!");
            assert_eq!(buffer.read().text, "[REDACTED] [REDACTED]!");
        }
        buffer.reset();
        buffer.begin(session.clone(), vec![]);
        buffer.push(&session, b"ssh-password model-key!");
        assert_eq!(buffer.read().text, "ssh-password model-key!");
    }

    #[test]
    fn active_registration_discards_old_unread_data_and_preserves_escape_state() {
        let (buffer, session) = started(&["old-value"]);
        buffer.push(&session, b"new-value unread\x1b]hidden");
        buffer.add_known_secret("new-value").unwrap();
        buffer.push(&session, b"payload\x07old-value new-value!");
        let observation = buffer.read();
        assert_eq!(observation.text, "[REDACTED] [REDACTED]!");
        assert!(observation.gap);
    }

    #[test]
    fn registration_limits_fail_closed_and_do_not_echo_the_value() {
        let (buffer, session) = started(&[]);
        let error = buffer
            .add_known_secret(&"s".repeat(MAX_SECRET_BYTES + 1))
            .unwrap_err();
        assert_eq!(error.code, "OBSERVATION_SECRET_LIMIT");
        buffer.push(&session, b"blocked");
        assert!(buffer.read().text.is_empty());
        assert_eq!(buffer.redact_text("blocked"), REDACTED);
        buffer.begin(session.clone(), vec![]);
        buffer.push(&session, b"allowed");
        assert_eq!(buffer.read().text, "allowed");
    }

    #[test]
    fn redact_text_is_bounded_prefix_safe_and_does_not_consume_the_pty_cursor() {
        let (buffer, session) = started(&["secret-value"]);
        buffer.push(&session, b"unread!");
        assert_eq!(buffer.redact_text("safe!"), "safe!");
        assert_eq!(buffer.redact_text("echo secret-"), "echo [REDACTED]");
        assert_eq!(buffer.redact_text("echo secret-value!"), "echo [REDACTED]!");
        assert_eq!(buffer.redact_text("a\x1b]title\x07b"), "ab");
        assert_eq!(buffer.read().text, "unread!");
        let text = "x".repeat(MAX_REDACT_TEXT_BYTES + 1);
        assert_eq!(buffer.redact_text(&text), REDACTED);
        let text = "x".repeat(MAX_REDACT_TEXT_BYTES);
        assert_eq!(buffer.redact_text(&text), text);
    }

    #[test]
    fn observation_serialization_has_only_safe_text_and_contract_flags() {
        let (buffer, session) = started(&["private-value"]);
        buffer.push(&session, b"private-value!");
        let value = serde_json::to_value(buffer.read()).unwrap();
        assert_eq!(value["text"], "[REDACTED]!");
        assert_eq!(value["untrusted"], true);
        assert_eq!(value.as_object().unwrap().len(), 4);
    }
    #[test]
    fn detached_redactor_survives_session_reset_without_retaining_output() {
        let original = ObservationBuffer::new();
        original.add_known_secret("old-ssh-password").unwrap();
        original.begin(
            SessionRef {
                session_id: "old".into(),
                generation: "1".into(),
            },
            vec!["old-model-key".into()],
        );
        let detached = original.detached_redactor();
        original.reset();
        original.add_known_secret("new-secret").unwrap();
        assert_eq!(
            detached.redact_text("old-ssh-password old-model-key"),
            "[REDACTED] [REDACTED]"
        );
        assert!(detached.read().text.is_empty());
    }
    #[test]
    fn prose_preserves_linebreaks_and_indentation_without_leaking_secrets() {
        let observer = ObservationBuffer::new();
        observer.add_known_secret("fixture-key").unwrap();
        observer.add_known_secret("line\nbreak").unwrap();
        assert_eq!(
            observer.redact_prose("first\n\tsecond\r\nfixture-key line\nbreak"),
            "first\n\tsecond\r\n[REDACTED] [REDACTED]"
        );
        assert_eq!(
            observer.redact_prose("safe\n\x1b]52;c;hidden\nsecret\x07last"),
            "safe\nlast"
        );
        assert_eq!(
            observer.redact_prose("prefix fixture-"),
            "prefix [REDACTED]"
        );
    }
}
