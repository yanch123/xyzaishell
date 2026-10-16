use super::*;
use crate::context::*;
fn manager() -> TaskManager {
    TaskManager::new(
        ModelConfigManager::from_path(Err(ApiError::new("TEST", "No provider"))),
        Arc::new(|_| true),
    )
}
fn record(count: usize, covered: bool) -> ChatRecord {
    let id = Uuid::new_v4().to_string();
    let messages: Vec<_> = (0..count)
        .map(|_| ChatMessage {
            id: Uuid::new_v4().to_string(),
            role: MessageRole::Assistant,
            text: "Repeated inert output line. ".repeat(2300),
            run_id: String::new(),
            state: MessageState::Complete,
            error: None,
            destination: None,
            model: None,
        })
        .collect();
    let mut ledger = ContextLedger::new(&id);
    for (sequence, message) in messages.iter().enumerate() {
        ledger
            .append(ContextSegment::new(
                &message.id,
                sequence as u64,
                ContextCategory::Dialogue,
                message.text.clone(),
            ))
            .unwrap();
    }
    if covered {
        let before = prepare_context(
            &id,
            &ledger.fence,
            RequestKind::Conversation,
            MeasurementScope::CommittedContext,
            &ledger.active_fragments(),
            count * 65000,
            &ContextBudgetConfig::default(),
        )
        .unwrap();
        let job = ledger
            .begin_compaction(
                ledger.fence.context_revision,
                "retention-test",
                CompactionMode::Quick,
                TriggerReason::Manual,
                true,
            )
            .unwrap();
        let candidate = ledger.quick_candidate(&job).unwrap();
        let after = prepare_context(
            &id,
            &ledger.fence,
            RequestKind::Conversation,
            MeasurementScope::CommittedContext,
            &ledger.candidate_fragments(&job, &candidate).unwrap(),
            30000,
            &ContextBudgetConfig::default(),
        )
        .unwrap();
        ledger
            .commit_compaction(&job, candidate, &before, &after, true)
            .unwrap();
    }
    let mut context = context_runtime::RuntimeContext::new(&id);
    context.retention_test_install_ledger(ledger);
    ChatRecord {
        snapshot: ChatSnapshot {
            chat_id: id,
            task_id: Uuid::new_v4().to_string(),
            title: "Retention test".into(),
            source: None,
            restored_read_only: false,
            messages,
            active_run_id: None,
            revision: "1".into(),
        },
        revision: 1,
        cancel: None,
        terminal_run: None,
        summarized_terminal_runs: BTreeSet::new(),
        conversational_terminal_runs: BTreeSet::new(),
        context,
    }
}
#[test]
fn covered_originals_do_not_expire_until_independent_ram_pressure() {
    let manager = manager();
    let record = record(12, true);
    let id = record.snapshot.chat_id.clone();
    let mut chats = BTreeMap::from([(id.clone(), record)]);
    let original = chats[&id].snapshot.messages[0].text.clone();
    manager
        .ensure_memory_capacity_locked(&mut chats, &id, 1024)
        .unwrap();
    assert_eq!(chats[&id].snapshot.messages[0].text, original);
}
#[test]
fn memory_only_expiration_is_visible_and_keeps_stable_reference_tombstones() {
    let manager = manager();
    let record = record(70, true);
    let id = record.snapshot.chat_id.clone();
    let mut chats = BTreeMap::from([(id.clone(), record)]);
    assert!(record_memory_bytes(&chats[&id]) > CHAT_MEMORY_BYTES);
    manager
        .ensure_memory_capacity_locked(&mut chats, &id, 10000)
        .unwrap();
    assert!(record_memory_bytes(&chats[&id]) + 24096 <= CHAT_MEMORY_BYTES);
    assert!(chats[&id]
        .snapshot
        .messages
        .iter()
        .any(|m| m.error.as_ref().is_some_and(|e| e.code == EXPIRED_CODE)));
    assert_eq!(chats[&id].snapshot.messages.len(), 70);
    assert!(chats[&id].context.retention_memory_bytes() > 0);
}
#[test]
fn active_or_uncovered_sources_fail_closed_without_mutation() {
    for active in [false, true] {
        let manager = manager();
        let mut record = record(70, active);
        let id = record.snapshot.chat_id.clone();
        if active {
            record.terminal_run = Some("in-flight-action".into());
        }
        let original = record.snapshot.messages[0].text.clone();
        let mut chats = BTreeMap::from([(id.clone(), record)]);
        assert_eq!(
            manager
                .ensure_memory_capacity_locked(&mut chats, &id, 10000)
                .unwrap_err()
                .code,
            "CONTEXT_MEMORY_LIMIT"
        );
        assert_eq!(chats[&id].snapshot.messages[0].text, original);
    }
}
#[test]
fn process_pressure_can_reclaim_other_idle_chats_without_ending_tasks() {
    let manager = manager();
    let mut chats = BTreeMap::new();
    for _ in 0..9 {
        let record = record(55, true);
        chats.insert(record.snapshot.chat_id.clone(), record);
    }
    let target = chats.keys().next().unwrap().clone();
    manager
        .ensure_memory_capacity_locked(&mut chats, &target, 1000)
        .unwrap();
    let total: usize = chats
        .iter()
        .map(|(id, r)| id.capacity() + record_memory_bytes(r))
        .sum();
    assert!(
        total + INFLIGHT_RESERVE_BYTES + AGENT_POOL_RESERVE_BYTES + 6096 <= PROCESS_MEMORY_BYTES
    );
    assert!(chats
        .values()
        .any(|r| r.snapshot.messages.iter().any(|m| m.error.is_some())));
    assert!(chats.values().all(|r| r.terminal_run.is_none()));
}
#[test]
fn saved_archive_cache_unload_preserves_original_disk_pages_and_unsaved_blocks() {
    let path = std::env::temp_dir().join(format!("xyz-retention-{}", Uuid::new_v4()));
    let history = HistoryService::new(
        Ok(path.clone()),
        crate::observation::ObservationBuffer::new(),
        Arc::new(|_| {}),
    );
    let mut manager = manager();
    manager.history = Some(history.clone());
    let record = record(70, true);
    let id = record.snapshot.chat_id.clone();
    let original = record.snapshot.messages[0].text.clone();
    history
        .set_enabled(true, std::slice::from_ref(&record.snapshot))
        .unwrap();
    let mut chats = BTreeMap::from([(id.clone(), record)]);
    // A newer unsaved snapshot cannot be unloaded even when an older disk row exists.
    chats.get_mut(&id).unwrap().changed();
    assert!(manager
        .ensure_memory_capacity_locked(&mut chats, &id, 10000)
        .is_err());
    assert_eq!(chats[&id].snapshot.messages[0].text, original);
    history.capture(&chats[&id].snapshot);
    history.flush().unwrap();
    manager
        .ensure_memory_capacity_locked(&mut chats, &id, 10000)
        .unwrap();
    assert_eq!(
        chats[&id].snapshot.messages[0].error.as_ref().unwrap().code,
        ARCHIVED_CODE
    );
    chats.get_mut(&id).unwrap().changed();
    history.capture(&chats[&id].snapshot);
    history.flush().unwrap();
    let row = history.list().unwrap().remove(0);
    let mut cursor = None;
    let mut found = false;
    loop {
        let page = history.page(&id, &row.revision, cursor, 128).unwrap();
        if page
            .chat
            .messages
            .iter()
            .any(|m| m.id == chats[&id].snapshot.messages[0].id && m.text == original)
        {
            found = true;
        }
        cursor = page.next_before;
        if cursor.is_none() {
            break;
        }
    }
    assert!(found);
    assert_eq!(row.message_count, 70);
    history.set_enabled(false, &[]).unwrap();
    drop(history);
    drop(manager);
    let _ = std::fs::remove_dir_all(path);
}
#[test]
fn quota_failure_does_not_make_partial_retention_plan_visible() {
    let manager = manager();
    let record = record(70, true);
    let id = record.snapshot.chat_id.clone();
    let original = record.snapshot.messages[0].text.clone();
    let mut chats = BTreeMap::from([(id.clone(), record)]);
    assert!(manager
        .ensure_memory_capacity_locked(&mut chats, &id, CHAT_MEMORY_BYTES)
        .is_err());
    assert_eq!(chats[&id].snapshot.messages[0].text, original);
}

