//! Bounded in-memory, redacted PTY windows. Not exclusive command output.
use crate::ssh::ApiError;
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, Weak},
};
use zeroize::Zeroize;
const ACTION_LIMIT: usize = 1024 * 1024;
const RUN_LIMIT: usize = 8 * ACTION_LIMIT;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputPage {
    pub text: String,
    pub next_offset: u64,
    pub earliest_offset: u64,
    pub complete: bool,
    pub has_more: bool,
    pub truncated: bool,
    pub gap: bool,
}
#[derive(Clone, Default)]
pub(crate) struct ActionOutputStore(Arc<Mutex<Store>>);
#[derive(Clone, Default)]
pub(crate) struct OutputRegistry(Arc<Mutex<Vec<Weak<Mutex<Store>>>>>);
impl OutputRegistry {
    pub fn register(&self, store: &ActionOutputStore) {
        let mut stores = self.0.lock().unwrap_or_else(|e| e.into_inner());
        stores.retain(|weak| weak.strong_count() > 0);
        stores.push(Arc::downgrade(&store.0));
    }
    pub fn invalidate_all(&self) {
        let mut stores = self.0.lock().unwrap_or_else(|e| e.into_inner());
        stores.retain(|weak| {
            if let Some(store) = weak.upgrade() {
                ActionOutputStore(store).invalidate();
                true
            } else {
                false
            }
        });
    }
}
#[derive(Default)]
struct Store {
    windows: VecDeque<Window>,
    active: Option<String>,
}
struct Window {
    id: String,
    text: String,
    earliest: u64,
    end: u64,
    complete: bool,
    gap: bool,
}
impl Drop for Window {
    fn drop(&mut self) {
        self.text.zeroize();
    }
}
impl ActionOutputStore {
    pub(crate) fn memory_bytes(&self) -> usize {
        let s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::size_of::<Store>()
            + s.windows.capacity() * std::mem::size_of::<Window>()
            + s.windows
                .iter()
                .map(|w| w.id.capacity() + w.text.capacity())
                .sum::<usize>()
            + s.active.as_ref().map_or(0, String::capacity)
    }
    pub fn start(&self, id: &str) {
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if !s.windows.iter().any(|w| w.id == id) {
            s.windows.push_back(Window {
                id: id.into(),
                text: String::new(),
                earliest: 0,
                end: 0,
                complete: false,
                gap: false,
            });
        }
        s.active = Some(id.into());
    }
    pub fn finish(&self, id: &str) {
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(w) = s.windows.iter_mut().find(|w| w.id == id) {
            w.complete = true;
        }
        if s.active.as_deref() == Some(id) {
            s.active = None;
        }
    }
    pub fn append(&self, text: &str) {
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let active = s.active.clone();
        if let Some(w) = s
            .windows
            .iter_mut()
            .find(|w| Some(&w.id) == active.as_ref())
        {
            w.text.push_str(text);
            w.end += text.len() as u64;
            trim(w, ACTION_LIMIT);
        }
        let mut total: usize = s.windows.iter().map(|w| w.text.len()).sum();
        while total > RUN_LIMIT {
            let index = s
                .windows
                .iter()
                .position(|w| w.complete && !w.text.is_empty())
                .or_else(|| s.windows.iter().position(|w| !w.text.is_empty()));
            let Some(i) = index else { break };
            let w = &mut s.windows[i];
            let before = w.text.len();
            trim(w, before.saturating_sub(total - RUN_LIMIT));
            total -= before - w.text.len();
        }
        if s.windows.iter().map(|w| w.text.capacity()).sum::<usize>() > RUN_LIMIT {
            for window in &mut s.windows {
                window.text.shrink_to_fit();
            }
        }
    }
    /// New secrets invalidate older windows rather than changing established offsets.
    pub fn invalidate(&self) {
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        for w in &mut s.windows {
            w.text.zeroize();
            w.text.clear();
            w.earliest = w.end;
            w.gap = true;
        }
    }
    pub fn read(&self, id: &str, offset: u64, max_bytes: usize) -> Result<OutputPage, ApiError> {
        if !(1..=16384).contains(&max_bytes) {
            return Err(ApiError::new(
                "OUTPUT_RANGE_INVALID",
                "每次读取应为 1–16384 字节。",
            ));
        }
        let s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let w = s.windows.iter().find(|w| w.id == id).ok_or_else(|| {
            ApiError::new("OUTPUT_REFERENCE_STALE", "此动作没有可读取的当前运行输出。")
        })?;
        if offset < w.earliest || offset > w.end {
            return Ok(OutputPage {
                text: String::new(),
                next_offset: w.earliest,
                earliest_offset: w.earliest,
                complete: w.complete,
                has_more: !w.text.is_empty(),
                truncated: true,
                gap: true,
            });
        }
        let start = (offset - w.earliest) as usize;
        if !w.text.is_char_boundary(start) {
            return Err(ApiError::new(
                "OUTPUT_RANGE_INVALID",
                "输出偏移必须位于 UTF-8 字符边界。",
            ));
        }
        let mut end = (start + max_bytes).min(w.text.len());
        while !w.text.is_char_boundary(end) {
            end -= 1;
        }
        if end == start && start < w.text.len() {
            return Err(ApiError::new(
                "OUTPUT_RANGE_INVALID",
                "字节上限不足以读取下一个完整 UTF-8 字符。",
            ));
        }
        Ok(OutputPage {
            text: w.text[start..end].into(),
            next_offset: w.earliest + end as u64,
            earliest_offset: w.earliest,
            complete: w.complete,
            has_more: end < w.text.len(),
            truncated: w.earliest > 0 || end < w.text.len(),
            gap: w.gap,
        })
    }
}
fn trim(w: &mut Window, limit: usize) {
    if w.text.len() > limit {
        let mut n = w.text.len() - limit;
        while !w.text.is_char_boundary(n) {
            n += 1;
        }
        w.text.drain(..n);
        w.earliest += n as u64;
        w.gap = true;
    }
    if w.text.capacity() > limit {
        w.text.shrink_to_fit();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn completed_action_pages_distinguish_lifecycle_completion_from_eof() {
        let store = ActionOutputStore::default();
        store.start("long");
        store.append(&"x".repeat(40_000));
        store.finish("long");
        let first = store.read("long", 0, 16_384).unwrap();
        assert!(first.complete && first.has_more);
        let second = store.read("long", first.next_offset, 16_384).unwrap();
        assert!(second.complete && second.has_more);
        let last = store.read("long", second.next_offset, 16_384).unwrap();
        assert!(last.complete && !last.has_more);
        assert_eq!(last.next_offset, 40_000);
        assert_eq!(
            first.text.len() + second.text.len() + last.text.len(),
            40_000
        );
    }
    #[test]
    fn paging_eviction_utf8_and_secret_invalidation() {
        let s = ActionOutputStore::default();
        s.start("a");
        s.append(&"中".repeat(ACTION_LIMIT / 3 + 5));
        let lost = s.read("a", 0, 16).unwrap();
        assert!(lost.gap);
        assert!(lost.earliest_offset > 0);
        let p = s.read("a", lost.earliest_offset, 16).unwrap();
        assert_eq!(p.text, "中中中中中");
        assert!(s.read("a", lost.earliest_offset + 1, 16).is_err());
        s.invalidate();
        assert!(s.read("a", p.next_offset, 16).unwrap().text.is_empty());
    }
    #[test]
    fn run_limit_bounds_allocated_capacities_not_only_text_lengths() {
        let store = ActionOutputStore::default();
        for i in 0..20 {
            let id = i.to_string();
            store.start(&id);
            for _ in 0..150 {
                store.append(&"x".repeat(4096));
            }
            store.finish(&id);
        }
        let state = store.0.lock().unwrap();
        assert!(
            state
                .windows
                .iter()
                .map(|w| w.text.capacity())
                .sum::<usize>()
                <= RUN_LIMIT
        );
        assert!(state.windows.iter().any(|w| w.gap));
    }
    #[test]
    fn run_limit_drops_completed_oldest_and_keeps_ids() {
        let s = ActionOutputStore::default();
        for i in 0..9 {
            let id = i.to_string();
            s.start(&id);
            s.append(&"x".repeat(ACTION_LIMIT));
            s.finish(&id);
        }
        assert_eq!(
            s.read("0", 0, 10).unwrap().earliest_offset,
            ACTION_LIMIT as u64
        );
        assert_eq!(s.read("8", 0, 10).unwrap().text, "xxxxxxxxxx");
        assert!(s.read("foreign", 0, 1).is_err());
        assert!(s.read("8", 0, 16385).is_err());
    }
}
