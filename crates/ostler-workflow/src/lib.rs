//! Pure workflow logic for Ostler: no browser, no network.
//!
//! * [`expect`]: postconditions an action can carry (`EXPECT OK` / `EXPECT FAILED`).
//! * [`task`]: task contracts, verdicts and the false-success building block.
//! * [`trace`]: redacted append-only JSONL trace and an OpenTelemetry-shaped export.
//! * [`risk`]: action risk classes and the per-domain policy.
//! * [`idempotency`]: double-submit guard.
//! * [`receipt`]: confirmation evidence from post-submit page text.
//!
//! Everything here is heuristic where it reads page text or labels; each module says where.

pub mod expect;
pub mod idempotency;
pub mod receipt;
pub mod risk;
pub mod task;
pub mod trace;

pub use expect::{
    evaluate, parse_expect, ElementRef, Expect, ExpectError, ExpectResult, Failure, Observation,
};
pub use idempotency::{Fingerprint, Guard, GuardVerdict, Salt};
pub use receipt::{extract_receipt, Receipt, ReceiptId};
pub use risk::{
    classify, decide, ActionDesc, Classification, Decision, DomainPolicy, FieldDesc, Mode, Policy,
    PolicyAction, RiskClass,
};
pub use task::{
    Budget, BudgetExceeded, BudgetKind, Claim, Clock, ManualClock, Outcome, OutcomeTally,
    SystemClock, Task, TaskError, TaskReport, TaskSpec, Verdict,
};
pub use trace::{otel_export, Redactor, TraceConfig, TraceEvent, TraceWriter};

/// Truncate to at most `max` chars, replacing control characters with spaces. Used wherever
/// page-derived or agent-derived text is put into a one-line string.
pub(crate) fn one_line(s: &str, max: usize) -> String {
    let mut out = String::new();
    for (n, c) in s.chars().enumerate() {
        if n >= max {
            out.push('…');
            break;
        }
        out.push(if c.is_control() { ' ' } else { c });
    }
    out
}
