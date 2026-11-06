//! Task contracts: a goal, an outcome check stated up front and a budget. Ostler checks the
//! outcome at `end` instead of trusting the agent's own claim of success.
//!
//! The state machine is `begin` -> `record_action`* -> `end`. Time comes from an injectable
//! [`Clock`], so tests and replays are deterministic.
//!
//! # False success
//!
//! The agent states a [`Claim`] at `end`; the [`Verdict`] comes from the outcome check. The two
//! combine into an [`Outcome`]. `FalseSuccess` (claimed success, check failed) is the metric that
//! matters most. It is only as trustworthy as the outcome check itself: a weak check yields
//! `TrueSuccess` for work that did not happen.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::expect::{
    evaluate, parse_expect, Expect, ExpectError, ExpectResult, Failure, Observation,
};
use crate::one_line;

/// Source of "now" in milliseconds. Only differences matter.
pub trait Clock: Send + Sync {
    /// Milliseconds since an arbitrary fixed point.
    fn now_ms(&self) -> u64;
}

/// Wall clock (milliseconds since the Unix epoch).
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

/// A clock you move by hand. Clones share the same time.
#[derive(Debug, Default, Clone)]
pub struct ManualClock(Arc<AtomicU64>);

impl ManualClock {
    /// Start at `ms`.
    pub fn new(ms: u64) -> Self {
        Self(Arc::new(AtomicU64::new(ms)))
    }
    /// Move forward.
    pub fn advance_ms(&self, ms: u64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
    /// Jump to an absolute time.
    pub fn set_ms(&self, ms: u64) {
        self.0.store(ms, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// Limits for one task. `None` means unlimited.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budget {
    /// Most actions the task may perform.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_actions: Option<u32>,
    /// Longest wall time in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_secs: Option<u64>,
}

/// What `task begin` receives.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSpec {
    /// Free-text goal (for reports only; never evaluated).
    pub goal: String,
    /// Outcome check. Empty means the verdict will be [`Verdict::Unknown`].
    #[serde(default)]
    pub expect: Vec<Expect>,
    /// Limits.
    #[serde(default)]
    pub budget: Budget,
}

impl TaskSpec {
    /// Parse the agent-facing JSON: `{"goal": "...", "expect": <expect>, "budget": {"max_actions": 30, "max_secs": 120}}`.
    pub fn from_json(v: &Value) -> Result<TaskSpec, ExpectError> {
        let bad = |path: &str, msg: &str| ExpectError {
            path: path.to_string(),
            msg: msg.to_string(),
        };
        let m = v
            .as_object()
            .ok_or_else(|| bad("task", "must be an object"))?;
        for k in m.keys() {
            if !matches!(k.as_str(), "goal" | "expect" | "budget") {
                return Err(bad(
                    &format!("task.{k}"),
                    "unknown key (valid: goal, expect, budget)",
                ));
            }
        }
        let goal = match m.get("goal") {
            Some(Value::String(s)) if !s.trim().is_empty() => one_line(s.trim(), 500),
            _ => return Err(bad("task.goal", "required non-empty string")),
        };
        let expect = match m.get("expect") {
            Some(e) => parse_expect(e)?,
            None => Vec::new(),
        };
        let mut budget = Budget::default();
        if let Some(b) = m.get("budget") {
            let bm = b
                .as_object()
                .ok_or_else(|| bad("task.budget", "must be an object"))?;
            for (k, x) in bm {
                let n = x.as_u64().filter(|n| *n > 0).ok_or_else(|| {
                    bad(&format!("task.budget.{k}"), "must be a positive integer")
                })?;
                match k.as_str() {
                    "max_actions" => {
                        budget.max_actions = Some(
                            u32::try_from(n)
                                .map_err(|_| bad("task.budget.max_actions", "too large"))?,
                        )
                    }
                    "max_secs" => budget.max_secs = Some(n),
                    _ => {
                        return Err(bad(
                            &format!("task.budget.{k}"),
                            "unknown key (valid: max_actions, max_secs)",
                        ))
                    }
                }
            }
        }
        Ok(TaskSpec {
            goal,
            expect,
            budget,
        })
    }
}

/// What the agent says happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Claim {
    /// Agent believes the goal was achieved.
    Success,
    /// Agent believes it was not.
    Failure,
    /// Agent does not know.
    Unsure,
}

/// Which budget limit was hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetKind {
    /// `max_actions`.
    Actions,
    /// `max_secs`.
    Secs,
}

