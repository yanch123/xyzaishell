//! Independent, bounded RAM retention. Compaction never calls this as a side
//! effect. Archive records are untouched; only host display/ledger caches change.
use super::*;
use std::mem::size_of;

pub(super) const CHAT_MEMORY_BYTES: usize = 8 * 1024 * 1024;
pub(super) const PROCESS_MEMORY_BYTES: usize = 64 * 1024 * 1024;
// Managed context budget, not a guarantee about allocator/OS RSS: reserve
// 8 MiB for four bounded requests, 8 MiB for the archive queue and 8 MiB
// for one serialized capture preparation. Retained state cannot borrow it.
const INFLIGHT_RESERVE_BYTES: usize = 24 * 1024 * 1024;
// AgentRecord/output/evidence retain their own independently enforced pool.
// This reservation cannot be borrowed by chat caches or compaction state.
const AGENT_POOL_RESERVE_BYTES: usize = 16 * 1024 * 1024;
const MARKER_OVERHEAD: usize = 1024;
const ARCHIVED_CODE: &str = "HISTORY_ARCHIVED_CACHE";
const EXPIRED_CODE: &str = "CONTEXT_ORIGINAL_EXPIRED";

pub(super) fn json_memory_bytes(value: &serde_json::Value) -> usize {
    use serde_json::Value;
    size_of::<Value>()
        + match value {
            Value::String(s) => s.capacity(),
            Value::Array(a) => {
                a.capacity() * size_of::<Value>() + a.iter().map(json_memory_bytes).sum::<usize>()
            }
            Value::Object(o) => o
                .iter()
                .map(|(k, v)| k.capacity() + 96 + json_memory_bytes(v))
                .sum(),
            _ => 0,
        }
}
fn option_bytes(value: &Option<String>) -> usize {
    value.as_ref().map(String::capacity).unwrap_or(0)
}
fn message_bytes(message: &ChatMessage) -> usize {
    message.id.capacity() + message.text.capacity() + message.run_id.capacity()
        + option_bytes(&message.destination) + option_bytes(&message.model)
        + message.error.as_ref().map(|e| e.code.capacity() + e.safe_message.capacity() + e.retry.capacity()).unwrap_or(0)
        // Account for archive gap registry nodes and independently owned IDs.
        + usize::from(message.error.as_ref().is_some_and(|e| e.code == ARCHIVED_CODE || e.code == EXPIRED_CODE)) * 256
}
fn record_memory_bytes(record: &ChatRecord) -> usize {
    let chat = &record.snapshot;
    let source = chat
        .source
        .as_ref()
        .map(|s| {
            s.host.capacity()
                + s.username.capacity()
                + s.session_id.capacity()
                + s.generation.capacity()
        })
        .unwrap_or(0);
    size_of::<ChatRecord>()
        + 128
        + chat.chat_id.capacity()
        + chat.task_id.capacity()
        + chat.title.capacity()
        + chat.revision.capacity()
        + option_bytes(&chat.active_run_id)
        + source
        + chat.messages.capacity() * size_of::<ChatMessage>()
        + chat.messages.iter().map(message_bytes).sum::<usize>()
        + option_bytes(&record.terminal_run)
        + record
            .summarized_terminal_runs
            .iter()
            .chain(record.conversational_terminal_runs.iter())
            .map(|id| id.capacity() + 96)
            .sum::<usize>()
        + record.context.retention_memory_bytes()
        + record.context.retention_unsynced_bytes(&chat.messages)
}
#[derive(Clone, Debug)]
struct Candidate {
    chat_id: String,
    message_id: String,
    sequence: u64,
    saved_bytes: usize,
    recoverable: bool,
}
fn overflow() -> ApiError {
    ApiError::new("CONTEXT_MEMORY_LIMIT", "Protected or unsaved context fills the RAM quota. No required original was discarded. Resolve history storage, finish another chat, reduce the input, or open a new chat.")
}

