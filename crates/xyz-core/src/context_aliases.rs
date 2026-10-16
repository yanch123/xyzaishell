//! Per-job reference compression. Prose and serialized source text remain opaque.
use super::{CompactionJob, ContextError};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug)]
pub struct CompactionAliases {
    job_id: String,
    sources: Vec<String>,
    protected: BTreeSet<String>,
    encode: BTreeMap<String, String>,
    decode: BTreeMap<String, String>,
}
impl CompactionAliases {
    pub fn new(job: &CompactionJob) -> Result<Self, ContextError> {
        let mut encode = BTreeMap::new();
        let mut decode = BTreeMap::new();
        for (prefix, ids) in [
            ("s", job.source_segment_ids.clone()),
            ("p", job.protected_segment_ids.iter().cloned().collect()),
        ] {
            for (index, id) in ids.into_iter().enumerate() {
                if id.is_empty() || encode.contains_key(&id) {
                    return Err(invalid("Duplicate or empty source identity."));
                }
                let alias = format!("{prefix}{index}");
                encode.insert(id.clone(), alias.clone());
                decode.insert(alias, id);
            }
        }
        Ok(Self {
            job_id: job.job_id.clone(),
            sources: job.source_segment_ids.clone(),
            protected: job.protected_segment_ids.clone(),
            encode,
            decode,
        })
    }
    /// Use the original full job, including when encoding an individual chunk.
    pub fn encode(&self, job: &CompactionJob, value: &Value) -> Result<Value, ContextError> {
        self.transform(job, value, &self.encode)
    }
    /// Expand references before the normal strict summary schema/coverage validator.
    pub fn decode(&self, job: &CompactionJob, value: &Value) -> Result<Value, ContextError> {
        self.transform(job, value, &self.decode)
    }
    fn transform(
        &self,
        job: &CompactionJob,
        value: &Value,
        map: &BTreeMap<String, String>,
    ) -> Result<Value, ContextError> {
        if job.job_id != self.job_id
            || job.source_segment_ids != self.sources
            || job.protected_segment_ids != self.protected
        {
            return Err(invalid("Reference aliases belong to a different job."));
        }
        transform(value, map)
    }
}
fn invalid(message: &str) -> ContextError {
    ContextError::new("CONTEXT_INVALID_SUMMARY", message)
}
fn reference(value: &Value, map: &BTreeMap<String, String>) -> Result<Value, ContextError> {
    value
        .as_str()
        .and_then(|id| map.get(id))
        .cloned()
        .map(Value::String)
        .ok_or_else(|| invalid("Unknown or malformed source reference."))
}
fn transform(value: &Value, map: &BTreeMap<String, String>) -> Result<Value, ContextError> {
    match value {
        Value::Object(object) => {
            let mut output = serde_json::Map::new();
            for (key, value) in object {
                let mapped = match key.as_str() {
                    "source_id" => reference(value, map)?,
                    "source_ids" | "evidence_refs" | "omitted_ranges" | "source_manifest" => {
                        let values = value
                            .as_array()
                            .ok_or_else(|| invalid("Reference list must be an array."))?;
                        let mut seen = BTreeSet::new();
                        let mut refs = Vec::with_capacity(values.len());
                        for value in values {
                            let mapped = reference(value, map)?;
                            if !seen.insert(mapped.as_str().unwrap().to_owned()) {
                                return Err(invalid("Duplicate source reference."));
                            }
                            refs.push(mapped);
                        }
                        Value::Array(refs)
                    }
                    _ => transform(value, map)?,
                };
                output.insert(key.clone(), mapped);
            }
            Ok(Value::Object(output))
        }
        Value::Array(values) => values.iter().map(|v| transform(v, map)).collect(),
        _ => Ok(value.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{CompactionMode, ContextFence, TriggerReason};
    use serde_json::json;
    fn job() -> CompactionJob {
        CompactionJob {
            job_id: "job-a".into(),
            client_request_id: "request".into(),
            mode: CompactionMode::Llm,
            reason: TriggerReason::Manual,
            fence: ContextFence::default(),
            source_segment_ids: (0..256)
                .map(|i| format!("evidence-{}-{i}", "a".repeat(128)))
                .collect(),
            protected_segment_ids: BTreeSet::from(["protected-user-goal".into()]),
            source_set_hash: "hash".into(),
            started_at: std::time::Instant::now(),
        }
    }
    #[test]
    fn full_coverage_is_bounded_and_round_trips() {
        let job = job();
        let aliases = CompactionAliases::new(&job).unwrap();
        let original = json!({"coverage":{"source_ids":job.source_segment_ids,"omitted_ranges":[job.source_segment_ids[2]]},"findings":[{"evidence_refs":["protected-user-goal"],"text":"s0 protected-user-goal"}],"summary_text":"s0","text":"{\"source_id\":\"untrusted\"}"});
        let encoded = aliases.encode(&job, &original).unwrap();
        assert!(encoded["coverage"].to_string().len() < 1800);
        assert_eq!(encoded["summary_text"], "s0");
        assert_eq!(encoded["text"], original["text"]);
        assert_eq!(aliases.decode(&job, &encoded).unwrap(), original);
    }
    #[test]
    fn unknown_duplicate_and_cross_job_references_fail() {
        let job = job();
        let aliases = CompactionAliases::new(&job).unwrap();
        for value in [
            json!({"source_ids":["s999"]}),
            json!({"source_ids":["s0","s0"]}),
            json!({"source_id":12}),
            json!({"evidence_refs":[job.source_segment_ids[0]]}),
        ] {
            assert!(aliases.decode(&job, &value).is_err());
        }
        let mut other = job.clone();
        other.job_id = "job-b".into();
        assert!(aliases
            .decode(&other, &json!({"source_ids":["s0"]}))
            .is_err());
        other.job_id = job.job_id.clone();
        other.source_segment_ids[0] = "different-source".into();
        assert!(aliases
            .decode(&other, &json!({"source_ids":["s0"]}))
            .is_err());
    }
    #[test]
    fn duplicate_input_id_is_rejected() {
        let mut job = job();
        job.source_segment_ids
            .push(job.source_segment_ids[0].clone());
        assert!(CompactionAliases::new(&job).is_err());
    }
}