#[test]
fn disabled_history_retention_never_creates_disk_cache_and_later_optin_skips_expired_originals() {
    let path = std::env::temp_dir().join(format!("xyz-retention-off-{}", Uuid::new_v4()));
    let history = HistoryService::new(
        Ok(path.clone()),
        crate::observation::ObservationBuffer::new(),
        Arc::new(|_| {}),
    );
    let mut manager = manager();
    manager.history = Some(history.clone());
    let record = record(70, true);
    let id = record.snapshot.chat_id.clone();
    let mut chats = BTreeMap::from([(id.clone(), record)]);
    manager
        .ensure_memory_capacity_locked(&mut chats, &id, 10000)
        .unwrap();
    assert!(!path.exists());
    let expired = chats[&id]
        .snapshot
        .messages
        .iter()
        .filter(|m| m.error.as_ref().is_some_and(|e| e.code == EXPIRED_CODE))
        .count();
    assert!(expired > 0);
    history
        .set_enabled(true, std::slice::from_ref(&chats[&id].snapshot))
        .unwrap();
    let row = history.list().unwrap().remove(0);
    assert_eq!(row.message_count, 70 - expired);
    history.set_enabled(false, &[]).unwrap();
    drop(history);
    drop(manager);
    let _ = std::fs::remove_dir_all(path);
}