/// Details of a budget breach.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("budget exceeded: {kind:?} {used}/{limit}")]
pub struct BudgetExceeded {
    /// Which limit.
    pub kind: BudgetKind,
    /// The limit.
    pub limit: u64,
    /// Amount used when the breach was detected.
    pub used: u64,
}

/// Result of the outcome check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum Verdict {
    /// Every expectation held.
    Pass,
    /// At least one expectation did not hold.
    Fail {
        /// The unmet expectations.
        failed: Vec<Failure>,
    },
    /// No outcome check was supplied, so nothing can be said.
    Unknown,
    /// A budget limit was hit; the outcome check is not consulted.
    BudgetExceeded {
        /// The breach.
        exceeded: BudgetExceeded,
    },
}

impl Verdict {
    /// Terse one-liner (plus one line per failure).
    pub fn render(&self) -> String {
        match self {
            Verdict::Pass => "TASK PASS".into(),
            Verdict::Fail { failed } => {
                let mut s = format!("TASK FAIL: {} unmet", failed.len());
                for f in failed {
                    s.push_str(&format!("\n- {}: {}", f.expect, one_line(&f.why, 160)));
                }
                s
            }
            Verdict::Unknown => "TASK UNKNOWN: no outcome check supplied".into(),
            Verdict::BudgetExceeded { exceeded } => format!(
                "TASK BUDGET EXCEEDED: {} {}/{}",
                match exceeded.kind {
                    BudgetKind::Actions => "actions",
                    BudgetKind::Secs => "secs",
                },
                exceeded.used,
                exceeded.limit
            ),
        }
    }
}

/// Claim x verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Agent claimed success and the check passed.
    TrueSuccess,
    /// Agent claimed success but the check failed. The dangerous one.
    FalseSuccess,
    /// Agent claimed failure and the check failed.
    TrueFailure,
    /// Agent claimed failure but the check passed.
    FalseFailure,
    /// Not decidable: no check, budget exceeded, or the agent was unsure.
    Unverified,
}

impl Outcome {
    /// Combine a claim with a verdict.
    pub fn from(claim: Claim, verdict: &Verdict) -> Outcome {
        match (claim, verdict) {
            (Claim::Success, Verdict::Pass) => Outcome::TrueSuccess,
            (Claim::Success, Verdict::Fail { .. }) => Outcome::FalseSuccess,
            (Claim::Failure, Verdict::Fail { .. }) => Outcome::TrueFailure,
            (Claim::Failure, Verdict::Pass) => Outcome::FalseFailure,
            _ => Outcome::Unverified,
        }
    }

    /// Upper-case label for tool results.
    pub fn label(self) -> &'static str {
        match self {
            Outcome::TrueSuccess => "TRUE SUCCESS",
            Outcome::FalseSuccess => "FALSE SUCCESS",
            Outcome::TrueFailure => "TRUE FAILURE",
            Outcome::FalseFailure => "FALSE FAILURE",
            Outcome::Unverified => "UNVERIFIED",
        }
    }
}

/// Counts of outcomes over many tasks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutcomeTally {
    /// Count of [`Outcome::TrueSuccess`].
    pub true_success: u64,
    /// Count of [`Outcome::FalseSuccess`].
    pub false_success: u64,
    /// Count of [`Outcome::TrueFailure`].
    pub true_failure: u64,
    /// Count of [`Outcome::FalseFailure`].
    pub false_failure: u64,
    /// Count of [`Outcome::Unverified`].
    pub unverified: u64,
}

