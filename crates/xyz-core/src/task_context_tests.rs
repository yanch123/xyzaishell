use super::*;

fn context() -> TaskContextSnapshot {
    TaskContextSnapshot::new(
        "task".into(),
        "run".into(),
        SessionRef {
            session_id: "connection".into(),
            generation: "1".into(),
        },
    )
}
fn action(id: &str, completed: bool) -> PreparedAction {
    PreparedAction {
        kind: "command".into(),
        secret_field: None,
        action_id: id.into(),
        display_revision: "1".into(),
        command_text: "command uname -r".into(),
        append_enter: true,
        digest: String::new(),
        policy: "normal".into(),
        risk: "normal".into(),
        blocked: false,
        exit_code: completed.then_some(0),
        command_completed: completed,
        reason: String::new(),
        approval_id: None,
        expires_at_unix_ms: 0,
        displayed: true,
        delivery: "write_accepted".into(),
    }
}
fn output(text: &str) -> Observation {
    Observation {
        text: text.into(),
        truncated: false,
        gap: false,
        untrusted: true,
    }
}
fn finding(id: &str, command_id: &str) -> TaskFinding {
    TaskFinding {
        id: id.into(),
        text: "Installed kernel is 6.8 (interpreted from output).".into(),
        evidence_command_ids: vec![command_id.into()],
    }
}

#[test]
fn host_lifecycle_and_model_findings_remain_distinct() {
    let redactor = ObservationBuffer::new();
    let mut c = context();
    c.observe_action(&action("first", false), &redactor);
    c.retain_output("first", &output("6.8.0"), &redactor);
    c.apply_model(
        ModelTaskContext {
            findings: vec![finding("version", "first")],
            ..Default::default()
        },
        &redactor,
    )
    .unwrap();
    assert_eq!(c.findings.len(), 1);
    assert!(c.confirmed_facts.is_empty());
    c.observe_action(&action("first", true), &redactor);
    assert_eq!(c.confirmed_facts.len(), 1);
    assert_eq!(c.confirmed_facts[0].kind, HostFactKind::ForegroundReturned);
    assert_eq!(c.findings.len(), 1);
    c.observe_action(&action("first", true), &redactor);
    assert_eq!(
        c.confirmed_facts.len(),
        1,
        "facts are not duplicated by emissions"
    );
}

