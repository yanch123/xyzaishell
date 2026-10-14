//! Optional advisory extension only. No Jev client, network request, credential
//! requirement, authorization, or terminal handle is provided in this milestone.
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DecisionAssessment {
    AllowCandidate,
    RequireApproval,
    Deny,
    Unknown,
}
pub struct DecisionInput<'a> {
    pub sanitized_action: &'a str,
    pub sanitized_observation: &'a str,
    pub truncated: bool,
}
pub trait DecisionProvider: Send + Sync {
    fn assess(&self, input: &DecisionInput<'_>) -> DecisionAssessment;
}
#[derive(Default)]
pub struct UnknownDecisionProvider;
impl DecisionProvider for UnknownDecisionProvider {
    fn assess(&self, _: &DecisionInput<'_>) -> DecisionAssessment {
        DecisionAssessment::Unknown
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disabled_is_unknown() {
        assert_eq!(
            UnknownDecisionProvider.assess(&DecisionInput {
                sanitized_action: "pwd",
                sanitized_observation: "",
                truncated: false
            }),
            DecisionAssessment::Unknown
        );
    }
}