impl TaskManager {
    /// Read-only quota check for draft previews and other non-mutating queries.
    pub(super) fn check_memory_capacity_locked(
        &self,
        chats: &BTreeMap<String, ChatRecord>,
        chat_id: &str,
        additional: usize,
    ) -> Result<(), ApiError> {
        let incoming = additional
            .checked_mul(2)
            .and_then(|n| n.checked_add(4096))
            .ok_or_else(overflow)?;
        let target = chats.get(chat_id).map(record_memory_bytes).unwrap_or(0);
        let total: usize = chats
            .iter()
            .map(|(id, r)| id.capacity() + record_memory_bytes(r))
            .sum();
        if target.saturating_add(incoming) > CHAT_MEMORY_BYTES
            || total
                .saturating_add(incoming)
                .saturating_add(INFLIGHT_RESERVE_BYTES)
                .saturating_add(AGENT_POOL_RESERVE_BYTES)
                > PROCESS_MEMORY_BYTES
        {
            return Err(overflow());
        }
        Ok(())
    }
    /// Call under the chat mutation lock, before reserving or growing a turn.
    /// `additional` includes the caller's complete bounded response reservation.
    /// The plan is checked before any cache mutation; insufficient space leaves
    /// every original untouched. No disk I/O or model request occurs here.
    pub(super) fn ensure_memory_capacity_locked(
        &self,
        chats: &mut BTreeMap<String, ChatRecord>,
        chat_id: &str,
        additional: usize,
    ) -> Result<(), ApiError> {
        let incoming = additional
            .checked_mul(2)
            .and_then(|n| n.checked_add(4096))
            .ok_or_else(overflow)?;
        let target_bytes = chats.get(chat_id).map(record_memory_bytes).unwrap_or(0);
        let total: usize = chats
            .iter()
            .map(|(id, record)| id.capacity() + record_memory_bytes(record))
            .sum();
        let mut local_need = target_bytes
            .saturating_add(incoming)
            .saturating_sub(CHAT_MEMORY_BYTES);
        let mut global_need = total
            .saturating_add(incoming)
            .saturating_add(INFLIGHT_RESERVE_BYTES)
            .saturating_add(AGENT_POOL_RESERVE_BYTES)
            .saturating_sub(PROCESS_MEMORY_BYTES);
        if local_need == 0 && global_need == 0 {
            return Ok(());
        }
        let status = self.history.as_ref().map(HistoryService::status);
        let archive_requested = status
            .as_ref()
            .is_some_and(|s| s.enabled || s.manageable || s.error.is_some());
        let mut candidates = Vec::new();
        for (id, record) in chats.iter() {
            // A running/streaming/action/approval scope is never reclaimed, even
            // when an old source would otherwise appear covered.
            if record.snapshot.active_run_id.is_some()
                || record.terminal_run.is_some()
                || !record.context.retention_safe()
            {
                continue;
            }
            let recoverable = self
                .history
                .as_ref()
                .and_then(|h| h.saved_live_revision(id))
                .is_some_and(|revision| revision >= record.revision);
            if archive_requested && !recoverable {
                continue;
            }
            for (message_id, sequence, ledger_bytes) in record.context.retention_candidates()? {
                let Some(message) = record.snapshot.messages.iter().find(|m| m.id == message_id)
                else {
                    continue;
                };
                if message.state == MessageState::Streaming {
                    continue;
                }
                let saved_bytes = ledger_bytes
                    .saturating_add(message.text.capacity())
                    .saturating_sub(MARKER_OVERHEAD);
                if saved_bytes == 0 {
                    continue;
                }
                candidates.push(Candidate {
                    chat_id: id.clone(),
                    message_id,
                    sequence,
                    saved_bytes,
                    recoverable,
                });
            }
        }
        candidates.sort_by(|a, b| {
            (a.sequence, &a.chat_id, &a.message_id).cmp(&(b.sequence, &b.chat_id, &b.message_id))
        });
        let mut selected = Vec::new();
        // First satisfy the target's own budget, then consider other idle chats
        // for the process-wide budget. This does not terminate another task.
        for candidate in &candidates {
            if local_need == 0 {
                break;
            }
            if candidate.chat_id != chat_id {
                continue;
            }
            local_need = local_need.saturating_sub(candidate.saved_bytes);
            global_need = global_need.saturating_sub(candidate.saved_bytes);
            selected.push(candidate.clone());
        }
        if local_need != 0 {
            return Err(overflow());
        }
        for candidate in candidates {
            if global_need == 0 {
                break;
            }
            if selected
                .iter()
                .any(|s| s.chat_id == candidate.chat_id && s.message_id == candidate.message_id)
            {
                continue;
            }
            global_need = global_need.saturating_sub(candidate.saved_bytes);
            selected.push(candidate);
        }
        if global_need != 0 {
            return Err(overflow());
        }
        // Validate/register authoritative host cache IDs before touching prose.
        // Registration itself never persists or removes anything from SQLite.
        if let Some(history) = &self.history {
            let requests: Vec<_> = selected
                .iter()
                .map(|candidate| {
                    (
                        candidate.chat_id.clone(),
                        chats[&candidate.chat_id].revision,
                        vec![candidate.message_id.clone()],
                        candidate.recoverable,
                    )
                })
                .collect();
            history.register_cache_gaps_batch(&requests)?;
        }
        let touched: BTreeSet<_> = selected.iter().map(|c| c.chat_id.clone()).collect();
        for candidate in selected {
            let record = chats.get_mut(&candidate.chat_id).ok_or_else(chat_missing)?;
            record
                .context
                .retention_reclaim(&candidate.message_id, candidate.recoverable)?;
            let message = record
                .snapshot
                .messages
                .iter_mut()
                .find(|m| m.id == candidate.message_id)
                .ok_or_else(chat_missing)?;
            let (code, text) = if candidate.recoverable {
                (ARCHIVED_CODE, "Original text is archived. Load saved history to view it; working memory retains its summary.")
            } else {
                (EXPIRED_CODE, "Earlier original text expired from working memory. Only its summary remains here; no recovery source is available in the current history mode.")
            };
            message.text = text.into();
            message.error = Some(ApiError::new(code, text));
            // Cache layout is not a model/authorization revision.
        }
        for id in touched {
            chats.get_mut(&id).ok_or_else(chat_missing)?.changed();
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "tasks_retention_tests.rs"]
mod tests;
