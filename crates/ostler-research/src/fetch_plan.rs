//! A pure planner for multi-URL opens (`open_many`).
//!
//! No I/O: the caller gathers robots decisions first and passes them in, then executes
//! [`Plan::to_fetch`] and records every [`Plan::skipped`] entry as a gap, so nothing is dropped silently.
//!
//! Order of checks per input URL: invalid, duplicate, already fetched (ledger `Ok`, unless forced),
//! robots, page cap. Input order is preserved.

use std::collections::HashSet;

use crate::budget::Budget;
use crate::ledger::{Ledger, Outcome};
use crate::robots::Decision;
use crate::search::normalise_url;

/// Why a URL will not be fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    /// Not an http(s) URL without credentials.
    Invalid,
    /// Same normalised URL as an earlier input.
    Duplicate {
        /// The earlier URL it duplicates.
        of: String,
    },
    /// The ledger already holds an `Ok` fetch of it (use `force` to refetch).
    AlreadyFetched,
    /// robots.txt disallows it.
    RobotsDisallowed {
        /// Deciding rule.
        rule: String,
    },
    /// robots.txt could not be fetched, so the URL is treated as disallowed.
    RobotsUnreachable,
    /// The page cap was reached.
    OverPageBudget {
        /// The cap.
        max_pages: usize,
    },
}

impl SkipReason {
    /// Short text for tool output.
    pub fn describe(&self) -> String {
        match self {
            Self::Invalid => "invalid or non-http(s) URL".into(),
            Self::Duplicate { of } => format!("duplicate of {of}"),
            Self::AlreadyFetched => "already fetched (ledger)".into(),
            Self::RobotsDisallowed { rule } => format!("robots.txt disallows ({rule})"),
            Self::RobotsUnreachable => "robots.txt unreachable, treated as disallowed".into(),
            Self::OverPageBudget { max_pages } => format!("over page budget ({max_pages})"),
        }
    }

    /// Ledger outcome a caller should record for this skip, if it is a gap (not a duplicate or a prior success).
    pub fn ledger_outcome(&self) -> Option<Outcome> {
        match self {
            Self::RobotsDisallowed { .. } => Some(Outcome::RobotsDisallowed),
            Self::RobotsUnreachable => Some(Outcome::Failed("robots.txt unreachable".into())),
            Self::OverPageBudget { .. } => Some(Outcome::BudgetExceeded),
            Self::Invalid => Some(Outcome::Failed("invalid URL".into())),
            Self::Duplicate { .. } | Self::AlreadyFetched => None,
        }
    }
}

/// A URL that will be fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedFetch {
    /// Normalised URL to fetch.
    pub url: String,
    /// The input string it came from.
    pub original: String,
}

/// A URL that will not be fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    /// The input string (normalised when it could be).
    pub url: String,
    /// Why.
    pub reason: SkipReason,
}

/// The plan.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// Fetch these, in order.
    pub to_fetch: Vec<PlannedFetch>,
    /// Skipped inputs with reasons (every input appears in exactly one list).
    pub skipped: Vec<Skipped>,
}

impl Plan {
    /// Terse summary, e.g. `plan: 3 to fetch, 2 skipped (1 duplicate, 1 robots)`.
    pub fn summary(&self) -> String {
        let mut s = format!("plan: {} to fetch", self.to_fetch.len());
        if !self.skipped.is_empty() {
            let count =
                |f: fn(&SkipReason) -> bool| self.skipped.iter().filter(|k| f(&k.reason)).count();
            let parts = [
                (count(|r| matches!(r, SkipReason::Invalid)), "invalid"),
                (
                    count(|r| matches!(r, SkipReason::Duplicate { .. })),
                    "duplicate",
                ),
                (
                    count(|r| matches!(r, SkipReason::AlreadyFetched)),
                    "already fetched",
                ),
                (
                    count(|r| {
                        matches!(
                            r,
                            SkipReason::RobotsDisallowed { .. } | SkipReason::RobotsUnreachable
                        )
                    }),
                    "robots",
                ),
                (
                    count(|r| matches!(r, SkipReason::OverPageBudget { .. })),
                    "over budget",
                ),
            ];
            let list: Vec<String> = parts
                .iter()
                .filter(|(n, _)| *n > 0)
                .map(|(n, l)| format!("{n} {l}"))
                .collect();
            s.push_str(&format!(
                ", {} skipped ({})",
                self.skipped.len(),
                list.join(", ")
            ));
        }
        s
    }
}

