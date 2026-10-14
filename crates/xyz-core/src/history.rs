//! Host-only optional archive coordinator. A bounded worker owns SQLite; capture
//! queues contain only already-redacted text. Enabling is an explicit choice for
//! this process, never inferred from existing files or a model response.
use crate::{
    observation::ObservationBuffer,
    persistence::{HistoryError, HistoryPreference, HistoryStore, RedactedChat, StoredChat},
    ssh::ApiError,
    tasks::{ChatSnapshot, MAX_CHATS},
};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, SyncSender},
        Arc, Mutex,
    },
};

const QUEUE_CAPACITY: usize = 32;
#[derive(Clone, Debug, Serialize)]
pub struct HistoryStatus {
    pub enabled: bool,
    pub manageable: bool,
    pub available: bool,
    pub plaintext: bool,
    pub revision: String,
    pub error: Option<ApiError>,
}
#[derive(Clone, Debug, Serialize)]
pub struct HistoryEntry {
    pub chat_id: String,
    pub task_id: String,
    pub title: String,
    pub message_count: usize,
    pub revision: String,
    pub updated_ms: i64,
    pub listed: bool,
}
pub type HistoryEventSink = Arc<dyn Fn(HistoryStatus) + Send + Sync>;
#[derive(Clone)]
pub struct HistoryService {
    sender: SyncSender<Command>,
    shared: Arc<Shared>,
    redactor: ObservationBuffer,
    gate: Arc<Mutex<()>>,
}
struct Shared {
    enabled: AtomicBool,
    manageable: AtomicBool,
    epoch: AtomicU64,
    status: Mutex<HistoryStatus>,
    events: HistoryEventSink,
}
struct Capture {
    chat_id: String,
    live_revision: u64,
    prepared: RedactedChat,
    epoch: u64,
}
struct Known {
    revision: i64,
    live_revision: u64,
}
type Reply<T> = mpsc::Sender<Result<T, ApiError>>;
enum Command {
    Capture(Capture),
    Enable {
        enabled: bool,
        captures: Vec<Capture>,
        epoch: u64,
        reply: Reply<HistoryStatus>,
    },
    List(Reply<Vec<HistoryEntry>>),
    Restore {
        chat_id: String,
        revision: i64,
        reply: Reply<ChatSnapshot>,
    },
    Forget {
        chat_id: String,
        revision: i64,
        reply: Reply<HistoryStatus>,
    },
    Flush(Reply<()>),
}
struct Worker {
    path: Result<PathBuf, ApiError>,
    store: Option<HistoryStore>,
    known: BTreeMap<String, Known>,
    forgotten: BTreeSet<String>,
    shared: Arc<Shared>,
    redactor: ObservationBuffer,
}

