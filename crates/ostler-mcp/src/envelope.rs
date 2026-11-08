//! The structured outcome of a tool call (PLAN-LEVEL-100 step 1.1).
//!
//! Every failed call carries an [`Envelope`] in `structuredContent`, so a client can act on the domain
//! outcome without parsing the prose. The text content is unchanged for people reading the transcript.

use serde::Serialize;
use serde_json::Value;

/// Version tag of the envelope shape. Bump it when a field changes meaning.
pub const SCHEMA: &str = "ostler.result/1";

/// What happened, in domain terms. A tool can succeed at the protocol level and still be `Rejected`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Completed,
    Rejected,
    Blocked,
    NeedsInput,
    Uncertain,
    Failed,
}

/// Whether the caller may repeat the same call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryPolicy {
    /// Repeating is harmless (reads, navigation that failed before it started).
    Safe,
    /// Repeat only after a fresh snapshot, because refs changed.
    AfterSnapshot,
    /// The effect may have happened: check the page before doing anything else.
    ReconcileFirst,
    /// Do not repeat this call.
    Never,
    /// Repeat only after the page state has changed.
    AfterChange,
}

/// The structured result attached to a failed tool call.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Envelope {
    pub schema: &'static str,
    pub status: Status,
    /// Stable machine-readable code, e.g. `STALE_REF`. `None` when the cause is not classified.
    pub code: Option<&'static str>,
    /// What the call did to the page, when known.
    pub effect: Option<Value>,
    /// Post-conditions checked after the call (filled in by later steps).
    pub checks: Vec<Value>,
    /// Evidence records this result refers to (filled in by later steps).
    pub evidence_ids: Vec<String>,
    pub retry_policy: RetryPolicy,
    /// What the caller should do next, when there is a clear step.
    pub next_action: Option<Value>,
}

impl Envelope {
    pub fn new(status: Status, code: Option<&'static str>, retry_policy: RetryPolicy) -> Self {
        Self {
            schema: SCHEMA,
            status,
            code,
            effect: None,
            checks: Vec::new(),
            evidence_ids: Vec::new(),
            retry_policy,
            next_action: None,
        }
    }

    /// The JSON placed under `structuredContent`.
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn serialises_snake_case_and_carries_the_schema_tag() {
        let e = Envelope::new(
            Status::Uncertain,
            Some("ACTION_UNCERTAIN"),
            RetryPolicy::ReconcileFirst,
        );
        assert_eq!(
            e.to_value(),
            json!({
                "schema": "ostler.result/1",
                "status": "uncertain",
                "code": "ACTION_UNCERTAIN",
                "effect": null,
                "checks": [],
                "evidence_ids": [],
                "retry_policy": "reconcile_first",
                "next_action": null
            })
        );
    }

    #[test]
    fn unclassified_code_is_null_not_missing() {
        let e = Envelope::new(Status::Failed, None, RetryPolicy::Never);
        assert_eq!(e.to_value()["code"], Value::Null);
    }
}
