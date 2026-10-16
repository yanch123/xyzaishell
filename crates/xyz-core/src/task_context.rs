//! Bounded, inert context for one task run and one SSH connection generation.
//!
//! Only host lifecycle observations become `confirmed_facts`. Findings and step
//! states are model interpretations, even when they cite genuine command IDs.
//! None of this data is an execution capability, approval, or proof of success.
use crate::{
    execution::PreparedAction,
    observation::{Observation, ObservationBuffer},
    ssh::{ApiError, SessionRef},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

// Independent host evidence-store budget, not a model request allowance.
// A run has at most 256 planning actions; every command can retain an anchor.
// The actual request selects/compacts these sources through tasks_evidence.
pub const MAX_CONTEXT_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_EVIDENCE: usize = 256;
const MAX_FINDINGS: usize = 8;
const MAX_STEPS: usize = 12;
const MAX_REFERENCES: usize = 4;
const MAX_OUTPUT_BYTES: usize = 512;
const MAX_COMMAND_BYTES: usize = 256;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TaskFinding {
    pub id: String,
    pub text: String,
    pub evidence_command_ids: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    InProgress,
    Blocked,
    Completed,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RemainingStep {
    pub id: String,
    pub text: String,
    pub status: StepStatus,
    pub evidence_command_ids: Vec<String>,
}
/// The complete model-owned portion; replacement is atomic after validation.
/// Host identity, evidence, and confirmed facts are deliberately not accepted.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModelTaskContext {
    pub summary: String,
    pub findings: Vec<TaskFinding>,
    pub hypotheses: Vec<TaskFinding>,
    pub remaining_steps: Vec<RemainingStep>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HostFactKind {
    ForegroundReturned,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfirmedFact {
    pub command_id: String,
    pub kind: HostFactKind,
    pub exit_code: i32,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandEvidence {
    pub command_id: String,
    /// A bounded display excerpt only. It must never be used for replay.
    pub command_text: String,
    pub delivery: String,
    pub command_completed: bool,
    pub exit_code: Option<i32>,
    /// Output observed between model rounds, not exact command attribution.
    /// Other foreground/background terminal activity may be interleaved.
    pub output_excerpt: String,
    pub output_truncated: bool,
    pub output_gap: bool,
    pub untrusted: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskContextSnapshot {
    pub task_id: String,
    pub run_id: String,
    pub session: SessionRef,
    pub summary: String,
    pub confirmed_facts: Vec<ConfirmedFact>,
    /// Model-extracted findings, not independently verified by the host.
    pub findings: Vec<TaskFinding>,
    pub hypotheses: Vec<TaskFinding>,
    /// All statuses are model-reported and confer no host authority.
    pub remaining_steps: Vec<RemainingStep>,
    pub evidence: Vec<CommandEvidence>,
    pub truncated: bool,
}

fn invalid() -> ApiError {
    ApiError::new("TASK_CONTEXT_INVALID", "The task summary is malformed, too large, or cites evidence outside this run. No command was prepared.")
}
pub(crate) fn excerpt(text: &str, maximum: usize) -> String {
    let mut end = text.len().min(maximum);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}
fn valid_text(text: &str, maximum: usize, allow_empty: bool) -> bool {
    text.len() <= maximum
        && (allow_empty || !text.trim().is_empty())
        && !text.chars().any(char::is_control)
}
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 48
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
}

impl TaskContextSnapshot {
    pub(crate) fn new(task_id: String, run_id: String, session: SessionRef) -> Self {
        Self {
            task_id,
            run_id,
            session,
            summary: String::new(),
            confirmed_facts: vec![],
            findings: vec![],
            hypotheses: vec![],
            remaining_steps: vec![],
            evidence: vec![],
            truncated: false,
        }
    }
    pub(crate) fn matches(&self, task_id: &str, run_id: &str, session: &SessionRef) -> bool {
        self.task_id == task_id && self.run_id == run_id && &self.session == session
    }
    pub(crate) fn observe_action_with_shared(
        &mut self,
        action: &PreparedAction,
        run_redactor: &ObservationBuffer,
        shared_redactor: &ObservationBuffer,
    ) {
        let mut safe = action.clone();
        safe.command_text =
            shared_redactor.redact_prose(&run_redactor.redact_prose(&action.command_text));
        self.observe_action(&safe, run_redactor);
        self.redact(shared_redactor);
    }
    /// Called only with host-owned actions. Secret-input actions are never
    /// retained as evidence and cannot be cited by model findings.
    pub(crate) fn observe_action(&mut self, action: &PreparedAction, redactor: &ObservationBuffer) {
        if action.kind != "command" {
            return;
        }
        let index = self
            .evidence
            .iter()
            .position(|e| e.command_id == action.action_id);
        let index = match index {
            Some(index) => index,
            None if self.evidence.len() < MAX_EVIDENCE => {
                self.truncated |= action.command_text.len() > MAX_COMMAND_BYTES;
                self.evidence.push(CommandEvidence {
                    command_id: action.action_id.clone(),
                    command_text: excerpt(
                        &redactor.redact_prose(&action.command_text),
                        MAX_COMMAND_BYTES,
                    ),
                    delivery: action.delivery.clone(),
                    command_completed: false,
                    exit_code: None,
                    output_excerpt: String::new(),
                    output_truncated: false,
                    output_gap: false,
                    untrusted: true,
                });
                self.evidence.len() - 1
            }
            None => {
                self.truncated = true;
                return;
            }
        };
        let evidence = &mut self.evidence[index];
        evidence.delivery = action.delivery.clone();
        evidence.command_completed = action.command_completed;
        evidence.exit_code = action.exit_code;
        if action.command_completed {
            if let Some(exit_code) = action.exit_code {
                if !self
                    .confirmed_facts
                    .iter()
                    .any(|fact| fact.command_id == action.action_id)
                {
                    self.confirmed_facts.push(ConfirmedFact {
                        command_id: action.action_id.clone(),
                        kind: HostFactKind::ForegroundReturned,
                        exit_code,
                    });
                }
            }
        }
        self.redact(redactor);
    }
    pub(crate) fn retain_output(
        &mut self,
        command_id: &str,
        output: &Observation,
        redactor: &ObservationBuffer,
    ) {
        let Some(evidence) = self
            .evidence
            .iter_mut()
            .find(|e| e.command_id == command_id)
        else {
            return;
        };
        if evidence.delivery == "not_sent" || evidence.delivery == "queued" {
            return;
        }
        let safe = redactor.redact_prose(&output.text);
        if !safe.is_empty() {
            let combined = format!("{}{}", evidence.output_excerpt, safe);
            evidence.output_excerpt = excerpt(&combined, MAX_OUTPUT_BYTES);
            evidence.output_truncated |= combined.len() > MAX_OUTPUT_BYTES;
        }
        evidence.output_truncated |= output.truncated;
        evidence.output_gap |= output.gap;
        self.truncated |= evidence.output_truncated;
        self.redact(redactor);
    }
    pub(crate) fn apply_model(
        &mut self,
        mut update: ModelTaskContext,
        redactor: &ObservationBuffer,
    ) -> Result<(), ApiError> {
        if !valid_text(&update.summary, 1024, true)
            || update.findings.len() > MAX_FINDINGS
            || update.hypotheses.len() > MAX_FINDINGS
            || update.remaining_steps.len() > MAX_STEPS
        {
            return Err(invalid());
        }
        let references = |ids: &[String], needs_output: bool, needs_completion: bool| {
            let unique: BTreeSet<_> = ids.iter().collect();
            ids.len() <= MAX_REFERENCES
                && unique.len() == ids.len()
                && ids.iter().all(|id| {
                    self.evidence.iter().any(|e| {
                        e.command_id == *id
                            && (!needs_output || !e.output_excerpt.is_empty())
                            && (!needs_completion || e.command_completed)
                    })
                })
        };
        for (items, needs_output) in [(&update.findings, true), (&update.hypotheses, false)] {
            let mut ids = BTreeSet::new();
            for finding in items {
                if !valid_id(&finding.id)
                    || !ids.insert(&finding.id)
                    || !valid_text(&finding.text, 384, false)
                    || (needs_output && finding.evidence_command_ids.is_empty())
                    || !references(&finding.evidence_command_ids, needs_output, false)
                {
                    return Err(invalid());
                }
            }
        }
        let mut ids = BTreeSet::new();
        for step in &update.remaining_steps {
            let completed = step.status == StepStatus::Completed;
            if !valid_id(&step.id)
                || !ids.insert(&step.id)
                || !valid_text(&step.text, 256, false)
                || (completed && step.evidence_command_ids.is_empty())
                || !references(&step.evidence_command_ids, false, completed)
            {
                return Err(invalid());
            }
        }
        // IDs are structural keys, so reject protected material instead of
        // transforming IDs and silently breaking their links.
        if update
            .findings
            .iter()
            .chain(&update.hypotheses)
            .any(|f| redactor.redact_text(&f.id) != f.id)
            || update
                .remaining_steps
                .iter()
                .any(|s| redactor.redact_text(&s.id) != s.id)
        {
            return Err(invalid());
        }
        update.summary = redactor.redact_prose(&update.summary);
        for item in update.findings.iter_mut().chain(&mut update.hypotheses) {
            item.text = redactor.redact_prose(&item.text);
        }
        for step in &mut update.remaining_steps {
            step.text = redactor.redact_prose(&step.text);
        }
        let mut candidate = self.clone();
        candidate.summary = update.summary;
        candidate.findings = update.findings;
        candidate.hypotheses = update.hypotheses;
        candidate.remaining_steps = update.remaining_steps;
        candidate.redact(redactor);
        if serde_json::to_vec(&candidate).map_err(|_| invalid())?.len() > MAX_CONTEXT_BYTES {
            return Err(invalid());
        }
        *self = candidate;
        Ok(())
    }
    pub(crate) fn redact(&mut self, redactor: &ObservationBuffer) {
        // IDs are opaque keys. Substitution markers may themselves contain a
        // newly known short secret, so repeatedly substituting into an ID can
        // grow it without bound. Drop conflicting model entries instead.
        let before = self.findings.len() + self.hypotheses.len() + self.remaining_steps.len();
        self.findings
            .retain(|f| valid_id(&f.id) && redactor.redact_text(&f.id) == f.id);
        self.hypotheses
            .retain(|f| valid_id(&f.id) && redactor.redact_text(&f.id) == f.id);
        self.remaining_steps
            .retain(|s| valid_id(&s.id) && redactor.redact_text(&s.id) == s.id);
        self.truncated |=
            before != self.findings.len() + self.hypotheses.len() + self.remaining_steps.len();
        self.summary = excerpt(&redactor.redact_prose(&self.summary), 1024);
        for finding in self.findings.iter_mut().chain(&mut self.hypotheses) {
            finding.text = excerpt(&redactor.redact_prose(&finding.text), 384);
        }
        for step in &mut self.remaining_steps {
            step.text = excerpt(&redactor.redact_prose(&step.text), 256);
        }
        for evidence in &mut self.evidence {
            evidence.command_text = excerpt(
                &redactor.redact_prose(&evidence.command_text),
                MAX_COMMAND_BYTES,
            );
            evidence.output_excerpt = excerpt(
                &redactor.redact_prose(&evidence.output_excerpt),
                MAX_OUTPUT_BYTES,
            );
        }
        // Escaping and later redactor expansion count toward the total budget.
        // Excerpt exhaustion must never silently disable this bound.
        while serde_json::to_vec(self).map_or(true, |bytes| bytes.len() > MAX_CONTEXT_BYTES) {
            if let Some(evidence) = self
                .evidence
                .iter_mut()
                .filter(|e| !e.output_excerpt.is_empty() || !e.command_text.is_empty())
                .max_by_key(|e| e.output_excerpt.len() + e.command_text.len())
            {
                evidence.output_excerpt =
                    excerpt(&evidence.output_excerpt, evidence.output_excerpt.len() / 2);
                evidence.command_text =
                    excerpt(&evidence.command_text, evidence.command_text.len() / 2);
                evidence.output_truncated = true;
            } else if self.hypotheses.pop().is_some()
                || self.remaining_steps.pop().is_some()
                || self.findings.pop().is_some()
            {
                // Discard model notes before authoritative lifecycle metadata.
            } else if !self.summary.is_empty() {
                self.summary.clear();
            } else if let Some(evidence) = self.evidence.pop() {
                self.confirmed_facts
                    .retain(|fact| fact.command_id != evidence.command_id);
            } else if self.confirmed_facts.pop().is_none() {
                // Only bounded backend-generated task/run/session identity is
                // left; there is no model-controlled text in this branch.
                break;
            }
            self.truncated = true;
        }
    }
    /// History keeps only inert prose. No action IDs, executable command
    /// payloads, raw evidence output, or scoped runtime objects are copied.
    pub(crate) fn history_text(&self) -> String {
        let mut text = format!("Retained task summary (model-authored): {}\nHost-observed foreground returns: {}. These do not establish business success.\n", self.summary, self.confirmed_facts.len());
        for finding in &self.findings {
            text.push_str(&format!(
                "Finding (model-interpreted, not independently verified): {}\n",
                finding.text
            ));
        }
        for hypothesis in &self.hypotheses {
            text.push_str(&format!("Hypothesis (unverified): {}\n", hypothesis.text));
        }
        for step in &self.remaining_steps {
            text.push_str(&format!(
                "Step ({:?}, model-reported): {}\n",
                step.status, step.text
            ));
        }
        excerpt(&text, 8 * 1024)
    }
}

pub(crate) fn model_context_schema() -> serde_json::Value {
    let ids = serde_json::json!({"type":"array","maxItems":4,"uniqueItems":true,"items":{"type":"string","maxLength":48}});
    let finding = serde_json::json!({"type":"object","additionalProperties":false,"properties":{"id":{"type":"string","maxLength":48},"text":{"type":"string","maxLength":384},"evidence_command_ids":ids},"required":["id","text","evidence_command_ids"]});
    serde_json::json!({"type":"object","additionalProperties":false,"properties":{
        "summary":{"type":"string","maxLength":1024},
        "findings":{"type":"array","maxItems":8,"items":finding},
        "hypotheses":{"type":"array","maxItems":8,"items":finding},
        "remaining_steps":{"type":"array","maxItems":12,"items":{"type":"object","additionalProperties":false,"properties":{"id":{"type":"string","maxLength":48},"text":{"type":"string","maxLength":256},"status":{"type":"string","enum":["pending","in_progress","blocked","completed"]},"evidence_command_ids":ids},"required":["id","text","status","evidence_command_ids"]}}
    },"required":["summary","findings","hypotheses","remaining_steps"]})
}

#[cfg(test)]
#[path = "task_context_tests.rs"]
mod tests;