impl Shared {
    fn publish(&self, enabled: bool, error: Option<ApiError>) -> HistoryStatus {
        self.enabled.store(enabled, Ordering::SeqCst);
        let status = {
            let mut status = self.status.lock().unwrap_or_else(|e| e.into_inner());
            status.enabled = enabled;
            status.manageable = self.manageable.load(Ordering::SeqCst);
            status.error = error;
            let revision = status
                .revision
                .parse::<u64>()
                .unwrap_or(0)
                .saturating_add(1);
            status.revision = revision.to_string();
            status.clone()
        };
        (self.events)(status.clone());
        status
    }
    fn fail(&self, error: ApiError) {
        self.publish(false, Some(error));
    }
    fn status(&self) -> HistoryStatus {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}
impl HistoryService {
    /// Does not create or read any history files. The supplied shared redactor
    /// must register known SSH/task secrets before exposing their text anywhere.
    pub fn new(
        path: Result<PathBuf, ApiError>,
        redactor: ObservationBuffer,
        events: HistoryEventSink,
    ) -> Self {
        let status = HistoryStatus {
            enabled: false,
            manageable: false,
            available: path.is_ok(),
            plaintext: true,
            revision: "0".into(),
            error: path.as_ref().err().cloned(),
        };
        let shared = Arc::new(Shared {
            enabled: AtomicBool::new(false),
            manageable: AtomicBool::new(false),
            epoch: AtomicU64::new(0),
            status: Mutex::new(status),
            events,
        });
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        let worker_shared = shared.clone();
        let worker_redactor = redactor.clone();
        let start = std::thread::Builder::new()
            .name("xyz-history".into())
            .spawn(move || {
                let mut worker = Worker {
                    path,
                    store: None,
                    known: BTreeMap::new(),
                    forgotten: BTreeSet::new(),
                    shared: worker_shared,
                    redactor: worker_redactor,
                };
                while let Ok(command) = receiver.recv() {
                    worker.handle(command);
                }
            });
        if start.is_err() {
            shared.fail(unavailable());
        }
        Self {
            sender,
            shared,
            redactor,
            gate: Arc::new(Mutex::new(())),
        }
    }
    pub fn status(&self) -> HistoryStatus {
        self.shared.status()
    }
    pub(crate) fn redactor(&self) -> &ObservationBuffer {
        &self.redactor
    }
    pub(crate) fn fail(&self, error: ApiError) {
        self.shared.fail(error);
    }
    fn prepare(&self, chat: &ChatSnapshot, epoch: u64) -> Result<Capture, ApiError> {
        let live_revision = chat
            .revision
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or_else(|| mapped(HistoryError::InvalidRecord))?;
        let prepared = RedactedChat::from_snapshot(chat, |text| self.redactor.redact_prose(text))
            .map_err(mapped)?;
        Ok(Capture {
            chat_id: chat.chat_id.clone(),
            live_revision,
            prepared,
            epoch,
        })
    }
    /// Never blocks on disk. Queue overflow fails visibly and stops saving;
    /// accepted earlier snapshots are not silently treated as the latest state.
    pub(crate) fn capture(&self, chat: &ChatSnapshot) {
        let _gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
        if !self.shared.enabled.load(Ordering::SeqCst) {
            return;
        }
        let capture = match self.prepare(chat, self.shared.epoch.load(Ordering::SeqCst)) {
            Ok(c) => c,
            Err(e) => {
                self.shared.fail(e);
                return;
            }
        };
        if self.sender.try_send(Command::Capture(capture)).is_err() {
            self.shared.fail(ApiError::new("HISTORY_QUEUE_FULL","History saving stopped because its bounded queue is unavailable. Current chat remains in memory; review saved history before enabling again."));
        }
    }
    /// Blocking host operation. Enabling covers the supplied current chats and
    /// subsequent changes. Disabling waits for the worker and never erases text.
    pub fn set_enabled(
        &self,
        enabled: bool,
        chats: &[ChatSnapshot],
    ) -> Result<HistoryStatus, ApiError> {
        if chats.len() > MAX_CHATS {
            return Err(mapped(HistoryError::LimitReached));
        }
        let (reply, receive) = mpsc::channel();
        {
            let _gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
            let epoch = self.shared.epoch.fetch_add(1, Ordering::SeqCst) + 1;
            let captures = if enabled {
                chats
                    .iter()
                    .map(|chat| self.prepare(chat, epoch))
                    .collect::<Result<Vec<_>, _>>()?
            } else {
                Vec::new()
            };
            // Epoch invalidates every queued snapshot from an earlier choice.
            self.shared.enabled.store(enabled, Ordering::SeqCst);
            if !enabled {
                self.shared.manageable.store(false, Ordering::SeqCst);
            }
            self.sender
                .send(Command::Enable {
                    enabled,
                    captures,
                    epoch,
                    reply,
                })
                .map_err(|_| unavailable())?;
        }
        receive.recv().map_err(|_| unavailable())?
    }
    pub fn list(&self) -> Result<Vec<HistoryEntry>, ApiError> {
        let (reply, receive) = mpsc::channel();
        self.sender
            .send(Command::List(reply))
            .map_err(|_| unavailable())?;
        receive.recv().map_err(|_| unavailable())?
    }
    /// TaskManager must validate free capacity/absence of this chat while
    /// holding its chat lock before this read, then install only the inert DTO.
    pub(crate) fn restore(&self, chat_id: &str, revision: &str) -> Result<ChatSnapshot, ApiError> {
        let revision = revision_number(revision)?;
        let (reply, receive) = mpsc::channel();
        self.sender
            .send(Command::Restore {
                chat_id: chat_id.into(),
                revision,
                reply,
            })
            .map_err(|_| unavailable())?;
        receive.recv().map_err(|_| unavailable())?
    }
    pub fn forget(&self, chat_id: &str, revision: &str) -> Result<HistoryStatus, ApiError> {
        let revision = revision_number(revision)?;
        let (reply, receive) = mpsc::channel();
        self.sender
            .send(Command::Forget {
                chat_id: chat_id.into(),
                revision,
                reply,
            })
            .map_err(|_| unavailable())?;
        receive.recv().map_err(|_| unavailable())?
    }
    pub fn flush(&self) -> Result<(), ApiError> {
        let (reply, receive) = mpsc::channel();
        self.sender
            .send(Command::Flush(reply))
            .map_err(|_| unavailable())?;
        receive.recv().map_err(|_| unavailable())?
    }
}
impl Worker {
    fn handle(&mut self, command: Command) {
        match command {
            Command::Capture(capture) => {
                if self.shared.enabled.load(Ordering::SeqCst)
                    && capture.epoch == self.shared.epoch.load(Ordering::SeqCst)
                {
                    if let Err(error) = self.save(capture) {
                        self.shared.fail(error);
                    }
                }
            }
            Command::Enable {
                enabled,
                captures,
                epoch,
                reply,
            } => {
                let result = self.enable(enabled, captures, epoch);
                if let Err(error) = &result {
                    self.shared.fail(error.clone());
                }
                let _ = reply.send(result);
            }
            Command::List(reply) => {
                let result = self
                    .active()
                    .and_then(|store| store.list(true).map_err(mapped))
                    .map(|rows| {
                        rows.into_iter()
                            .map(|row| entry(row, &self.redactor))
                            .collect()
                    });
                let _ = reply.send(result);
            }
            Command::Restore {
                chat_id,
                revision,
                reply,
            } => {
                let result = (|| {
                    let row = self.active()?.read(&chat_id).map_err(mapped)?;
                    if row.revision != revision {
                        return Err(mapped(HistoryError::Conflict));
                    }
                    if !row.listed {
                        return Err(mapped(HistoryError::NotFound));
                    }
                    let mut chat = row.to_snapshot();
                    chat.title = self.redactor.redact_prose(&chat.title);
                    if let Some(source) = &mut chat.source {
                        source.host = self.redactor.redact_prose(&source.host);
                        source.username = self.redactor.redact_prose(&source.username);
                    }
                    for message in &mut chat.messages {
                        message.text = self.redactor.redact_prose(&message.text);
                    }
                    self.known.insert(
                        chat_id.clone(),
                        Known {
                            revision: row.revision,
                            live_revision: row.revision as u64,
                        },
                    );
                    self.forgotten.remove(&chat_id);
                    Ok(chat)
                })();
                let _ = reply.send(result);
            }
            Command::Forget {
                chat_id,
                revision,
                reply,
            } => {
                let result = (|| {
                    if !self.forgotten.contains(&chat_id) && self.forgotten.len() >= MAX_CHATS * 4 {
                        return Err(mapped(HistoryError::LimitReached));
                    }
                    self.active()?.forget(&chat_id, revision).map_err(mapped)?;
                    self.known.remove(&chat_id);
                    self.forgotten.insert(chat_id);
                    Ok(self.shared.status())
                })();
                let _ = reply.send(result);
            }
            Command::Flush(reply) => {
                let status = self.shared.status();
                let _ = reply.send(status.error.map_or(Ok(()), Err));
            }
        }
    }
    fn active(&mut self) -> Result<&mut HistoryStore, ApiError> {
        if !self.shared.manageable.load(Ordering::SeqCst) {
            return Err(mapped(HistoryError::Disabled));
        }
        self.store
            .as_mut()
            .ok_or_else(|| mapped(HistoryError::Disabled))
    }
    fn enable(
        &mut self,
        enabled: bool,
        captures: Vec<Capture>,
        epoch: u64,
    ) -> Result<HistoryStatus, ApiError> {
        if epoch != self.shared.epoch.load(Ordering::SeqCst) {
            return Err(mapped(HistoryError::Conflict));
        }
        if !enabled {
            self.shared.manageable.store(false, Ordering::SeqCst);
            self.store = None;
            return Ok(self.shared.publish(false, None));
        }
        if self.store.is_none() {
            self.store = Some(
                HistoryStore::open(
                    self.path.as_ref().map_err(Clone::clone)?,
                    HistoryPreference::SaveRedactedText,
                )
                .map_err(mapped)?,
            );
        }
        self.shared.manageable.store(true, Ordering::SeqCst);
        self.shared.enabled.store(true, Ordering::SeqCst);
        // Never adopt a disk revision simply because it is newer. Only explicit
        // restore or our own successful writes grant an expected revision.
        for capture in &captures {
            if self.forgotten.contains(&capture.chat_id) {
                continue;
            }
            match self.store.as_ref().unwrap().read(&capture.chat_id) {
                Ok(row)
                    if self
                        .known
                        .get(&capture.chat_id)
                        .is_none_or(|known| known.revision != row.revision) =>
                {
                    return Err(mapped(HistoryError::Conflict))
                }
                Ok(_) | Err(HistoryError::NotFound) => {}
                Err(error) => return Err(mapped(error)),
            }
        }
        for capture in captures {
            if epoch != self.shared.epoch.load(Ordering::SeqCst) {
                return Err(mapped(HistoryError::Conflict));
            }
            self.save(capture)?;
        }
        if epoch != self.shared.epoch.load(Ordering::SeqCst) {
            return Err(mapped(HistoryError::Conflict));
        }
        Ok(self.shared.publish(true, None))
    }
    fn save(&mut self, capture: Capture) -> Result<(), ApiError> {
        if self.forgotten.contains(&capture.chat_id) {
            return Ok(());
        }
        let known = self.known.get(&capture.chat_id);
        if known.is_some_and(|known| capture.live_revision <= known.live_revision) {
            return Ok(());
        }
        let store = self
            .store
            .as_mut()
            .ok_or_else(|| mapped(HistoryError::Disabled))?;
        let prepared = capture
            .prepared
            .redact_again(|text| self.redactor.redact_prose(text))
            .map_err(mapped)?;
        let row = match known {
            Some(known) => store.replace(&prepared, known.revision),
            None => store.create(&prepared),
        }
        .map_err(mapped)?;
        self.known.insert(
            capture.chat_id,
            Known {
                revision: row.revision,
                live_revision: capture.live_revision,
            },
        );
        Ok(())
    }
}
fn entry(row: StoredChat, redactor: &ObservationBuffer) -> HistoryEntry {
    HistoryEntry {
        chat_id: row.chat.chat_id,
        task_id: row.chat.task_id,
        title: redactor.redact_prose(&row.chat.title),
        message_count: row.chat.messages.len(),
        revision: row.revision.to_string(),
        updated_ms: row.updated_ms,
        listed: row.listed,
    }
}
fn revision_number(value: &str) -> Result<i64, ApiError> {
    value
        .parse::<i64>()
        .ok()
        .filter(|n| *n > 0 && n.to_string() == value)
        .ok_or_else(|| mapped(HistoryError::InvalidRecord))
}
fn unavailable() -> ApiError {
    ApiError::new(
        "HISTORY_UNAVAILABLE",
        "The local history worker is unavailable. Current chat remains in memory.",
    )
}
fn mapped(error: HistoryError) -> ApiError {
    let (code,message)=match error {
        HistoryError::Disabled=>("HISTORY_DISABLED","Local history is off. Enable it explicitly to manage saved chats."),
        HistoryError::Conflict=>("HISTORY_CONFLICT","Saved history changed in another instance. Reload the saved list; no newer revision was overwritten."),
        HistoryError::Corrupt=>("HISTORY_CORRUPT","Saved history is damaged. It was not reset or discarded; preserve the files for recovery."),
        HistoryError::UnsupportedSchema=>("HISTORY_SCHEMA_UNSUPPORTED","This history database uses an unsupported format. It was not migrated or reset."),
        HistoryError::LimitReached=>("HISTORY_LIMIT","Local history reached a bounded size or count limit. No older chats were silently evicted."),
        HistoryError::Busy=>("HISTORY_BUSY","Another instance is using local history. Review its current state before trying again."),
        HistoryError::SaveUncertain=>("HISTORY_SAVE_UNCERTAIN","A history save could not be confirmed. Reload saved history before deciding whether to retry."),
        HistoryError::NotFound=>("HISTORY_NOT_FOUND","That saved chat is no longer available."),
        HistoryError::UnsafeFile|HistoryError::InvalidPath=>("HISTORY_UNSAFE_PATH","History requires a private, owned local application directory without links or unsafe permissions."),
        HistoryError::InvalidRecord=>("HISTORY_INVALID_RECORD","That chat or saved revision is invalid or exceeds the history bounds."),
        HistoryError::Unavailable=>("HISTORY_UNAVAILABLE","Local history could not be accessed. Current chat remains in memory."),
    };
    ApiError::new(code, message)
}
/// Platform state directory only; never CWD, a model-file override, or a network path.
pub fn resolve_history_root() -> Result<PathBuf, ApiError> {
    #[cfg(windows)]
    let path = std::env::var_os("LOCALAPPDATA")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .map(|p| p.join("xyzaishell").join("state"));
    #[cfg(target_os = "linux")]
    let path = std::env::var_os("XDG_STATE_HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|p| !p.is_empty())
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .map(|p| p.join(".local").join("state"))
        })
        .map(|p| p.join("xyzaishell"));
    #[cfg(not(any(windows, target_os = "linux")))]
    let path: Option<PathBuf> = None;
    path.ok_or_else(|| mapped(HistoryError::InvalidPath))
}
#[cfg(test)]
#[path = "history_tests.rs"]
mod tests;