impl OutcomeTally {
    /// Add one outcome.
    pub fn add(&mut self, o: Outcome) {
        match o {
            Outcome::TrueSuccess => self.true_success += 1,
            Outcome::FalseSuccess => self.false_success += 1,
            Outcome::TrueFailure => self.true_failure += 1,
            Outcome::FalseFailure => self.false_failure += 1,
            Outcome::Unverified => self.unverified += 1,
        }
    }

    /// Share of *claimed successes that were verifiable* which turned out false.
    /// `None` when there were no verifiable success claims.
    pub fn false_success_rate(&self) -> Option<f64> {
        let d = self.true_success + self.false_success;
        (d > 0).then(|| self.false_success as f64 / d as f64)
    }
}

/// Misuse of the task state machine.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TaskError {
    /// The task already ended.
    #[error("task already ended")]
    Ended,
    /// The spec has expectations but `end` got no observation.
    #[error("task has an outcome check but no observation was supplied")]
    ObservationRequired,
}

/// What `end` returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskReport {
    /// Task id.
    pub task_id: String,
    /// The goal as stated.
    pub goal: String,
    /// What the agent claimed.
    pub claim: Claim,
    /// What the check found.
    pub verdict: Verdict,
    /// Claim x verdict.
    pub outcome: Outcome,
    /// Actions recorded (refused over-budget actions are not counted).
    pub actions: u32,
    /// Elapsed time.
    pub elapsed_ms: u64,
    /// Full expectation result when a check ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect: Option<ExpectResult>,
}

impl TaskReport {
    /// Terse text: verdict, then the outcome line.
    pub fn render(&self) -> String {
        format!(
            "{}\n{} (claimed {:?}; {} actions, {} ms)",
            self.verdict.render(),
            self.outcome.label(),
            self.claim,
            self.actions,
            self.elapsed_ms
        )
    }
}

/// A running task.
pub struct Task {
    id: String,
    spec: TaskSpec,
    clock: Arc<dyn Clock>,
    started_ms: u64,
    actions: u32,
    breach: Option<BudgetExceeded>,
    ended: bool,
}

impl std::fmt::Debug for Task {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Task")
            .field("id", &self.id)
            .field("actions", &self.actions)
            .field("ended", &self.ended)
            .finish()
    }
}

impl Task {
    /// Start a task.
    pub fn begin(id: impl Into<String>, spec: TaskSpec, clock: Arc<dyn Clock>) -> Task {
        let started_ms = clock.now_ms();
        Task {
            id: id.into(),
            spec,
            clock,
            started_ms,
            actions: 0,
            breach: None,
            ended: false,
        }
    }

    /// Task id.
    pub fn id(&self) -> &str {
        &self.id
    }
    /// The contract.
    pub fn spec(&self) -> &TaskSpec {
        &self.spec
    }
    /// Actions recorded so far.
    pub fn actions(&self) -> u32 {
        self.actions
    }
    /// Milliseconds since `begin`.
    pub fn elapsed_ms(&self) -> u64 {
        self.clock.now_ms().saturating_sub(self.started_ms)
    }
    /// True once `end` succeeded.
    pub fn is_ended(&self) -> bool {
        self.ended
    }
    /// The recorded breach, if any. Sticky.
    pub fn breach(&self) -> Option<BudgetExceeded> {
        self.breach
    }

    fn check_time(&mut self) -> Option<BudgetExceeded> {
        if let Some(max) = self.spec.budget.max_secs {
            let used_ms = self.elapsed_ms();
            if used_ms > max.saturating_mul(1000) {
                let b = BudgetExceeded {
                    kind: BudgetKind::Secs,
                    limit: max,
                    used: used_ms / 1000,
                };
                self.breach.get_or_insert(b);
                return Some(b);
            }
        }
        None
    }

