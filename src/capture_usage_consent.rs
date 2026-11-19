//! Path-free persisted numeric policy. Functional profile paths belong only in
//! the separately approved private device record, never this SQLite policy.
use serde::{Deserialize, Serialize};

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConsentPolicy {
    pub policy_version: u8,
    pub binding_id: String,
    pub binding_version: i64,
    pub source_namespace: String,
    pub profile_id: String,
    pub profile_digest: String,
    pub epoch: u64,
    pub enabled: bool,
    #[serde(default)]
    pub unavailable_reason: Option<String>,
}

impl ConsentPolicy {
    pub(crate) fn matches(&self, binding: &crate::capture::Binding, namespace: &str) -> bool {
        let digest = |s: &str| {
            s.len() == 64
                && s.bytes()
                    .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
        };
        self.policy_version == 1
            && self.binding_id == binding.id
            && self.binding_version == binding.version
            && self.source_namespace == namespace
            && matches!(namespace, "claude-code" | "codex")
            && digest(&self.profile_id)
            && digest(&self.profile_digest)
            && self.epoch > 0
            && self.unavailable_reason.as_deref().is_none_or(known_reason)
    }
}

pub(crate) fn known_reason(reason: &str) -> bool {
    matches!(
        reason,
        "NUMERIC_CAPTURE_DISABLED"
            | "NUMERIC_SOURCE_UNQUALIFIED"
            | "PROFILE_UNAVAILABLE"
            | "PROFILE_CHANGED"
            | "HARNESS_CHANGED"
            | "REGISTRATION_CHANGED"
            | "RUNTIME_CHANGED"
            | "UNSUPPORTED_SOURCE"
            | "UNSUPPORTED_HARNESS_VERSION"
            | "HARNESS_ORIGIN_UNAVAILABLE"
            | "LIVE_GUARD_REQUIRED"
            | "ACTIVE_SOURCE_UNAVAILABLE"
            | "MISSING_QUALIFIED_PROMPT"
            | "SOURCE_CHANGED"
            | "SOURCE_PARSE_UNAVAILABLE"
            | "OBSERVED_REQUEST_RECORDS"
    )
}

pub(crate) fn status(namespace: &str, enabled: bool, reason: &str) -> serde_json::Value {
    serde_json::json!({"source_namespace":namespace,"enabled":enabled,
        "status":"UNAVAILABLE","reason":reason})
}
