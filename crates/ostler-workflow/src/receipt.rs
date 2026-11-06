//! Confirmation evidence from post-submit page text: order, confirmation, reference and booking
//! numbers, "thank you" style phrases, and failure phrases that contradict them.
//!
//! This is a **heuristic over untrusted text**. Anything on the page, including an attacker's
//! page, can print "Order confirmed #123456". A receipt is evidence for a person or an outcome
//! check to weigh, never proof that the server accepted the submission. Rules are deliberately
//! conservative: an id needs a label word (order, confirmation, reference, ...), a digit, and
//! either an explicit marker (`number`, `#`, `:`, `id`, `code`) or at least six characters.
//! English only; ids written in prose with no digit ("reference ALPHA") are not found.

use serde::{Deserialize, Serialize};

use crate::one_line;

const MAX_CHARS: usize = 100_000;
const MAX_IDS: usize = 8;

/// A labelled identifier found on the page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptId {
    /// Lower-case label, e.g. `order number`.
    pub label: String,
    /// The identifier as printed.
    pub value: String,
}

/// Evidence extracted from page text.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    /// Labelled identifiers (at most 8).
    pub ids: Vec<ReceiptId>,
    /// Lower-case confirmation phrases found.
    pub phrases: Vec<String>,
    /// Failure phrases found ("payment failed"). Their presence contradicts the evidence above.
    pub warnings: Vec<String>,
}

impl Receipt {
    /// Any id or confirmation phrase at all.
    pub fn has_evidence(&self) -> bool {
        !self.ids.is_empty() || !self.phrases.is_empty()
    }

    /// Evidence present and no failure phrase contradicts it.
    pub fn looks_confirmed(&self) -> bool {
        self.has_evidence() && self.warnings.is_empty()
    }

    /// Terse one-liner for tool results. Values come from the page: single-line and truncated,
    /// but still untrusted.
    pub fn render(&self) -> String {
        if !self.has_evidence() && self.warnings.is_empty() {
            return "RECEIPT: none found".into();
        }
        let mut parts: Vec<String> = self
            .ids
            .iter()
            .map(|i| format!("{} {}", i.label, one_line(&i.value, 48)))
            .collect();
        parts.extend(
            self.phrases
                .iter()
                .map(|p| format!("\"{}\"", one_line(p, 60))),
        );
        let mut s = format!("RECEIPT: {}", parts.join("; "));
        if !self.warnings.is_empty() {
            s.push_str(&format!(" | WARNING: {}", self.warnings.join("; ")));
        }
        s
    }
}

const NOUNS: &[&str] = &[
    "confirmation",
    "order",
    "reference",
    "booking",
    "reservation",
    "ticket",
    "tracking",
    "invoice",
    "transaction",
    "receipt",
    "case",
    "request",
    "application",
    "registration",
    "appointment",
    "claim",
    "payment",
];
const KINDS: &[&str] = &["number", "no", "no.", "num", "id", "code", "ref", "#"];
const FILLERS: &[&str] = &["is", "was", "are", ":", "-", "–", "—"];

const PHRASES: &[&str] = &[
    "your order has been placed",
    "your order is confirmed",
    "order confirmed",
    "order confirmation",
    "order received",
    "booking confirmed",
    "booking confirmation",
    "reservation confirmed",
    "appointment confirmed",
    "registration complete",
    "registration successful",
    "payment successful",
    "payment received",
    "successfully submitted",
    "has been submitted",
    "has been received",
    "we have received your",
    "application submitted",
    "application received",
    "confirmation email",
    "you're all set",
    "you are all set",
    "you are registered",
    "successfully registered",
    "successfully subscribed",
    "message sent",
    "form submitted",
];
const THANK_STARTS: &[&str] = &[
    "your",
    "booking",
    "ordering",
    "registering",
    "signing",
    "subscribing",
    "submitting",
    "applying",
    "contacting",
    "donating",
    "shopping",
    "purchasing",
    "choosing",
    "reserving",
    "completing",
    "the",
];
const WARNINGS: &[&str] = &[
    "payment failed",
    "payment declined",
    "card was declined",
    "transaction failed",
    "could not be processed",
    "was not successful",
    "unsuccessful",
    "not been submitted",
    "not been placed",
    "has failed",
    "something went wrong",
    "an error occurred",
    "session expired",
    "try again later",
];