/// Inputs to [`plan`].
pub struct PlanInput<'a> {
    /// URLs as given by the agent.
    pub urls: &'a [String],
    /// Session budget; only `max_pages` is used here.
    pub budget: &'a Budget,
    /// Pages already used in this session (counted against `max_pages`).
    pub pages_used: usize,
    /// Robots decision for a normalised URL.
    pub robots: &'a dyn Fn(&str) -> Decision,
    /// Source ledger.
    pub ledger: &'a Ledger,
    /// Refetch URLs the ledger already holds as `Ok`.
    pub force: bool,
}

/// Build the plan.
pub fn plan(input: &PlanInput<'_>) -> Plan {
    let mut out = Plan::default();
    let mut seen: HashSet<String> = HashSet::new();
    let mut first_for: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let room = input.budget.max_pages.saturating_sub(input.pages_used);
    for raw in input.urls {
        let Some(url) = normalise_url(raw) else {
            out.skipped.push(Skipped {
                url: raw.clone(),
                reason: SkipReason::Invalid,
            });
            continue;
        };
        if !seen.insert(url.clone()) {
            let of = first_for.get(&url).cloned().unwrap_or_else(|| url.clone());
            out.skipped.push(Skipped {
                url,
                reason: SkipReason::Duplicate { of },
            });
            continue;
        }
        first_for.insert(url.clone(), raw.clone());
        let skip = if !input.force
            && input
                .ledger
                .get(&url)
                .is_some_and(|e| e.outcome == Outcome::Ok)
        {
            Some(SkipReason::AlreadyFetched)
        } else {
            match (input.robots)(&url) {
                Decision::Allowed => None,
                Decision::Disallowed { rule } => Some(SkipReason::RobotsDisallowed { rule }),
                Decision::Unreachable => Some(SkipReason::RobotsUnreachable),
            }
        };
        let skip = skip.or_else(|| {
            (out.to_fetch.len() >= room).then_some(SkipReason::OverPageBudget {
                max_pages: input.budget.max_pages,
            })
        });
        match skip {
            Some(reason) => out.skipped.push(Skipped { url, reason }),
            None => out.to_fetch.push(PlannedFetch {
                url,
                original: raw.clone(),
            }),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::SourceEntry;

    fn urls(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    fn allow(_: &str) -> Decision {
        Decision::Allowed
    }

    fn run(
        u: &[&str],
        max_pages: usize,
        used: usize,
        ledger: &Ledger,
        force: bool,
        robots: &dyn Fn(&str) -> Decision,
    ) -> Plan {
        let budget = Budget {
            max_pages,
            ..Budget::default()
        };
        let urls = urls(u);
        plan(&PlanInput {
            urls: &urls,
            budget: &budget,
            pages_used: used,
            robots,
            ledger,
            force,
        })
    }

    #[test]
    fn dedupes_and_flags_invalid() {
        let l = Ledger::in_memory();
        let p = run(
            &[
                "https://a.com/x?utm_a=1",
                "https://A.com/x#f",
                "mailto:x@y",
                "https://b.com/",
            ],
            10,
            0,
            &l,
            false,
            &allow,
        );
        assert_eq!(
            p.to_fetch
                .iter()
                .map(|f| f.url.as_str())
                .collect::<Vec<_>>(),
            ["https://a.com/x", "https://b.com/"]
        );
        assert_eq!(p.skipped.len(), 2);
        assert!(matches!(p.skipped[0].reason, SkipReason::Duplicate { .. }));
        assert_eq!(p.skipped[1].reason, SkipReason::Invalid);
        assert_eq!(p.to_fetch[0].original, "https://a.com/x?utm_a=1");
    }

    #[test]
    fn skips_fetched_unless_forced_but_refetches_failures() {
        let l = Ledger::in_memory();
        l.record(SourceEntry::new("https://a.com/", Outcome::Ok))
            .unwrap();
        l.record(SourceEntry::new(
            "https://b.com/",
            Outcome::Failed("timeout".into()),
        ))
        .unwrap();
        let p = run(
            &["https://a.com/", "https://b.com/"],
            10,
            0,
            &l,
            false,
            &allow,
        );
        assert_eq!(p.to_fetch.len(), 1);
        assert_eq!(p.to_fetch[0].url, "https://b.com/");
        assert_eq!(p.skipped[0].reason, SkipReason::AlreadyFetched);
        let p = run(&["https://a.com/"], 10, 0, &l, true, &allow);
        assert_eq!(p.to_fetch.len(), 1);
    }

    #[test]
    fn robots_decisions_are_applied() {
        let l = Ledger::in_memory();
        let robots = |u: &str| match u {
            "https://no.com/" => Decision::Disallowed {
                rule: "Disallow: /".into(),
            },
            "https://down.com/" => Decision::Unreachable,
            _ => Decision::Allowed,
        };
        let p = run(
            &["https://no.com/", "https://down.com/", "https://ok.com/"],
            10,
            0,
            &l,
            false,
            &robots,
        );
        assert_eq!(p.to_fetch.len(), 1);
        assert_eq!(
            p.skipped[0].reason,
            SkipReason::RobotsDisallowed {
                rule: "Disallow: /".into()
            }
        );
        assert_eq!(
            p.skipped[0].reason.ledger_outcome(),
            Some(Outcome::RobotsDisallowed)
        );
        assert_eq!(p.skipped[1].reason, SkipReason::RobotsUnreachable);
        assert!(p.skipped[1].reason.describe().contains("unreachable"));
    }

    #[test]
    fn page_cap_counts_used_pages_and_only_fetchable_urls() {
        let l = Ledger::in_memory();
        let robots = |u: &str| {
            if u == "https://no.com/" {
                Decision::Disallowed { rule: "r".into() }
            } else {
                Decision::Allowed
            }
        };
        let p = run(
            &[
                "https://no.com/",
                "https://a.com/",
                "https://b.com/",
                "https://c.com/",
            ],
            3,
            1,
            &l,
            false,
            &robots,
        );
        assert_eq!(
            p.to_fetch.len(),
            2,
            "robots skip does not use budget; 1 page already used"
        );
        assert_eq!(p.skipped.len(), 2);
        assert_eq!(
            p.skipped[1].reason,
            SkipReason::OverPageBudget { max_pages: 3 }
        );
        assert_eq!(
            p.skipped[1].reason.ledger_outcome(),
            Some(Outcome::BudgetExceeded)
        );
        let p = run(&["https://a.com/"], 2, 5, &l, false, &allow);
        assert!(p.to_fetch.is_empty());
        assert_eq!(p.to_fetch.len() + p.skipped.len(), 1);
    }

    #[test]
    fn summary_text() {
        let l = Ledger::in_memory();
        let p = run(
            &["https://a.com/", "https://a.com/", "x"],
            10,
            0,
            &l,
            false,
            &allow,
        );
        assert_eq!(
            p.summary(),
            "plan: 1 to fetch, 2 skipped (1 invalid, 1 duplicate)"
        );
        assert_eq!(Plan::default().summary(), "plan: 0 to fetch");
    }
}