#[test]
fn host_fields_cannot_be_injected_by_model_or_unknown_json_fields() {
    for field in [
        "confirmed_facts",
        "evidence",
        "task_id",
        "run_id",
        "session",
        "approved",
        "completed",
    ] {
        let mut value = serde_json::to_value(ModelTaskContext::default()).unwrap();
        value[field] = serde_json::json!(true);
        assert!(
            serde_json::from_value::<ModelTaskContext>(value).is_err(),
            "{field}"
        );
    }
    assert!(serde_json::from_str::<ModelTaskContext>(r#"{"summary":"first","summary":"second","findings":[],"hypotheses":[],"remaining_steps":[]}"#).is_err());
}

#[test]
fn dangling_foreign_empty_and_duplicate_evidence_are_rejected_atomically() {
    let redactor = ObservationBuffer::new();
    let mut c = context();
    c.observe_action(&action("first", true), &redactor);
    let before = c.clone();
    for references in [
        vec![],
        vec!["foreign-run"],
        vec!["first"],
        vec!["first", "first"],
    ] {
        let mut f = finding("version", "first");
        f.evidence_command_ids = references.into_iter().map(str::to_owned).collect();
        assert!(c
            .apply_model(
                ModelTaskContext {
                    findings: vec![f],
                    ..Default::default()
                },
                &redactor
            )
            .is_err());
        assert_eq!(c, before);
    }
    c.retain_output("first", &output("6.8.0"), &redactor);
    c.apply_model(
        ModelTaskContext {
            findings: vec![finding("version", "first")],
            ..Default::default()
        },
        &redactor,
    )
    .unwrap();
}

#[test]
fn completion_step_needs_real_completed_evidence_but_does_not_establish_goal_success() {
    let redactor = ObservationBuffer::new();
    let mut c = context();
    c.observe_action(&action("first", false), &redactor);
    let update = ModelTaskContext {
        remaining_steps: vec![RemainingStep {
            id: "inspect".into(),
            text: "Read the installed version".into(),
            status: StepStatus::Completed,
            evidence_command_ids: vec!["first".into()],
        }],
        ..Default::default()
    };
    assert!(c.apply_model(update.clone(), &redactor).is_err());
    let mut returned = action("first", true);
    returned.exit_code = Some(1);
    c.observe_action(&returned, &redactor);
    c.apply_model(update, &redactor).unwrap();
    assert_eq!(c.confirmed_facts[0].exit_code, 1);
    assert_eq!(c.remaining_steps[0].status, StepStatus::Completed);
    assert!(c.history_text().contains("model-reported"));
    assert!(c
        .history_text()
        .contains("do not establish business success"));
}

#[test]
fn hypothesis_may_have_no_evidence_but_cannot_cite_nonexistent_command() {
    let redactor = ObservationBuffer::new();
    let mut c = context();
    let mut f = finding("possible_version", "missing");
    assert!(c
        .apply_model(
            ModelTaskContext {
                hypotheses: vec![f.clone()],
                ..Default::default()
            },
            &redactor
        )
        .is_err());
    f.evidence_command_ids.clear();
    c.apply_model(
        ModelTaskContext {
            hypotheses: vec![f],
            ..Default::default()
        },
        &redactor,
    )
    .unwrap();
    assert_eq!(c.hypotheses.len(), 1);
    assert!(c.findings.is_empty() && c.confirmed_facts.is_empty());
}

#[test]
fn scope_is_exact_and_new_runs_or_connections_start_empty() {
    let c = context();
    assert!(c.matches("task", "run", &c.session));
    assert!(!c.matches("other-task", "run", &c.session));
    assert!(!c.matches("task", "other-run", &c.session));
    assert!(!c.matches(
        "task",
        "run",
        &SessionRef {
            generation: "2".into(),
            ..c.session.clone()
        }
    ));
    let new = TaskContextSnapshot::new("task".into(), "new-run".into(), c.session.clone());
    assert!(new.evidence.is_empty() && new.findings.is_empty() && new.remaining_steps.is_empty());
}

#[test]
fn old_output_survives_subsequent_rounds_and_preserves_uncertainty_flags() {
    let redactor = ObservationBuffer::new();
    let mut c = context();
    c.observe_action(&action("first", true), &redactor);
    let mut observation = output("6.8.0");
    observation.gap = true;
    observation.truncated = true;
    c.retain_output("first", &observation, &redactor);
    c.apply_model(
        ModelTaskContext {
            findings: vec![finding("version", "first")],
            ..Default::default()
        },
        &redactor,
    )
    .unwrap();
    c.observe_action(&action("second", true), &redactor);
    c.retain_output("second", &output("service active"), &redactor);
    c.retain_output("first", &output(""), &redactor);
    assert_eq!(c.evidence[0].output_excerpt, "6.8.0");
    assert!(c.evidence[0].output_gap && c.evidence[0].output_truncated && c.evidence[0].untrusted);
    assert_eq!(c.findings[0].evidence_command_ids, ["first"]);
}

#[test]
fn secrets_are_redacted_on_insert_and_again_after_registration() {
    let redactor = ObservationBuffer::new();
    redactor.add_known_secret("old-secret").unwrap();
    let mut c = context();
    let mut a = action("first", true);
    a.command_text = "cat old-secret new-secret".into();
    c.observe_action(&a, &redactor);
    c.retain_output("first", &output("old-secret new-secret"), &redactor);
    let mut f = finding("version", "first");
    f.text = "old-secret new-secret".into();
    c.apply_model(
        ModelTaskContext {
            summary: "old-secret new-secret".into(),
            findings: vec![f],
            ..Default::default()
        },
        &redactor,
    )
    .unwrap();
    assert!(!serde_json::to_string(&c).unwrap().contains("old-secret"));
    redactor.add_known_secret("new-secret").unwrap();
    c.redact(&redactor);
    let json = serde_json::to_string(&c).unwrap();
    assert!(!json.contains("new-secret") && !json.contains("old-secret"));
    assert!(json.contains("[REDACTED]"));
}

#[test]
fn secret_input_and_unsent_output_are_not_command_evidence() {
    let redactor = ObservationBuffer::new();
    let mut c = context();
    let mut secret = action("secret", true);
    secret.kind = "secret_input".into();
    c.observe_action(&secret, &redactor);
    c.retain_output("secret", &output("sensitive"), &redactor);
    assert!(c.evidence.is_empty());
    let mut unsent = action("unsent", false);
    unsent.delivery = "not_sent".into();
    c.observe_action(&unsent, &redactor);
    c.retain_output("unsent", &output("not attributed"), &redactor);
    assert!(c.evidence[0].output_excerpt.is_empty());
}

#[test]
fn bounded_unicode_evidence_never_expands_without_limit() {
    let redactor = ObservationBuffer::new();
    let mut c = context();
    for i in 0..MAX_EVIDENCE + 5 {
        let mut a = action(&format!("command-{i}"), true);
        a.command_text = "中".repeat(2500);
        c.observe_action(&a, &redactor);
        c.retain_output(&a.action_id, &output(&"中".repeat(6000)), &redactor);
    }
    assert_eq!(c.evidence.len(), MAX_EVIDENCE);
    assert_eq!(c.confirmed_facts.len(), MAX_EVIDENCE);
    assert!(c.truncated);
    assert!(serde_json::to_vec(&c).unwrap().len() <= MAX_CONTEXT_BYTES);
    assert!(c
        .evidence
        .iter()
        .all(|e| e.output_excerpt.len() <= MAX_OUTPUT_BYTES && e.output_truncated));
}

#[test]
fn invalid_lengths_counts_ids_and_controls_leave_context_unchanged() {
    let redactor = ObservationBuffer::new();
    let mut c = context();
    for update in [
        ModelTaskContext {
            summary: "x".repeat(1025),
            ..Default::default()
        },
        ModelTaskContext {
            summary: "hidden\0control".into(),
            ..Default::default()
        },
        ModelTaskContext {
            hypotheses: vec![TaskFinding {
                id: "bad id".into(),
                text: "text".into(),
                evidence_command_ids: vec![],
            }],
            ..Default::default()
        },
        ModelTaskContext {
            hypotheses: (0..9)
                .map(|i| TaskFinding {
                    id: format!("h{i}"),
                    text: "text".into(),
                    evidence_command_ids: vec![],
                })
                .collect(),
            ..Default::default()
        },
        ModelTaskContext {
            hypotheses: vec![TaskFinding {
                id: "id".into(),
                text: "".into(),
                evidence_command_ids: vec![],
            }],
            ..Default::default()
        },
    ] {
        assert!(c.apply_model(update, &redactor).is_err());
        assert_eq!(c, context());
    }
}

#[test]
fn history_has_only_labeled_inert_interpretations() {
    let redactor = ObservationBuffer::new();
    let mut c = context();
    c.observe_action(&action("private-command-id", true), &redactor);
    c.retain_output(
        "private-command-id",
        &output("raw-private-output"),
        &redactor,
    );
    c.apply_model(
        ModelTaskContext {
            findings: vec![finding("version", "private-command-id")],
            ..Default::default()
        },
        &redactor,
    )
    .unwrap();
    let text = c.history_text();
    assert!(text.contains("not independently verified"));
    assert!(!text.contains("private-command-id") && !text.contains("raw-private-output"));
    assert!(!text.contains("command uname"));
}

#[test]
fn repeatedly_redacting_late_secret_in_model_ids_never_expands_context() {
    let redactor = ObservationBuffer::new();
    let mut c = context();
    let id = "E".repeat(48);
    c.apply_model(
        ModelTaskContext {
            summary: "E".repeat(1024),
            hypotheses: vec![TaskFinding {
                id: id.clone(),
                text: "E".repeat(384),
                evidence_command_ids: vec![],
            }],
            remaining_steps: vec![RemainingStep {
                id,
                text: "E".repeat(256),
                status: StepStatus::Pending,
                evidence_command_ids: vec![],
            }],
            ..Default::default()
        },
        &redactor,
    )
    .unwrap();
    redactor.add_known_secret("E").unwrap();
    for _ in 0..128 {
        c.redact(&redactor);
        assert!(serde_json::to_vec(&c).unwrap().len() <= MAX_CONTEXT_BYTES);
        assert!(c.hypotheses.is_empty() && c.remaining_steps.is_empty());
        assert!(c.summary.len() <= 1024);
    }
    assert!(c.truncated);
}

#[test]
fn late_short_secret_keeps_maximal_context_within_total_budget() {
    let redactor = ObservationBuffer::new();
    let mut c = context();
    for i in 0..MAX_EVIDENCE {
        let a = action(&format!("command-{i}"), true);
        c.observe_action(&a, &redactor);
        c.retain_output(
            &a.action_id,
            &output(&"E".repeat(MAX_OUTPUT_BYTES)),
            &redactor,
        );
    }
    let ids: Vec<String> = c
        .evidence
        .iter()
        .take(4)
        .map(|e| e.command_id.clone())
        .collect();
    c.apply_model(
        ModelTaskContext {
            summary: "E".repeat(1024),
            findings: (0..8)
                .map(|i| TaskFinding {
                    id: format!("f{i}"),
                    text: "E".repeat(384),
                    evidence_command_ids: ids.clone(),
                })
                .collect(),
            hypotheses: (0..8)
                .map(|i| TaskFinding {
                    id: format!("h{i}"),
                    text: "E".repeat(384),
                    evidence_command_ids: ids.clone(),
                })
                .collect(),
            remaining_steps: (0..12)
                .map(|i| RemainingStep {
                    id: format!("s{i}"),
                    text: "E".repeat(256),
                    status: StepStatus::Pending,
                    evidence_command_ids: ids.clone(),
                })
                .collect(),
        },
        &redactor,
    )
    .unwrap();
    redactor.add_known_secret("E").unwrap();
    for _ in 0..32 {
        c.redact(&redactor);
        assert!(serde_json::to_vec(&c).unwrap().len() <= MAX_CONTEXT_BYTES);
        assert!(c
            .findings
            .iter()
            .chain(&c.hypotheses)
            .all(|f| f.id.len() <= 48 && f.text.len() <= 384));
        assert!(c
            .remaining_steps
            .iter()
            .all(|s| s.id.len() <= 48 && s.text.len() <= 256));
    }
}

#[test]
fn evidence_filters_full_command_with_late_shared_secret_before_clipping() {
    let run = ObservationBuffer::new();
    let shared = ObservationBuffer::new();
    let secret = "late-loaded-model-key-SENSITIVE-SUFFIX";
    shared.add_known_secret(secret).unwrap();
    let mut a = action("first", true);
    a.command_text = format!("command cat /tmp/{}{}", "x".repeat(230), secret);
    let mut c = context();
    c.observe_action_with_shared(&a, &run, &shared);
    let wire = serde_json::to_string(&c).unwrap();
    assert!(!wire.contains("late-load") && !wire.contains("SENSITIVE-SUFFIX"));
    assert!(wire.contains("REDACT"));
}

#[test]
fn seventeenth_command_remains_citable_without_replay() {
    let redactor = ObservationBuffer::new();
    let mut c = context();
    for n in 0..20 {
        let id = format!("command-{n}");
        c.observe_action(&action(&id, true), &redactor);
        c.retain_output(&id, &output(&format!("observed result {n}")), &redactor);
    }
    assert_eq!(c.evidence.len(), 20);
    c.apply_model(
        ModelTaskContext {
            findings: vec![finding("seventeenth", "command-16")],
            remaining_steps: vec![RemainingStep {
                id: "step".into(),
                text: "Foreground returned; business success unknown".into(),
                status: StepStatus::Completed,
                evidence_command_ids: vec!["command-16".into()],
            }],
            ..Default::default()
        },
        &redactor,
    )
    .unwrap();
    assert_eq!(c.findings[0].evidence_command_ids, vec!["command-16"]);
    assert!(c
        .confirmed_facts
        .iter()
        .any(|f| f.command_id == "command-16"));
}