fn clean(w: &str) -> &str {
    w.trim_matches(|c: char| {
        matches!(
            c,
            '.' | ','
                | ';'
                | ':'
                | '!'
                | '?'
                | ')'
                | '('
                | '"'
                | '\''
                | '['
                | ']'
                | '“'
                | '”'
                | '*'
        )
    })
}

fn is_date_like(t: &str) -> bool {
    let p: Vec<&str> = t.split('-').collect();
    p.len() == 3
        && p.iter()
            .all(|x| !x.is_empty() && x.chars().all(|c| c.is_ascii_digit()))
        && p.iter().map(|x| x.len()).sum::<usize>() <= 10
}

fn valid_token(t: &str, marker: bool) -> bool {
    let len = t.chars().count();
    (4..=40).contains(&len)
        && t.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && t.chars().any(|c| c.is_ascii_digit())
        && !is_date_like(t)
        && (marker || len >= 6)
}

fn find_ids(text: &str) -> Vec<ReceiptId> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let low: Vec<String> = words.iter().map(|w| clean(w).to_lowercase()).collect();
    let mut out: Vec<ReceiptId> = Vec::new();
    let mut i = 0;
    while i < words.len() && out.len() < MAX_IDS {
        if !NOUNS.contains(&low[i].as_str()) {
            i += 1;
            continue;
        }
        let mut label = vec![low[i].clone()];
        let mut marker = words[i].ends_with(':');
        let mut j = i + 1;
        while j < words.len() && NOUNS.contains(&low[j].as_str()) && label.len() < 3 {
            label.push(low[j].clone());
            marker |= words[j].ends_with(':');
            j += 1;
        }
        if j < words.len() && KINDS.contains(&low[j].as_str()) {
            if low[j] != "#" {
                label.push(low[j].clone());
            }
            marker = true;
            j += 1;
        }
        let mut skipped = 0;
        while j < words.len() && skipped < 3 && FILLERS.contains(&words[j]) {
            marker |= words[j] != "is" && words[j] != "was" && words[j] != "are";
            j += 1;
            skipped += 1;
        }
        if j < words.len() {
            let raw = words[j];
            if raw.starts_with('#') {
                marker = true;
            }
            let cand = clean(raw.trim_start_matches('#'));
            if valid_token(cand, marker) {
                let value = cand.to_string();
                if !out.iter().any(|x| x.value.eq_ignore_ascii_case(&value)) {
                    out.push(ReceiptId {
                        label: label.join(" "),
                        value,
                    });
                }
            }
        }
        i += 1;
    }
    out
}

fn find_phrases(norm: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in PHRASES {
        if norm.contains(p) && !out.iter().any(|x| x == p) {
            out.push((*p).to_string());
        }
    }
    for lead in ["thank you for ", "thanks for "] {
        let mut from = 0;
        while let Some(pos) = norm[from..].find(lead) {
            let start = from + pos + lead.len();
            from = start;
            let mut taken: Vec<String> = Vec::new();
            for w in norm[start..].split(' ').take(4) {
                let stop = w.ends_with(['.', '!', '?', ',', ';', ':']);
                taken.push(
                    w.trim_end_matches(['.', '!', '?', ',', ';', ':'])
                        .to_string(),
                );
                if stop {
                    break;
                }
            }
            if taken
                .first()
                .is_some_and(|f| THANK_STARTS.contains(&f.as_str()))
            {
                let phrase = format!("{}{}", lead, taken.join(" "));
                if !out.contains(&phrase) {
                    out.push(phrase);
                }
            }
            if out.len() > 12 {
                break;
            }
        }
    }
    out
}