    /// Call before each action. `Ok` means the action may run and has been counted; `Err` means
    /// the budget is exhausted and the action must not run. A breach is sticky and makes the
    /// final verdict [`Verdict::BudgetExceeded`].
    pub fn record_action(&mut self) -> Result<(), BudgetExceeded> {
        if let Some(b) = self.breach {
            return Err(b);
        }
        if let Some(b) = self.check_time() {
            return Err(b);
        }
        if let Some(max) = self.spec.budget.max_actions {
            if self.actions >= max {
                let b = BudgetExceeded {
                    kind: BudgetKind::Actions,
                    limit: u64::from(max),
                    used: u64::from(self.actions),
                };
                self.breach = Some(b);
                return Err(b);
            }
        }
        self.actions += 1;
        Ok(())
    }

    /// Finish the task. `observation` is the page as it is now; it is required when the spec has
    /// expectations (otherwise `Err(ObservationRequired)` and the task stays open). Verdict rules:
    /// a budget breach (including time exceeded by now) wins and the check is not consulted; no
    /// expectations gives `Unknown`; otherwise `Pass` or `Fail`.
    pub fn end(
        &mut self,
        claim: Claim,
        observation: Option<&Observation>,
    ) -> Result<TaskReport, TaskError> {
        if self.ended {
            return Err(TaskError::Ended);
        }
        self.check_time();
        let mut expect_result = None;
        let verdict = if let Some(b) = self.breach {
            Verdict::BudgetExceeded { exceeded: b }
        } else if self.spec.expect.is_empty() {
            Verdict::Unknown
        } else {
            let obs = observation.ok_or(TaskError::ObservationRequired)?;
            let r = evaluate(&self.spec.expect, obs);
            let v = if r.ok {
                Verdict::Pass
            } else {
                Verdict::Fail {
                    failed: r.failed.clone(),
                }
            };
            expect_result = Some(r);
            v
        };
        self.ended = true;
        let outcome = Outcome::from(claim, &verdict);
        Ok(TaskReport {
            task_id: self.id.clone(),
            goal: self.spec.goal.clone(),
            claim,
            verdict,
            outcome,
            actions: self.actions,
            elapsed_ms: self.elapsed_ms(),
            expect: expect_result,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn spec(v: Value) -> TaskSpec {
        TaskSpec::from_json(&v).unwrap()
    }

    fn page(text: &str) -> Observation {
        Observation {
            url: "https://x.test/done".into(),
            text: text.into(),
            ..Default::default()
        }
    }

    fn task(v: Value) -> (Task, ManualClock) {
        let c = ManualClock::new(1_000);
        (Task::begin("t1", spec(v), Arc::new(c.clone())), c)
    }

    #[test]
    fn spec_parsing() {
        let s = spec(
            json!({"goal":"buy","expect":{"text":"Done"},"budget":{"max_actions":3,"max_secs":9}}),
        );
        assert_eq!(s.budget.max_actions, Some(3));
        assert_eq!(s.expect.len(), 1);
        assert!(TaskSpec::from_json(&json!({"expect":{}})).is_err());
        assert!(TaskSpec::from_json(&json!({"goal":"x","budget":{"max_actions":0}})).is_err());
        assert!(TaskSpec::from_json(&json!({"goal":"x","budget":{"max_actionz":1}})).is_err());
        assert!(TaskSpec::from_json(&json!({"goal":"x","what":1})).is_err());
        let e = TaskSpec::from_json(&json!({"goal":"x","expect":{"txt":"a"}})).unwrap_err();
        assert_eq!(e.path, "expect.txt");
    }

    #[test]
    fn pass_fail_and_outcomes() {
        let (mut t, _) = task(json!({"goal":"g","expect":{"text":"Done"}}));
        t.record_action().unwrap();
        let r = t.end(Claim::Success, Some(&page("All Done"))).unwrap();
        assert_eq!(r.verdict, Verdict::Pass);
        assert_eq!(r.outcome, Outcome::TrueSuccess);

        let (mut t, _) = task(json!({"goal":"g","expect":{"text":"Done"}}));
        let r = t.end(Claim::Success, Some(&page("Oops"))).unwrap();
        assert!(matches!(r.verdict, Verdict::Fail { .. }));
        assert_eq!(r.outcome, Outcome::FalseSuccess);
        assert!(r.render().contains("FALSE SUCCESS"));

        let (mut t, _) = task(json!({"goal":"g","expect":{"text":"Done"}}));
        assert_eq!(
            t.end(Claim::Failure, Some(&page("Oops"))).unwrap().outcome,
            Outcome::TrueFailure
        );
        let (mut t, _) = task(json!({"goal":"g","expect":{"text":"Done"}}));
        assert_eq!(
            t.end(Claim::Failure, Some(&page("Done"))).unwrap().outcome,
            Outcome::FalseFailure
        );
        let (mut t, _) = task(json!({"goal":"g","expect":{"text":"Done"}}));
        assert_eq!(
            t.end(Claim::Unsure, Some(&page("Done"))).unwrap().outcome,
            Outcome::Unverified
        );
    }

    #[test]
    fn unknown_without_check() {
        let (mut t, _) = task(json!({"goal":"g"}));
        let r = t.end(Claim::Success, None).unwrap();
        assert_eq!(r.verdict, Verdict::Unknown);
        assert_eq!(r.outcome, Outcome::Unverified);
        assert!(r.expect.is_none());
    }

    #[test]
    fn observation_required_keeps_task_open() {
        let (mut t, _) = task(json!({"goal":"g","expect":{"text":"x"}}));
        assert_eq!(
            t.end(Claim::Success, None),
            Err(TaskError::ObservationRequired)
        );
        assert!(!t.is_ended());
        assert!(t.end(Claim::Success, Some(&page("x"))).is_ok());
        assert_eq!(
            t.end(Claim::Success, Some(&page("x"))),
            Err(TaskError::Ended)
        );
    }

    #[test]
    fn action_budget() {
        let (mut t, _) = task(json!({"goal":"g","expect":{"text":"x"},"budget":{"max_actions":2}}));
        t.record_action().unwrap();
        t.record_action().unwrap();
        let b = t.record_action().unwrap_err();
        assert_eq!((b.kind, b.limit, b.used), (BudgetKind::Actions, 2, 2));
        assert_eq!(t.actions(), 2);
        assert!(t.record_action().is_err(), "sticky");
        let r = t.end(Claim::Success, Some(&page("x"))).unwrap();
        assert!(matches!(r.verdict, Verdict::BudgetExceeded { .. }));
        assert_eq!(r.outcome, Outcome::Unverified);
        assert!(r.verdict.render().contains("actions 2/2"));
    }

    #[test]
    fn time_budget_with_injected_clock() {
        let (mut t, c) = task(json!({"goal":"g","budget":{"max_secs":10}}));
        c.advance_ms(10_000);
        assert!(t.record_action().is_ok(), "exactly at the limit is fine");
        c.advance_ms(1);
        let b = t.record_action().unwrap_err();
        assert_eq!(b.kind, BudgetKind::Secs);
        assert_eq!(t.elapsed_ms(), 10_001);
    }

    #[test]
    fn time_budget_detected_at_end_without_actions() {
        let (mut t, c) = task(json!({"goal":"g","expect":{"text":"x"},"budget":{"max_secs":1}}));
        c.advance_ms(5_000);
        let r = t.end(Claim::Success, None).unwrap();
        assert!(matches!(r.verdict, Verdict::BudgetExceeded { .. }));
    }

    #[test]
    fn tally() {
        let mut t = OutcomeTally::default();
        assert_eq!(t.false_success_rate(), None);
        t.add(Outcome::TrueSuccess);
        t.add(Outcome::FalseSuccess);
        t.add(Outcome::Unverified);
        assert_eq!(t.false_success_rate(), Some(0.5));
    }

    #[test]
    fn report_serde() {
        let (mut t, _) = task(json!({"goal":"g","expect":{"text":"Done"}}));
        let r = t.end(Claim::Success, Some(&page("no"))).unwrap();
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("\"verdict\":\"fail\""), "{s}");
        let back: TaskReport = serde_json::from_str(&s).unwrap();
        assert_eq!(r, back);
    }
}
