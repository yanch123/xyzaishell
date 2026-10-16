//! Bounded, process-local provider-usage calibration. No persisted text or keys.
use super::{
    ContextBudgetConfig, ContextFence, PreparedContext, ProviderUsage,
    DEFAULT_CONTEXT_WINDOW_TOKENS, ESTIMATOR_VERSION,
};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default)]
pub struct UsageCalibration {
    entries: BTreeMap<(String, String, String), CalibrationEntry>,
    sequence: u64,
}
#[derive(Clone, Debug)]
struct CalibrationEntry {
    extra_margin: u64,
    updated: u64,
}
impl UsageCalibration {
    /// Usage may refine the safety margin only for its exact completed preparation.
    /// It never replaces the next request's estimate or fabricates category usage.
    pub fn observe(&mut self, prepared: &PreparedContext, usage: &ProviderUsage) -> bool {
        if usage.completeness != "reported"
            || usage.request_id.is_empty()
            || usage.model != prepared.fence.model
            || usage.projection_id != prepared.usage.projection_id
            || usage.config_revision != prepared.usage.config_revision
            || usage.request_kind != prepared.usage.request_kind
        {
            return false;
        }
        let (Some(actual), Some(estimated)) = (
            usage.input_tokens,
            prepared.usage.estimate.conservative_input_tokens,
        ) else {
            return false;
        };
        let extra = actual.saturating_sub(estimated);
        let key = (
            prepared.fence.authorized_destination.clone(),
            prepared.fence.model.clone(),
            prepared.usage.estimate.estimator_version.clone(),
        );
        self.sequence = self.sequence.saturating_add(1);
        if !self.entries.contains_key(&key) && self.entries.len() >= 64 {
            if let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.updated)
                .map(|(key, _)| key.clone())
            {
                self.entries.remove(&oldest);
            }
        }
        let entry = self.entries.entry(key).or_insert(CalibrationEntry {
            extra_margin: 0,
            updated: self.sequence,
        });
        entry.extra_margin = entry.extra_margin.max(extra);
        entry.updated = self.sequence;
        true
    }
    /// Recompute from the user's original config each time, never compound margins.
    pub fn apply(&self, config: &ContextBudgetConfig, fence: &ContextFence) -> ContextBudgetConfig {
        let mut result = config.clone();
        let key = (
            fence.authorized_destination.clone(),
            fence.model.clone(),
            ESTIMATOR_VERSION.to_owned(),
        );
        if let Some(entry) = self.entries.get(&key) {
            let w = config
                .context_window_override
                .or(config.verified_context_window)
                .unwrap_or(DEFAULT_CONTEXT_WINDOW_TOKENS);
            let base = config
                .safety_margin_override
                .unwrap_or(w.div_ceil(20).max(1024));
            result.safety_margin_override = Some(base.saturating_add(entry.extra_margin));
        }
        result
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{prepare_context, MeasurementScope, RequestKind};
    #[test]
    fn calibration_is_exact_scoped_and_unknown_is_not_zero() {
        let fence = ContextFence {
            model: "m".into(),
            authorized_destination: "https://example.com".into(),
            ..Default::default()
        };
        let config = ContextBudgetConfig::default();
        let prepared = prepare_context(
            "c",
            &fence,
            RequestKind::Conversation,
            MeasurementScope::CommittedContext,
            &[],
            32,
            &config,
        )
        .unwrap();
        let mut usage = ProviderUsage {
            request_id: "r".into(),
            request_kind: RequestKind::Conversation,
            projection_id: prepared.usage.projection_id.clone(),
            config_revision: prepared.usage.config_revision.clone(),
            model: "m".into(),
            received_at: "now".into(),
            input_tokens: None,
            output_tokens: None,
            cached_input_tokens: None,
            reasoning_tokens: None,
            completeness: "reported".into(),
        };
        let mut calibration = UsageCalibration::default();
        assert!(!calibration.observe(&prepared, &usage));
        usage.input_tokens = Some(116);
        usage.cached_input_tokens = Some(100);
        assert!(calibration.observe(&prepared, &usage));
        assert_eq!(
            calibration.apply(&config, &fence).safety_margin_override,
            Some(12900)
        );
        let mut other = fence.clone();
        other.model = "other".into();
        assert_eq!(
            calibration.apply(&config, &other).safety_margin_override,
            None
        );
        usage.projection_id = "stale".into();
        assert!(!calibration.observe(&prepared, &usage));
    }
}