/// Extract receipt evidence from post-submit page text. Looks at the first 100,000 characters.
pub fn extract_receipt(text: &str) -> Receipt {
    let text: String = text.chars().take(MAX_CHARS).collect();
    let norm = text
        .replace(['’', '‘'], "'")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let warnings = WARNINGS
        .iter()
        .filter(|w| norm.contains(**w))
        .map(|w| (*w).to_string())
        .collect();
    Receipt {
        ids: find_ids(&text),
        phrases: find_phrases(&norm),
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(t: &str) -> Vec<String> {
        extract_receipt(t)
            .ids
            .into_iter()
            .map(|i| i.value)
            .collect()
    }

    #[test]
    fn varied_id_phrasing() {
        assert_eq!(
            ids("Your order number is 123-4567890."),
            vec!["123-4567890"]
        );
        assert_eq!(ids("Order #A1B2C3 confirmed"), vec!["A1B2C3"]);
        assert_eq!(ids("Confirmation number: XK42-99"), vec!["XK42-99"]);
        assert_eq!(ids("Confirmation Code ABC123"), vec!["ABC123"]);
        assert_eq!(ids("Booking reference: PNR8H2K"), vec!["PNR8H2K"]);
        assert_eq!(ids("Your order 98765432 has shipped"), vec!["98765432"]);
        assert_eq!(
            ids("Order ID: 7"),
            Vec::<String>::new(),
            "too short even with marker"
        );
        assert_eq!(ids("Order confirmation no. 555-22"), vec!["555-22"]);
        assert_eq!(
            ids("Tracking number 1Z999AA10123456784"),
            vec!["1Z999AA10123456784"]
        );
        assert_eq!(ids("Reference #4432"), vec!["4432"]);
        assert_eq!(ids("Ticket (TCK-20931) opened"), vec!["TCK-20931"]);
    }

    #[test]
    fn label_is_reported() {
        let r = extract_receipt("Order number: 55501234");
        assert_eq!(r.ids[0].label, "order number");
        let r = extract_receipt("Order confirmation number 555012");
        assert_eq!(r.ids[0].label, "order confirmation number");
    }

    #[test]
    fn conservative_rejections() {
        for t in [
            "Your order has been placed",
            "Reference code below",
            "Order 3 items",
            "Order date: 2024-11-06",
            "case study 2024",
            "Request more information",
            "Confirmation: pending",
            "order number: ",
            "",
        ] {
            assert!(ids(t).is_empty(), "{t:?} -> {:?}", ids(t));
        }
    }

    #[test]
    fn dedupes_and_caps() {
        assert_eq!(
            ids("Order #55501234. Confirmation 55501234 again, order #55501234"),
            vec!["55501234"]
        );
        let many: String = (0..30).map(|i| format!("Order #10000{i} ")).collect();
        assert_eq!(ids(&many).len(), MAX_IDS);
    }

    #[test]
    fn phrases() {
        let r = extract_receipt("Thank you for your order! We have received your payment.\nA confirmation email is on its way.");
        assert!(
            r.phrases.contains(&"thank you for your order".to_string()),
            "{:?}",
            r.phrases
        );
        assert!(r.phrases.contains(&"confirmation email".to_string()));
        assert!(r.has_evidence());
        assert!(r.looks_confirmed());
        let r = extract_receipt("Thanks   for\nbooking with us today and more words");
        assert_eq!(
            r.phrases,
            vec!["thanks for booking with us today".to_string()]
        );
        let r = extract_receipt("You're all set. Registration complete.");
        assert!(r.phrases.contains(&"you're all set".to_string()));
        let r = extract_receipt("You’re all set");
        assert!(
            r.phrases.contains(&"you're all set".to_string()),
            "curly apostrophe"
        );
    }

    #[test]
    fn thank_you_needs_a_known_continuation() {
        assert!(
            extract_receipt("Thank you for the feedback about shipping")
                .phrases
                .len()
                == 1
        );
        assert!(extract_receipt("Thank you for visiting our site")
            .phrases
            .is_empty());
        assert!(extract_receipt("thank you").phrases.is_empty());
    }

    #[test]
    fn failure_phrases_contradict() {
        let r =
            extract_receipt("Thank you for your order. Payment failed, please try again later.");
        assert!(r.has_evidence());
        assert!(!r.looks_confirmed());
        assert_eq!(r.warnings, vec!["payment failed", "try again later"]);
        let r = extract_receipt("Your application has not been submitted.");
        assert!(r.phrases.is_empty());
        assert_eq!(r.warnings, vec!["not been submitted"]);
    }

    #[test]
    fn render_is_terse_and_single_line() {
        let r = extract_receipt("Order number: 55501234\nThank you for your order");
        assert_eq!(
            r.render(),
            "RECEIPT: order number 55501234; \"thank you for your order\""
        );
        assert_eq!(extract_receipt("hello").render(), "RECEIPT: none found");
        let r = extract_receipt("Payment failed");
        assert!(r.render().contains("WARNING: payment failed"));
    }

    #[test]
    fn huge_and_odd_input() {
        let big = "word ".repeat(200_000);
        let _ = extract_receipt(&big);
        let _ = extract_receipt("Order #\u{0}\u{1f4a9}\u{1f4a9}\u{1f4a9} 💳");
        assert!(
            extract_receipt(&format!("{} Order #55501234", "x ".repeat(60_000)))
                .ids
                .is_empty(),
            "past the 100k char cap"
        );
    }
}
