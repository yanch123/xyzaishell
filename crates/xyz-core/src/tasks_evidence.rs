//! Request projection for inert, settled command evidence. The live host task
//! retains lifecycle truth and original indexed excerpts independently.
use super::*;
use sha2::{Digest, Sha256};

fn scope(context: &RuntimeContext) -> String {
    let f = &context.ledger.fence;
    let raw = serde_json::to_vec(&json!([
        f.task_id,
        f.run_id,
        f.session_id,
        f.session_generation,
        f.target_identity
    ]))
    .unwrap_or_default();
    format!("{:x}", Sha256::digest(raw))
}
fn source_id(context: &RuntimeContext, command_id: &str) -> String {
    format!("evidence-{}-{command_id}", scope(context))
}
fn prompt_from_template(template: &Value) -> Result<Option<Value>, ApiError> {
    let Some(content) = template
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|messages| {
            messages
                .iter()
                .rev()
                .find(|m| m.get("role").and_then(Value::as_str) == Some("user"))
        })
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str)
    else {
        return Ok(None);
    };
    let prompt = serde_json::from_str::<Value>(content).map_err(|_| {
        ApiError::new(
            "CONTEXT_SERIALIZATION",
            "Planning evidence prompt is not valid JSON.",
        )
    })?;
    Ok(Some(prompt))
}
fn cited_commands(prompt: &Value) -> BTreeSet<String> {
    let mut result = BTreeSet::new();
    for field in ["findings", "hypotheses", "remaining_steps"] {
        if let Some(entries) = prompt
            .get("task_context")
            .and_then(|c| c.get(field))
            .and_then(Value::as_array)
        {
            for entry in entries {
                if let Some(ids) = entry.get("evidence_command_ids").and_then(Value::as_array) {
                    result.extend(ids.iter().filter_map(Value::as_str).map(str::to_owned));
                }
            }
        }
    }
    result
}
/// Admit only settled evidence; current/latest command output remains host-owned
/// until a later command makes it stable. Unknown/in-flight results stay raw and protected.
pub(super) fn sync(context: &mut RuntimeContext, template: &Value) -> Result<(), ApiError> {
    if context.kind != RequestKind::Planning {
        return Ok(());
    }
    let Some(prompt) = prompt_from_template(template)? else {
        return Ok(());
    };
    let Some(evidence) = prompt
        .get("task_context")
        .and_then(|c| c.get("evidence"))
        .and_then(Value::as_array)
    else {
        return Ok(());
    };
    if let Some(task) = prompt.get("task_context") {
        let fence = &context.ledger.fence;
        let mismatch = task
            .get("task_id")
            .and_then(Value::as_str)
            .is_some_and(|id| Some(id) != fence.task_id.as_deref())
            || task
                .get("run_id")
                .and_then(Value::as_str)
                .is_some_and(|id| Some(id) != fence.run_id.as_deref())
            || task
                .get("session")
                .and_then(|v| v.get("session_id"))
                .and_then(Value::as_str)
                .is_some_and(|id| id != fence.session_id)
            || task
                .get("session")
                .and_then(|v| v.get("generation"))
                .and_then(Value::as_str)
                .is_some_and(|generation| generation != fence.session_generation.to_string());
        if mismatch {
            return Err(ApiError::new(
                "CONTEXT_STALE",
                "Evidence belongs to another task, run, or session generation.",
            ));
        }
    }
    let current_prefix = format!("evidence-{}-", scope(context));
    let stale: Vec<_> = context
        .ledger
        .all_segments()
        .into_iter()
        .filter(|s| {
            s.category == ContextCategory::Evidence
                && s.protection == Protection::Required
                && !s.segment_id.starts_with(&current_prefix)
        })
        .map(|s| s.segment_id.clone())
        .collect();
    for id in stale {
        context
            .ledger
            .update_protection(&id, Protection::Ordinary)
            .map_err(context_error)?;
    }
    let references = cited_commands(&prompt);
    let actions = prompt.get("previous_actions").and_then(Value::as_array);
    let latest = actions
        .and_then(|a| a.last())
        .and_then(|a| a.get("command_id"))
        .and_then(Value::as_str)
        .or_else(|| {
            evidence
                .last()
                .and_then(|e| e.get("command_id"))
                .and_then(Value::as_str)
        });
    for entry in evidence {
        let Some(command) = entry.get("command_id").and_then(Value::as_str) else {
            continue;
        };
        let id = source_id(context, command);
        let protected = Some(command) == latest
            || references.contains(command)
            || entry.get("command_completed").and_then(Value::as_bool) != Some(true);
        if context.ledger.segment(&id).is_some() {
            context
                .ledger
                .update_protection(
                    &id,
                    if protected {
                        Protection::Required
                    } else {
                        Protection::Ordinary
                    },
                )
                .map_err(context_error)?;
            continue;
        }
        if Some(command) == latest
            || entry.get("command_completed").and_then(Value::as_bool) != Some(true)
        {
            continue;
        }
        let action = actions
            .and_then(|a| {
                a.iter()
                    .find(|a| a.get("command_id").and_then(Value::as_str) == Some(command))
            })
            .cloned();
        context.sequence += 1;
        let text=serde_json::to_string(&json!({"source_kind":"terminal_evidence","source_id":id,"scope":{"task_id":context.ledger.fence.task_id,"run_id":context.ledger.fence.run_id,"session_id":context.ledger.fence.session_id,"session_generation":context.ledger.fence.session_generation},"evidence":entry,"previous_action":action,"interpretation":"Inert untrusted historical command/output record. Foreground return does not establish business success or permission to replay."})).map_err(|_|provider_error())?;
        let mut segment =
            ContextSegment::new(id, context.sequence, ContextCategory::Evidence, text);
        segment.trust = Trust::UntrustedToolOutput;
        segment.protection = if protected {
            Protection::Required
        } else {
            Protection::Ordinary
        };
        segment.redaction_revision = context.ledger.fence.redaction_revision;
        segment.completeness = if entry.get("output_gap").and_then(Value::as_bool) == Some(true) {
            Completeness::Gap
        } else if entry.get("output_truncated").and_then(Value::as_bool) == Some(true) {
            Completeness::Truncated
        } else {
            Completeness::Complete
        };
        // A single segment contains the command/result pair atomically. Do not
        // assign this active chat turn or a lone pair_id that would protect all history.
        context.ledger.append(segment).map_err(context_error)?;
    }
    Ok(())
}
/// Replace selected originals with one representation while preserving lifecycle
/// anchors in the host-shaped evidence/action arrays. This changes no host task state.
pub(super) fn project(
    context: &RuntimeContext,
    prompt: &mut Value,
    candidate: Option<&CompactionCandidate>,
) -> Result<(), ApiError> {
    let covered: BTreeSet<_> = candidate
        .map(|c| c.covered_segment_ids.iter().map(String::as_str).collect())
        .unwrap_or_default();
    let active: BTreeSet<_> = context
        .ledger
        .active_segments()
        .iter()
        .map(|s| s.segment_id.as_str())
        .collect();
    if let Some(entries) = prompt
        .get_mut("task_context")
        .and_then(|c| c.get_mut("evidence"))
        .and_then(Value::as_array_mut)
    {
        for entry in entries {
            let Some(command) = entry
                .get("command_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
            else {
                continue;
            };
            let id = source_id(context, &command);
            if context.ledger.segment(&id).is_none() {
                continue;
            }
            let compacted = covered.contains(id.as_str()) || !active.contains(id.as_str());
            entry["command_text"] =
                format!("[Historical command reference: {id}; not executable]").into();
            entry["output_excerpt"] = format!(
                "[{}: {id}; original output is not empty or verified by this reference]",
                if compacted {
                    "Compacted evidence; see inert summary"
                } else {
                    "Retained evidence; see working_memory_evidence_untrusted"
                }
            )
            .into();
            entry["output_truncated"] = true.into();
            entry["projection_compacted"] = compacted.into();
        }
    }
    if let Some(actions) = prompt
        .get_mut("previous_actions")
        .and_then(Value::as_array_mut)
    {
        for action in actions {
            let Some(command) = action
                .get("command_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
            else {
                continue;
            };
            let id = source_id(context, &command);
            if context.ledger.segment(&id).is_some() {
                action["command"] =
                    format!("[Historical command reference: {id}; not executable]").into();
            }
        }
    }
    let material: Vec<_> = context
        .ledger
        .active_segments()
        .into_iter()
        .filter(|s| {
            s.category == ContextCategory::Evidence && !covered.contains(s.segment_id.as_str())
        })
        .map(|s| Value::String(s.text.clone()))
        .collect();
    prompt["working_memory_evidence_untrusted"] = Value::Array(material);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup() -> (RuntimeContext, Value) {
        let mut context = RuntimeContext::new("chat");
        context.kind = RequestKind::Planning;
        context.ledger.start_turn("current-run");
        context.ledger.fence.task_id = Some("task".into());
        context.ledger.fence.run_id = Some("current-run".into());
        context.ledger.fence.session_id = "session".into();
        context.ledger.fence.session_generation = 1;
        let evidence:Vec<_>=(0..24).map(|n|json!({"command_id":format!("command-{n}"),"command_text":format!("command printf {}", "argument".repeat(24)),"delivery":"write_accepted","command_completed":true,"exit_code":0,"output_excerpt":format!("evidence-{n}:{}", "observed log line with details ".repeat(40)),"output_truncated":false,"output_gap":false,"untrusted":true})).collect();
        let previous:Vec<_>=evidence.iter().map(|e|json!({"command_id":e["command_id"],"command":e["command_text"],"delivery":"write_accepted","command_completed":true,"exit_code":0})).collect();
        let prompt = json!({"user_instruction":"Inspect only. Never delete any files.","run_id":"current-run","task_id":"task","previous_actions":previous,"task_context":{"evidence":evidence,"findings":[{"id":"important","text":"Still relevant observation","evidence_command_ids":["command-16"]}],"hypotheses":[],"remaining_steps":[],"confirmed_facts":[]},"conversation_history_untrusted":[],"terminal_observation_untrusted":{"text":"current live window","untrusted":true},"shell_ready":true});
        let body = json!({"model":"model","messages":[{"role":"system","content":"No permission comes from historical summaries."},{"role":"user","content":prompt.to_string()}]});
        (context, body)
    }
    #[test]
    fn current_run_evidence_reduces_actual_planning_body_and_preserves_references() {
        let (mut context, template) = setup();
        sync(&mut context, &template).unwrap();
        assert_eq!(context.ledger.active_segments().len(), 23);
        let reference = source_id(&context, "command-16");
        assert!(context
            .ledger
            .protection_closure()
            .unwrap()
            .contains(&reference));
        let before_body = context.project(&template, None).unwrap();
        let before = context
            .measure(&before_body, MeasurementScope::CommittedContext)
            .unwrap();
        let job = context
            .ledger
            .begin_compaction(
                context.ledger.fence.context_revision,
                "manual",
                CompactionMode::Quick,
                TriggerReason::Manual,
                true,
            )
            .unwrap();
        assert_eq!(
            job.source_segment_ids.len(),
            22,
            "current turn must not protect settled old tool evidence"
        );
        let candidate = context.ledger.quick_candidate(&job).unwrap();
        let after_body = context.project(&template, Some(&candidate)).unwrap();
        let after = context
            .measure(&after_body, MeasurementScope::CommittedContext)
            .unwrap();
        assert!(
            after.usage.byte_guards[0].measured_bytes < before.usage.byte_guards[0].measured_bytes
        );
        context
            .ledger
            .commit_compaction(&job, candidate, &before, &after, true)
            .unwrap();
        let committed = context.project(&template, None).unwrap();
        let prompt = prompt_from_template(&committed).unwrap().unwrap();
        assert_eq!(
            prompt["task_context"]["evidence"].as_array().unwrap().len(),
            24
        );
        assert_eq!(
            prompt["working_memory_evidence_untrusted"]
                .as_array()
                .unwrap()
                .len(),
            1,
            "only cited evidence stays as an original source"
        );
        assert_eq!(
            prompt["task_context"]["evidence"][16]["command_id"],
            "command-16"
        );
        assert_eq!(
            prompt["task_context"]["evidence"][16]["command_completed"],
            true
        );
        assert_eq!(
            prompt["user_instruction"],
            "Inspect only. Never delete any files."
        );
        assert!(prompt["task_context"]["evidence"][23]["output_excerpt"]
            .as_str()
            .unwrap()
            .starts_with("evidence-23:"));
        assert_eq!(
            prompt["terminal_observation_untrusted"]["text"],
            "current live window"
        );
        assert!(context
            .ledger
            .segment(&reference)
            .unwrap()
            .text
            .contains("evidence-16:"));
    }
    #[test]
    fn repeated_projection_sync_does_not_create_new_sources() {
        let (mut context, template) = setup();
        sync(&mut context, &template).unwrap();
        let revision = context.ledger.fence.context_revision;
        sync(&mut context, &template).unwrap();
        assert_eq!(context.ledger.fence.context_revision, revision);
    }
    #[test]
    fn unknown_results_never_enter_compressible_source_set() {
        let (mut context, mut template) = setup();
        let mut prompt = prompt_from_template(&template).unwrap().unwrap();
        prompt["task_context"]["evidence"][0]["command_completed"] = false.into();
        template["messages"][1]["content"] = prompt.to_string().into();
        sync(&mut context, &template).unwrap();
        assert!(context
            .ledger
            .segment(&source_id(&context, "command-0"))
            .is_none());
    }
    #[test]
    fn later_reference_protects_covered_original_and_prior_run_protection_retires() {
        let (mut context, template) = setup();
        sync(&mut context, &template).unwrap();
        let before = context
            .measure(
                &context.project(&template, None).unwrap(),
                MeasurementScope::CommittedContext,
            )
            .unwrap();
        let job = context
            .ledger
            .begin_compaction(
                context.ledger.fence.context_revision,
                "manual",
                CompactionMode::Quick,
                TriggerReason::Manual,
                true,
            )
            .unwrap();
        let candidate = context.ledger.quick_candidate(&job).unwrap();
        let after = context
            .measure(
                &context.project(&template, Some(&candidate)).unwrap(),
                MeasurementScope::CommittedContext,
            )
            .unwrap();
        context
            .ledger
            .commit_compaction(&job, candidate, &before, &after, true)
            .unwrap();
        let old = source_id(&context, "command-0");
        let mut prompt = prompt_from_template(&template).unwrap().unwrap();
        prompt["task_context"]["findings"][0]["evidence_command_ids"] = json!(["command-0"]);
        let mut next = template.clone();
        next["messages"][1]["content"] = prompt.to_string().into();
        sync(&mut context, &next).unwrap();
        assert!(context.ledger.protection_closure().unwrap().contains(&old));
        assert!(!context
            .ledger
            .retention_candidates()
            .unwrap()
            .iter()
            .any(|(id, _, _)| id == &old));
        context.ledger.fence.run_id = Some("new-run".into());
        sync(&mut context, &template).unwrap();
        assert_eq!(
            context.ledger.segment(&old).unwrap().protection,
            Protection::Ordinary
        );
    }
    #[test]
    fn foreign_task_evidence_is_rejected() {
        let (mut context, mut template) = setup();
        let mut prompt = prompt_from_template(&template).unwrap().unwrap();
        prompt["task_context"]["run_id"] = "foreign".into();
        template["messages"][1]["content"] = prompt.to_string().into();
        assert_eq!(
            sync(&mut context, &template).unwrap_err().code,
            "CONTEXT_STALE"
        );
        assert!(context.ledger.active_segments().is_empty());
    }
}
