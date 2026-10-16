//! A separate retained terminal pool, reserved inside the managed 64 MiB budget.
//! Admission never discards an old record, evidence, or an uncertain execution.
use super::*;
use std::mem::size_of;
const POOL_BYTES: usize = 16 * 1024 * 1024;
const LIVE_RESERVE: usize = 12 * 1024 * 1024;
const SNAPSHOT_LIMIT: usize = 3 * 1024 * 1024;
fn strings(values: &[&String]) -> usize {
    values.iter().map(|s| s.capacity()).sum()
}
fn ids(values: &Vec<String>) -> usize {
    values.capacity() * size_of::<String>() + values.iter().map(String::capacity).sum::<usize>()
}
fn action(a: &PreparedAction) -> usize {
    strings(&[
        &a.kind,
        &a.action_id,
        &a.display_revision,
        &a.command_text,
        &a.digest,
        &a.policy,
        &a.risk,
        &a.reason,
        &a.delivery,
    ]) + a.secret_field.as_ref().map_or(0, String::capacity)
        + a.approval_id.as_ref().map_or(0, String::capacity)
}
fn snapshot_bytes(s: &TerminalTaskSnapshot) -> usize {
    let mut bytes = size_of::<TerminalTaskSnapshot>()
        + strings(&[
            &s.chat_id,
            &s.task_id,
            &s.run_id,
            &s.revision,
            &s.phase,
            &s.instruction,
            &s.assistant_text,
            &s.remote_outcome,
            &s.target.host,
            &s.target.username,
            &s.session.session_id,
            &s.session.generation,
        ]);
    bytes += s.actions.capacity() * size_of::<PreparedAction>()
        + s.actions.iter().map(action).sum::<usize>()
        + s.action.as_ref().map_or(0, action);
    if let Some(e) = &s.error {
        bytes += strings(&[&e.code, &e.safe_message, &e.retry]);
    }
    if let Some(c) = &s.task_context {
        bytes += size_of::<TaskContextSnapshot>()
            + strings(&[
                &c.task_id,
                &c.run_id,
                &c.session.session_id,
                &c.session.generation,
                &c.summary,
            ]);
        bytes += c.confirmed_facts.capacity() * size_of::<crate::task_context::ConfirmedFact>()
            + c.confirmed_facts
                .iter()
                .map(|f| f.command_id.capacity())
                .sum::<usize>();
        for list in [&c.findings, &c.hypotheses] {
            bytes += list.capacity() * size_of::<crate::task_context::TaskFinding>()
                + list
                    .iter()
                    .map(|f| strings(&[&f.id, &f.text]) + ids(&f.evidence_command_ids))
                    .sum::<usize>();
        }
        bytes += c.remaining_steps.capacity() * size_of::<crate::task_context::RemainingStep>()
            + c.remaining_steps
                .iter()
                .map(|f| strings(&[&f.id, &f.text]) + ids(&f.evidence_command_ids))
                .sum::<usize>();
        bytes += c.evidence.capacity() * size_of::<crate::task_context::CommandEvidence>()
            + c.evidence
                .iter()
                .map(|e| {
                    strings(&[
                        &e.command_id,
                        &e.command_text,
                        &e.delivery,
                        &e.output_excerpt,
                    ])
                })
                .sum::<usize>();
    }
    // Secret requests and their bounded field descriptors are conservatively reserved.
    bytes + 64 * 1024
}
fn error() -> ApiError {
    ApiError::new("TERMINAL_MEMORY_LIMIT", "The retained terminal evidence pool is full. No prior evidence was deleted. Finish the current task and start the next task in the same chat to replace its old terminal record, or restart the app after saving required history.")
}
pub(super) fn check_snapshot(snapshot: &TerminalTaskSnapshot) -> Result<(), ApiError> {
    if snapshot_bytes(snapshot) > SNAPSHOT_LIMIT {
        Err(error())
    } else {
        Ok(())
    }
}
fn retained(r: &Record) -> usize {
    if r.snapshot.owner && r.snapshot.phase != "completed" {
        return LIVE_RESERVE;
    }
    // Includes cache capacities and a fixed allowance for bounded config, filters,
    // identities and control metadata. The conversation vector is intentionally empty.
    snapshot_bytes(&r.snapshot) + r.output.memory_bytes() + 1024 * 1024
}
fn fits(existing: usize) -> Result<(), ApiError> {
    if existing.saturating_add(LIVE_RESERVE) > POOL_BYTES {
        Err(error())
    } else {
        Ok(())
    }
}
pub(super) fn admit(records: &BTreeMap<String, Record>, chat_id: &str) -> Result<(), ApiError> {
    fits(
        records
            .values()
            .filter(|r| r.snapshot.chat_id != chat_id)
            .map(retained)
            .sum(),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    fn record(chat: &str) -> Record {
        let snapshot = serde_json::from_value(serde_json::json!({
            "chat_id":chat,"task_id":chat,"run_id":"run","session":{"session_id":"session","generation":"1"},
            "target":{"host":"localhost","port":22,"username":"fixture"},"revision":"1","phase":"completed",
            "instruction":"test","assistant_text":"done","action":null,"actions":[],"error":null,"owner":false,
            "remote_outcome":"not_sent","terminal_ready":true,"round":1,"batch_round":1
        })).unwrap();
        Record {
            snapshot,
            config: Arc::new(ModelSnapshot {
                destination: "https://example.com/v1".into(),
                model: "fixture".into(),
                api_key: zeroize::Zeroizing::new(String::new()),
                request: crate::model::RequestOptions::default(),
            }),
            redactor: ObservationBuffer::new(),
            shared_redactor: ObservationBuffer::new(),
            cancel: watch::channel(false).0,
            revision: 1,
            ready_binding: None,
            conversation: vec![],
            output: ActionOutputStore::default(),
            output_request: None,
        }
    }
    #[test]
    fn actual_records_admit_small_prior_reject_large_and_allow_replacement() {
        let old = record("old");
        let mut records = BTreeMap::from([("old".to_owned(), old)]);
        assert!(admit(&records, "new").is_ok());
        let old = records.get_mut("old").unwrap();
        for i in 0..5 {
            let id = i.to_string();
            old.output.start(&id);
            old.output.append(&"x".repeat(1024 * 1024));
            old.output.finish(&id);
        }
        let before = old.output.memory_bytes();
        assert_eq!(
            admit(&records, "new").unwrap_err().code,
            "TERMINAL_MEMORY_LIMIT"
        );
        assert_eq!(records["old"].output.memory_bytes(), before);
        assert!(admit(&records, "old").is_ok());
        assert_eq!(
            records["old"].output.read("0", 0, 10).unwrap().text,
            "xxxxxxxxxx"
        );
    }
    #[test]
    fn cancelled_ownerless_record_uses_actual_allocations() {
        let mut old = record("old");
        old.snapshot.phase = "cancelled".into();
        assert!(admit(&BTreeMap::from([("old".into(), old)]), "new").is_ok());
    }
    #[test]
    fn tiny_completed_records_allow_another_run() {
        assert!(fits(1024 * 1024).is_ok());
    }
    #[test]
    fn large_retained_records_fail_without_reclamation() {
        assert_eq!(
            fits(5 * 1024 * 1024).unwrap_err().code,
            "TERMINAL_MEMORY_LIMIT"
        );
    }
    #[test]
    fn same_chat_replacement_has_full_headroom() {
        assert!(fits(0).is_ok());
    }
}
