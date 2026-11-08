//! Minimized immutable request evidence. Raw transcripts and paths are never storage types.
use crate::capture_usage::UsageEvidence;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UsageRecord {
    pub source_adapter: String,
    pub source_record_id: String,
    pub source_turn_id: Option<String>,
    pub model: Option<String>,
    pub model_provenance: String,
    pub counter_semantics: String,
    pub input_basis: String,
    pub input_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub cache_write_input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub reasoning_output_tokens: Option<u64>,
}
fn opaque(value: &str) -> bool {
    crate::capture::identifier(value).is_ok()
        && ![
            "kto_",
            "sk-",
            "sk_",
            "akia",
            "asia",
            "ghp_",
            "gho_",
            "ghu_",
            "ghs_",
            "ghr_",
            "github_pat_",
            "xoxb-",
            "xoxp-",
        ]
        .iter()
        .any(|p| value.to_ascii_lowercase().starts_with(p))
}
impl UsageRecord {
    pub fn valid(&self, source: &str, version: &str) -> bool {
        let claude = source == "claude-code" && version == "2.1.319";
        let codex = source == "codex" && version == "0.167.0";
        (claude || codex)
            && self.source_adapter
                == if claude {
                    "CLAUDE_JSONL_2_1_319_V1"
                } else {
                    "CODEX_ROLLOUT_0_167_0_V1"
                }
            && self.input_basis
                == if claude {
                    "CLAUDE_UNCACHED_INPUT"
                } else {
                    "CODEX_TOTAL_INPUT"
                }
            && self.counter_semantics == "REQUEST_DELTA"
            && opaque(&self.source_record_id)
            && self.source_turn_id.as_deref().is_none_or(opaque)
            && self.model.as_deref().is_none_or(opaque)
            && matches!(
                self.model_provenance.as_str(),
                "RESPONSE_MODEL" | "TURN_CONFIG" | "UNKNOWN"
            )
            && (self.model.is_none() == (self.model_provenance == "UNKNOWN"))
            && if claude {
                self.model_provenance != "TURN_CONFIG"
            } else {
                self.source_turn_id.is_some() && self.model_provenance != "RESPONSE_MODEL"
            }
            && (self.input_tokens.is_some() || self.output_tokens.is_some())
            && [
                self.input_tokens,
                self.cached_input_tokens,
                self.cache_write_input_tokens,
                self.output_tokens,
                self.reasoning_output_tokens,
            ]
            .into_iter()
            .flatten()
            .all(|v| v <= 1_000_000_000_000)
            && !self
                .reasoning_output_tokens
                .zip(self.output_tokens)
                .is_some_and(|(r, o)| r > o)
            && (claude
                || self.input_tokens.is_none_or(|i| {
                    self.cached_input_tokens.unwrap_or(0)
                        + self.cache_write_input_tokens.unwrap_or(0)
                        <= i
                }))
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageLedger {
    pub records: Vec<UsageRecord>,
    pub record_revisions: Vec<i64>,
    pub queued: usize,
    pub incomplete: bool,
    pub held: bool,
    #[serde(default)]
    pub consent_epoch: u64,
    #[serde(default)]
    pub baseline_epoch: Option<u64>,
    #[serde(default)]
    pub excluded_request_ids: std::collections::BTreeSet<String>,
}
impl UsageLedger {
    pub fn baseline_ready(&self) -> bool {
        self.baseline_epoch == Some(self.consent_epoch)
    }
    pub fn require_baseline(&mut self) {
        self.consent_epoch = self.consent_epoch.saturating_add(1);
        self.baseline_epoch = None;
        self.incomplete = true;
    }
    /// Persist identifiers only. Prior immutable accepted records remain eligible
    /// for conflict checking; no excluded record's counters enter this ledger.
    pub fn establish_baseline(&mut self, ids: std::collections::BTreeSet<String>) -> bool {
        self.baseline_epoch = None;
        if ids.len() > 4096 || ids.iter().any(|id| !opaque(id)) {
            self.incomplete = true;
            return false;
        }
        let mut excluded = self.excluded_request_ids.clone();
        excluded.extend(
            ids.into_iter()
                .filter(|id| !self.records.iter().any(|old| old.source_record_id == *id)),
        );
        if excluded.len() > 16384 {
            self.incomplete = true;
            return false;
        }
        self.incomplete |= !excluded.is_empty();
        self.excluded_request_ids = excluded;
        self.baseline_epoch = Some(self.consent_epoch);
        true
    }
    /// Validate the whole snapshot before changing any minimized records.
    pub fn observe(&mut self, evidence: UsageEvidence, source: &str, version: &str, session: &str) {
        if !self.baseline_ready() {
            self.incomplete = true;
            return;
        }
        if evidence.source_session_id != session
            || evidence.harness_version != version
            || evidence.coverage != "PARTIAL"
        {
            self.incomplete = true;
            return;
        }
        if evidence.limitations.iter().any(|reason| {
            matches!(
                *reason,
                "CONFLICTING_REQUEST_RECORDS" | "UNKNOWN_REQUEST_FINALITY"
            )
        }) {
            self.held = true;
            self.incomplete = true;
            return;
        }
        let candidate = serde_json::to_value(&evidence.requests)
            .ok()
            .and_then(|v| serde_json::from_value::<Vec<UsageRecord>>(v).ok());
        let Some(candidate) = candidate.filter(|v| v.iter().all(|r| r.valid(source, version)))
        else {
            self.incomplete = true;
            return;
        };
        let candidate: Vec<_> = candidate
            .into_iter()
            .filter(|r| {
                !self.excluded_request_ids.contains(&r.source_record_id)
                    || self
                        .records
                        .iter()
                        .any(|old| old.source_record_id == r.source_record_id)
            })
            .collect();
        if candidate.iter().any(|r| {
            self.records
                .iter()
                .any(|old| old.source_record_id == r.source_record_id && old != r)
        }) {
            self.held = true;
            self.incomplete = true;
            return;
        }
        self.incomplete |= !evidence.limitations.is_empty();
        for record in candidate {
            if !self
                .records
                .iter()
                .any(|old| old.source_record_id == record.source_record_id)
            {
                self.records.push(record);
                self.record_revisions.push(0);
            }
        }
    }
    pub fn payload(&self, start: usize, end: usize) -> serde_json::Value {
        if start == end {
            return serde_json::json!({"status":"UNAVAILABLE","reason":"NO_QUALIFIED_USAGE_EVIDENCE","models":[]});
        }
        serde_json::json!({"schema_version":1,"status":"PARTIAL","reason":if self.incomplete {"INCOMPLETE_USAGE_EVIDENCE"} else {"OBSERVED_REQUEST_RECORDS"},"records":self.records[start..end]})
    }
    pub fn validate_payload(
        &self,
        raw: &serde_json::Value,
        source: &str,
        version: &str,
        revision: i64,
    ) -> bool {
        if raw
            == &serde_json::json!({"status":"UNAVAILABLE","reason":"NO_QUALIFIED_USAGE_EVIDENCE","models":[]})
        {
            return true;
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Partial {
            schema_version: u8,
            status: String,
            reason: String,
            records: Vec<UsageRecord>,
        }
        let Ok(p) = serde_json::from_value::<Partial>(raw.clone()) else {
            return false;
        };
        let mut ids = std::collections::BTreeSet::new();
        p.schema_version == 1
            && p.status == "PARTIAL"
            && matches!(
                p.reason.as_str(),
                "OBSERVED_REQUEST_RECORDS" | "INCOMPLETE_USAGE_EVIDENCE"
            )
            && !p.records.is_empty()
            && p.records.len() <= 25
            && p.records.iter().all(|r| {
                r.valid(source, version)
                    && ids.insert(&r.source_record_id)
                    && self
                        .records
                        .iter()
                        .position(|old| old == r)
                        .is_some_and(|i| self.record_revisions.get(i) == Some(&revision))
            })
    }
}
