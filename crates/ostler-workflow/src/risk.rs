//! Action risk classes, a heuristic classifier and the per-domain policy that decides what to do
//! with each class. Defaults follow `docs/SCOPE-TASKS.md` section 4.
//!
//! # Classes
//!
//! | class | examples | default decision |
//! |---|---|---|
//! | [`RiskClass::Read`] | navigate, snapshot, extract | allow |
//! | [`RiskClass::ReversibleWrite`] | typing, add to cart, save draft, filter, sign in | allow |
//! | [`RiskClass::Irreversible`] | submit, send, post, publish, delete, confirm, subscribe | needs confirmation |
//! | [`RiskClass::MoneyIdentity`] | pay, buy, transfer, change password, 2FA, delete account | deny unless enabled per domain |
//!
//! # Classifier limits (read before trusting it)
//!
//! [`classify`] is a heuristic over button text, form action, HTTP method and field labels, in
//! English only. Rule: **when unsure, take the stricter class and say so** (`uncertain`).
//!
//! * Labels are controlled by the page. A hostile page can call a payment button "Continue".
//!   The classifier guards against agent mistakes and sloppy pages, not against adversaries;
//!   the browser-level gate (`--require-confirm-submit`) remains the backstop for non-GET
//!   requests.
//! * False positives are expected and deliberate: "Pay later", "Purchase history", "Transfer
//!   files", "Send feedback" and a link to a "Change password" page are all classed as risky.
//! * False negatives: other languages, icon-only buttons, generic labels ("Go", "Next") on a
//!   button that actually submits, GET requests with side effects, and anything built from
//!   JavaScript the caller did not describe in [`ActionDesc`]. A `click` with no label, method or
//!   form information is assumed reversible (and flagged uncertain) because nothing can be said.
//! * Unknown tool names are classed [`RiskClass::Irreversible`] (fail safe).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::one_line;

/// Risk class of an action, ordered from least to most severe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskClass {
    /// Looks without changing anything.
    Read,
    /// Changes state that is easily undone.
    ReversibleWrite,
    /// Commits something external or hard to undo.
    Irreversible,
    /// Moves money or touches identity and credentials.
    MoneyIdentity,
}

impl RiskClass {
    /// snake_case name, as used in policy files.
    pub fn name(self) -> &'static str {
        match self {
            RiskClass::Read => "read",
            RiskClass::ReversibleWrite => "reversible_write",
            RiskClass::Irreversible => "irreversible",
            RiskClass::MoneyIdentity => "money_identity",
        }
    }
}

/// A form field the action involves.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FieldDesc {
    /// Input kind, e.g. `password`, `text`, `email` (as in `ostler_forms::FieldKind`'s snake_case).
    pub kind: Option<String>,
    /// Visible label or accessible name.
    pub label: Option<String>,
}

/// What the browser layer knows about an action about to run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActionDesc {
    /// Tool name, e.g. `click`, `form_submit`.
    pub tool: String,
    /// HTTP method if known (`GET`, `POST`, ...).
    pub method: Option<String>,
    /// URL the action leads to (link href), if known.
    pub target_url: Option<String>,
    /// Text of the control being activated.
    pub control_text: Option<String>,
    /// `action` attribute of the enclosing form.
    pub form_action: Option<String>,
    /// Fields of the form involved.
    pub fields: Vec<FieldDesc>,
}

/// Result of [`classify`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classification {
    /// The class chosen (the strictest of everything that matched).
    pub class: RiskClass,
    /// Why, one short phrase per signal.
    pub reasons: Vec<String>,
    /// True when the classifier chose a stricter class (or a guess) because it was unsure.
    pub uncertain: bool,
}

impl Classification {
    /// One line: `irreversible: reason; reason (unsure)`.
    pub fn summary(&self) -> String {
        format!(
            "{}: {}{}",
            self.class.name(),
            self.reasons.join("; "),
            if self.uncertain {
                " (unsure, stricter class chosen)"
            } else {
                ""
            }
        )
    }
}

const READ_TOOLS: &[&str] = &[
    "navigate",
    "snapshot",
    "look",
    "scroll",
    "hover",
    "wait_for",
    "wait_for_human",
    "back",
    "tabs",
    "context",
    "screenshot",
    "read",
    "extract",
    "links",
    "cite",
    "search",
    "open_many",
    "form_inspect",
    "diagnostics",
    "task",
    "stats",
];
const WRITE_TOOLS: &[&str] = &["type", "fill_field", "form_fill", "downloads"];
const CLICK_TOOLS: &[&str] = &["click", "press"];

fn words(s: &str) -> Vec<String> {
    s.to_lowercase()
        .replace(['\'', '’'], "")
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

fn has(w: &[String], phrase: &str) -> bool {
    let p: Vec<&str> = phrase.split(' ').collect();
    w.windows(p.len())
        .any(|win| win.iter().zip(&p).all(|(a, b)| a == b))
}

fn has_any(w: &[String], phrases: &[&str]) -> bool {
    phrases.iter().any(|p| has(w, p))
}

fn has_word_any(w: &[String], list: &[&str]) -> bool {
    w.iter().any(|x| list.contains(&x.as_str()))
}

const MONEY_PHRASES: &[&str] = &[
    "pay",
    "buy",
    "purchase",
    "place order",
    "place your order",
    "complete order",
    "confirm order",
    "submit order",
    "order now",
    "make payment",
    "confirm payment",
    "submit payment",
    "complete payment",
    "send money",
    "send payment",
    "send funds",
    "transfer",
    "wire",
    "withdraw",
    "withdrawal",
    "deposit",
    "donate",
    "top up",
    "topup",
    "place bid",
    "bid now",
    "free trial",
    "start trial",
    "upgrade plan",
    "renew",
    "change password",
    "reset password",
    "update password",
    "set password",
    "change email",
    "change phone",
    "two factor",
    "2fa",
    "mfa",
    "authenticator",
    "recovery code",
    "recovery codes",
    "delete account",
    "close account",
    "deactivate account",
    "add card",
    "add payment method",
    "link bank",
    "connect bank",
    "create api key",
    "generate api key",
    "generate token",
];

const IRREVERSIBLE_WORDS: &[&str] = &[
    "send",
    "post",
    "publish",
    "tweet",
    "share",
    "reply",
    "comment",
    "submit",
    "confirm",
    "unsubscribe",
    "subscribe",
    "register",
    "signup",
    "book",
    "reserve",
    "approve",
    "finalize",
    "finalise",
    "upgrade",
];
const IRREVERSIBLE_PHRASES: &[&str] = &[
    "sign up",
    "create account",
    "apply now",
    "save changes",
    "save settings",
    "update settings",
    "change settings",
    "update profile",
    "submit application",
];

const BENIGN_PHRASES: &[&str] = &[
    "save draft",
    "save as draft",
    "save for later",
    "add to cart",
    "add to basket",
    "add to bag",
    "add to wishlist",
    "add to favorites",
    "add to favourites",
    "add to list",
    "filter",
    "filters",
    "apply filter",
    "apply filters",
    "search",
    "sort",
    "show more",
    "load more",
    "next",
    "previous",
    "continue",
    "back",
    "close",
    "dismiss",
    "cancel",
    "preview",
    "log in",
    "login",
    "sign in",
    "log out",
    "logout",
    "sign out",
    "accept cookies",
    "accept all",
    "reject all",
    "got it",
    "save",
    "update cart",
    "update quantity",
    "apply coupon",
    "apply code",
    "apply promo",
    "clear filters",
    "reset filters",
    "clear",
];

const DESTRUCTIVE_VERBS: &[&str] = &["delete", "remove", "erase", "destroy", "discard"];
const LOW_OBJECTS: &[&str] = &[
    "filter",
    "filters",
    "tag",
    "tags",
    "search",
    "searches",
    "draft",
    "drafts",
    "cart",
    "basket",
    "bag",
    "wishlist",
    "favorites",
    "favourites",
    "selection",
    "chip",
];
const HIGH_OBJECTS: &[&str] = &[
    "account",
    "profile",
    "card",
    "bank",
    "credentials",
    "password",
];
const CANCEL_OBJECTS: &[&str] = &[
    "order",
    "subscription",
    "booking",
    "reservation",
    "appointment",
    "membership",
    "plan",
    "trip",
    "flight",
    "account",
];

#[derive(Default)]
struct Hits {
    list: Vec<(RiskClass, String)>,
    uncertain: bool,
}

impl Hits {
    fn add(&mut self, c: RiskClass, why: impl Into<String>) {
        self.list.push((c, why.into()));
    }
    fn unsure(&mut self, c: RiskClass, why: impl Into<String>) {
        self.uncertain = true;
        self.list.push((c, why.into()));
    }
    fn max(&self) -> RiskClass {
        self.list
            .iter()
            .map(|h| h.0)
            .max()
            .unwrap_or(RiskClass::Read)
    }
    fn risky(&self) -> bool {
        self.max() >= RiskClass::Irreversible
    }
    fn finish(self) -> Classification {
        let class = self.max();
        let mut reasons: Vec<String> = Vec::new();
        for (_, r) in self.list {
            if !reasons.contains(&r) {
                reasons.push(r);
            }
        }
        Classification {
            class,
            reasons,
            uncertain: self.uncertain,
        }
    }
}

/// Classify the text of a control. Returns whether a benign phrase matched.
fn text_signals(text: &str, h: &mut Hits) -> bool {
    let w = words(&one_line(text, 300));
    if w.is_empty() {
        return false;
    }
    let mut risky = false;
    for p in MONEY_PHRASES {
        if has(&w, p) {
            h.add(RiskClass::MoneyIdentity, format!("label mentions \"{p}\""));
            risky = true;
        }
    }
    if has(&w, "subscribe")
        && (has_word_any(
            &w,
            &[
                "pay", "plan", "premium", "pro", "month", "monthly", "year", "yearly", "annual",
            ],
        ) || text.contains(['$', '€', '£']))
    {
        h.add(RiskClass::MoneyIdentity, "paid subscription");
        risky = true;
    }
    if has(&w, "upgrade") && !has(&w, "upgrade plan") {
        h.unsure(
            RiskClass::Irreversible,
            "label mentions \"upgrade\" (may involve payment)",
        );
    }
    for p in IRREVERSIBLE_PHRASES {
        if has(&w, p) {
            h.add(RiskClass::Irreversible, format!("label mentions \"{p}\""));
            risky = true;
        }
    }
    for p in IRREVERSIBLE_WORDS {
        if w.iter().any(|x| x == p) {
            h.add(RiskClass::Irreversible, format!("label mentions \"{p}\""));
            risky = true;
        }
    }
    if has(&w, "apply")
        && has_word_any(
            &w,
            &[
                "application",
                "job",
                "role",
                "position",
                "visa",
                "loan",
                "now",
            ],
        )
    {
        h.add(RiskClass::Irreversible, "label looks like an application");
        risky = true;
    }
    if has(&w, "accept")
        && has_word_any(&w, &["offer", "terms", "invitation", "invite", "agreement"])
    {
        h.add(
            RiskClass::Irreversible,
            "label accepts an agreement or offer",
        );
        risky = true;
    }
    if has(&w, "cancel") && has_word_any(&w, CANCEL_OBJECTS) {
        let cls = if has(&w, "cancel account") {
            RiskClass::MoneyIdentity
        } else {
            RiskClass::Irreversible
        };
        h.add(cls, "label cancels an order, booking, plan or account");
        risky = true;
    }
    for v in DESTRUCTIVE_VERBS {
        if !w.iter().any(|x| x == v) {
            continue;
        }
        risky = true;
        if has_word_any(&w, HIGH_OBJECTS) {
            h.add(
                RiskClass::MoneyIdentity,
                format!("\"{v}\" applied to an account, card or credential"),
            );
        } else if has_word_any(&w, LOW_OBJECTS) {
            h.add(
                RiskClass::ReversibleWrite,
                format!("\"{v}\" applied to a filter, draft or cart item"),
            );
            risky = false;
        } else if has_any(&w, &["permanently", "forever", "everything", "all data"])
            || *v != "remove"
        {
            h.add(RiskClass::Irreversible, format!("label mentions \"{v}\""));
        } else {
            h.unsure(
                RiskClass::Irreversible,
                "\"remove\" of an unrecognised object",
            );
        }
    }
    let benign = !risky && has_any(&w, BENIGN_PHRASES);
    if benign {
        h.add(RiskClass::ReversibleWrite, "label looks benign");
    }
    benign
}

fn path_words(url: &str) -> Vec<String> {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let path = if url.contains("://") || !url.starts_with('/') {
        rest.split_once('/').map_or("", |(_, p)| p)
    } else {
        rest
    };
    let path = path.split(['?', '#']).next().unwrap_or("");
    words(&one_line(path, 300))
}

const DESTRUCTIVE_PATH: &[&str] = &[
    "delete",
    "remove",
    "destroy",
    "unsubscribe",
    "cancel",
    "publish",
    "send",
    "transfer",
    "confirm",
    "submit",
    "post",
    "subscribe",
];
const MONEY_PATH: &[&str] = &["pay", "payment", "purchase", "charge", "withdraw", "donate"];
const IDENTITY_PATH: &[&str] = &["password", "passwd", "2fa", "mfa", "otp"];

fn field_signals(fields: &[FieldDesc], control_text: &str, h: &mut Hits) -> bool {
    let mut passwords = 0;
    let mut change = false;
    let mut money = false;
    let mut otp = false;
    for f in fields {
        let kind = f.kind.as_deref().unwrap_or("").to_lowercase();
        let w = words(&one_line(f.label.as_deref().unwrap_or(""), 100));
        if kind == "password" {
            passwords += 1;
        }
        if has_word_any(&w, &["password", "passcode"])
            && has_word_any(
                &w,
                &["new", "confirm", "current", "old", "repeat", "retype"],
            )
        {
            change = true;
        }
        if has_any(
            &w,
            &[
                "card number",
                "credit card",
                "debit card",
                "cvv",
                "cvc",
                "security code",
                "expiry",
                "expiration",
                "iban",
                "routing number",
                "account number",
                "sort code",
                "swift",
                "bank account",
                "card holder",
                "cardholder",
                "ssn",
                "social security",
                "passport number",
                "tax id",
            ],
        ) {
            money = true;
        }
        if has_any(
            &w,
            &[
                "one time",
                "otp",
                "verification code",
                "authentication code",
                "2fa",
                "two factor",
                "mfa",
            ],
        ) {
            otp = true;
        }
    }
    if money {
        h.add(RiskClass::MoneyIdentity, "payment or national-id field");
    }
    if otp {
        h.add(RiskClass::MoneyIdentity, "one-time code or 2FA field");
    }
    if change || passwords >= 2 {
        h.add(RiskClass::MoneyIdentity, "password change fields");
    }
    let mut login = false;
    if passwords == 1 && !change {
        let w = words(control_text);
        if has_any(&w, &["log in", "login", "sign in"]) {
            h.add(RiskClass::ReversibleWrite, "login form");
            login = true;
        } else {
            h.unsure(
                RiskClass::Irreversible,
                "password field with unrecognised submit label",
            );
        }
    }
    login
}

/// Classify an action. See the module docs for what this can and cannot do.
pub fn classify(a: &ActionDesc) -> Classification {
    let tool = a.tool.trim().to_lowercase();
    let mut h = Hits::default();
    if READ_TOOLS.contains(&tool.as_str()) || tool.starts_with("memory_") {
        h.add(RiskClass::Read, "read-only tool");
        return h.finish();
    }
    let click = CLICK_TOOLS.contains(&tool.as_str());
    let submit = tool == "form_submit";
    if WRITE_TOOLS.contains(&tool.as_str()) {
        h.add(
            RiskClass::ReversibleWrite,
            "typing or filling does not commit",
        );
    } else if submit {
        h.add(RiskClass::Irreversible, "form submit commits data");
    } else if !click {
        h.unsure(
            RiskClass::Irreversible,
            format!("unknown tool \"{}\"", one_line(&a.tool, 40)),
        );
    }

    let text = a.control_text.as_deref().unwrap_or("");
    let benign = text_signals(text, &mut h);
    let login = field_signals(&a.fields, text, &mut h);

    if let Some(fa) = a.form_action.as_deref() {
        let w = path_words(fa);
        if has_word_any(&w, DESTRUCTIVE_PATH) {
            h.add(
                RiskClass::Irreversible,
                "form action path looks destructive or external",
            );
        }
        if has_word_any(&w, MONEY_PATH) {
            h.add(
                RiskClass::MoneyIdentity,
                "form action path looks like payment",
            );
        }
        if has_word_any(&w, IDENTITY_PATH) {
            h.add(
                RiskClass::MoneyIdentity,
                "form action path looks like credentials",
            );
        }
        if has_word_any(&w, &["checkout", "order", "orders"]) {
            h.add(
                RiskClass::Irreversible,
                "form action path looks like checkout",
            );
        }
    }
    if let Some(u) = a.target_url.as_deref() {
        if click || submit {
            let w = path_words(u);
            if has_word_any(&w, DESTRUCTIVE_PATH) {
                h.unsure(
                    RiskClass::Irreversible,
                    "link path looks destructive (GET with side effects?)",
                );
            }
            if has_word_any(&w, MONEY_PATH) {
                h.add(RiskClass::ReversibleWrite, "link leads to a payment page");
            }
        }
    }

    let method = a.method.as_deref().map(|m| m.trim().to_uppercase());
    match method.as_deref() {
        None | Some("GET" | "HEAD" | "OPTIONS") => {}
        Some("DELETE") => h.add(RiskClass::Irreversible, "DELETE request"),
        Some(m) => {
            if click && benign && !h.risky() {
                h.add(
                    RiskClass::ReversibleWrite,
                    format!("{m} request with a benign label"),
                );
            } else if !submit {
                h.unsure(RiskClass::Irreversible, format!("{m} request"));
            }
        }
    }

    if click && h.list.is_empty() {
        h.unsure(
            RiskClass::ReversibleWrite,
            "no label, method or form information; assumed reversible",
        );
    }
    // Downgrade form submits only on positive evidence.
    if submit && !h.list.iter().any(|(c, _)| *c > RiskClass::Irreversible) {
        let get = matches!(method.as_deref(), Some("GET"));
        let other_risk = h
            .list
            .iter()
            .skip(1)
            .any(|(c, _)| *c >= RiskClass::Irreversible);
        if (get && !other_risk) || (login && !other_risk) {
            h.list.remove(0);
            h.add(
                RiskClass::ReversibleWrite,
                if login {
                    "login form"
                } else {
                    "GET form without risky labels"
                },
            );
        }
    }
    h.finish()
}

/// What to do with an action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Go ahead.
    Allow,
    /// Ask the user (name the action and the values) before proceeding.
    NeedsConfirm {
        /// Why.
        why: String,
    },
    /// Refuse.
    Deny {
        /// Why, including how to enable it when that is possible.
        why: String,
    },
}

/// Per-class override in a policy file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyAction {
    /// Always allow.
    Allow,
    /// Always ask.
    Confirm,
    /// Always refuse.
    Deny,
}

/// Domain mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Default behaviour per class.
    #[default]
    Normal,
    /// Nothing that commits: `irreversible` and `money_identity` are denied. Typing, filters and
    /// sign-in (`reversible_write`) are still allowed, because reading a portal needs them;
    /// tighten with `reversible_write = "deny"` or `"confirm"` if that is too loose.
    ReadOnly,
}

/// One `[domains."example.com"]` table.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainPolicy {
    /// `normal` (default) or `read_only`.
    #[serde(default)]
    pub mode: Mode,
    /// Classes enabled for this domain: listing `money_identity` lifts its default deny to
    /// "needs confirmation" (never to silent allow); listing `irreversible` lifts "needs
    /// confirmation" to allow. Ignored in `read_only` mode. Use an override for anything else.
    #[serde(default)]
    pub allow: Vec<RiskClass>,
    /// Override for `read`.
    #[serde(default)]
    pub read: Option<PolicyAction>,
    /// Override for `reversible_write`.
    #[serde(default)]
    pub reversible_write: Option<PolicyAction>,
    /// Override for `irreversible`.
    #[serde(default)]
    pub irreversible: Option<PolicyAction>,
    /// Override for `money_identity`.
    #[serde(default)]
    pub money_identity: Option<PolicyAction>,
}

impl DomainPolicy {
    fn override_for(&self, c: RiskClass) -> Option<PolicyAction> {
        match c {
            RiskClass::Read => self.read,
            RiskClass::ReversibleWrite => self.reversible_write,
            RiskClass::Irreversible => self.irreversible,
            RiskClass::MoneyIdentity => self.money_identity,
        }
    }
}

/// The `domains` part of `ostler.toml`.
///
/// ```toml
/// [default]                       # optional, for domains without an entry
/// mode = "normal"
///
/// [domains."bank.example"]
/// mode = "read_only"
///
/// [domains."shop.example"]
/// allow = ["irreversible"]        # no confirmation for ordinary submits
/// money_identity = "confirm"      # payments ask instead of being denied
/// ```
///
/// Unknown keys inside a domain table are rejected (a typo must not silently loosen a policy).
/// Domain lookup matches the host or any parent domain; the longest key wins.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    /// Applied to domains with no entry.
    #[serde(default)]
    pub default: Option<DomainPolicy>,
    /// Per-domain tables.
    #[serde(default)]
    pub domains: BTreeMap<String, DomainPolicy>,
}

fn host_of(s: &str) -> String {
    let s = s.trim().to_lowercase();
    let s = s.split_once("://").map_or(s.as_str(), |(_, r)| r);
    let s = s.split(['/', '?', '#']).next().unwrap_or("");
    let s = s.rsplit_once('@').map_or(s, |(_, h)| h);
    let host = if s.starts_with('[') {
        s.split_once(']').map_or(s, |(h, _)| h).to_string() + "]"
    } else {
        s.split(':').next().unwrap_or("").to_string()
    };
    host.trim_end_matches('.').to_string()
}

impl Policy {
    fn lookup(&self, domain: &str) -> Option<&DomainPolicy> {
        let host = host_of(domain);
        let mut cand = host.as_str();
        loop {
            for (k, v) in &self.domains {
                let k = k
                    .trim()
                    .trim_start_matches("*.")
                    .trim_start_matches('.')
                    .trim_end_matches('.');
                if k.eq_ignore_ascii_case(cand) {
                    return Some(v);
                }
            }
            match cand.split_once('.') {
                Some((_, rest)) if !rest.is_empty() => cand = rest,
                _ => return None,
            }
        }
    }

    /// Human-readable warnings about contradictory settings (e.g. `allow` under `read_only`).
    pub fn warnings(&self) -> Vec<String> {
        let mut w = Vec::new();
        for (d, p) in &self.domains {
            if p.mode == Mode::ReadOnly && !p.allow.is_empty() {
                w.push(format!(
                    "domains.\"{d}\": `allow` is ignored in read_only mode"
                ));
            }
            if p.allow.contains(&RiskClass::MoneyIdentity) {
                w.push(format!("domains.\"{d}\": money_identity enabled; actions will still ask for confirmation"));
            }
        }
        w
    }
}

/// Decide what to do with an action of `class` on `domain` (a host or a URL).
pub fn decide(policy: &Policy, domain: &str, class: RiskClass) -> Decision {
    let host = host_of(domain);
    let dp = policy.lookup(domain).or(policy.default.as_ref());
    let empty = DomainPolicy::default();
    let dp = dp.unwrap_or(&empty);
    if let Some(a) = dp.override_for(class) {
        return match a {
            PolicyAction::Allow => Decision::Allow,
            PolicyAction::Confirm => Decision::NeedsConfirm {
                why: format!(
                    "policy for {host} requires confirmation of {} actions",
                    class.name()
                ),
            },
            PolicyAction::Deny => Decision::Deny {
                why: format!("policy for {host} denies {} actions", class.name()),
            },
        };
    }
    let read_only = dp.mode == Mode::ReadOnly;
    match class {
        RiskClass::Read | RiskClass::ReversibleWrite => Decision::Allow,
        RiskClass::Irreversible if read_only => Decision::Deny {
            why: format!("{host} is read-only; irreversible actions are denied"),
        },
        RiskClass::MoneyIdentity if read_only => Decision::Deny {
            why: format!("{host} is read-only; money and identity actions are denied"),
        },
        RiskClass::Irreversible => {
            if dp.allow.contains(&RiskClass::Irreversible) {
                Decision::Allow
            } else {
                Decision::NeedsConfirm {
                    why: "irreversible or external action; confirm it and the values first".into(),
                }
            }
        }
        RiskClass::MoneyIdentity => {
            if dp.allow.contains(&RiskClass::MoneyIdentity) {
                Decision::NeedsConfirm {
                    why: format!(
                        "money or identity action on {host} (enabled by policy); confirm it first"
                    ),
                }
            } else {
                Decision::Deny {
                    why: format!(
                        "money or identity actions are denied on {host}; enable with [domains.\"{host}\"] allow = [\"money_identity\"]"
                    ),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn click(text: &str) -> ActionDesc {
        ActionDesc {
            tool: "click".into(),
            control_text: Some(text.into()),
            ..Default::default()
        }
    }

    fn cls(text: &str) -> RiskClass {
        classify(&click(text)).class
    }

    #[test]
    fn money_labels() {
        for t in [
            "Pay now",
            "Pay later",
            "Buy",
            "Buy now",
            "Place your order",
            "Purchase",
            "Transfer",
            "Send money",
            "Change password",
            "Enable 2FA",
            "Set up two-factor authentication",
            "Delete account",
            "Donate $5",
            "Confirm payment",
            "Start free trial",
        ] {
            assert_eq!(cls(t), RiskClass::MoneyIdentity, "{t}");
        }
    }

    #[test]
    fn pay_later_is_not_waved_through() {
        let c = classify(&click("Pay later"));
        assert_eq!(c.class, RiskClass::MoneyIdentity);
    }

    #[test]
    fn irreversible_labels() {
        for t in [
            "Submit",
            "Send a message",
            "Send feedback",
            "Post comment",
            "Publish",
            "Delete",
            "Confirm",
            "Subscribe",
            "Unsubscribe",
            "Sign up",
            "Save changes",
            "Apply now",
            "Book",
            "Cancel order",
            "Remove permanently",
        ] {
            assert!(cls(t) >= RiskClass::Irreversible, "{t} -> {:?}", cls(t));
        }
        assert_eq!(cls("Send money"), RiskClass::MoneyIdentity);
        assert!(cls("Send money") > cls("Send a message"));
    }

    #[test]
    fn benign_labels() {
        for t in [
            "Save draft",
            "Save as draft",
            "Add to cart",
            "Filter",
            "Apply filters",
            "Search",
            "Next",
            "Cancel",
            "Sign in",
            "Show more",
            "Save",
        ] {
            assert_eq!(cls(t), RiskClass::ReversibleWrite, "{t}");
        }
    }

    #[test]
    fn delete_depends_on_object() {
        assert_eq!(cls("Delete filter"), RiskClass::ReversibleWrite);
        assert_eq!(cls("Remove from cart"), RiskClass::ReversibleWrite);
        assert_eq!(cls("Delete draft"), RiskClass::ReversibleWrite);
        assert_eq!(cls("Delete account"), RiskClass::MoneyIdentity);
        assert_eq!(cls("Delete my profile"), RiskClass::MoneyIdentity);
        assert_eq!(cls("Delete project"), RiskClass::Irreversible);
        assert_eq!(cls("Delete"), RiskClass::Irreversible);
        let r = classify(&click("Remove"));
        assert_eq!(r.class, RiskClass::Irreversible);
        assert!(r.uncertain);
        // a risky object always wins over a benign one
        assert_eq!(cls("Delete filter and account"), RiskClass::MoneyIdentity);
    }

    #[test]
    fn attacker_cannot_lower_with_benign_words() {
        assert_eq!(cls("Save draft and pay"), RiskClass::MoneyIdentity);
        assert_eq!(cls("Search... Submit"), RiskClass::Irreversible);
        assert_eq!(cls("Cancel order"), RiskClass::Irreversible);
    }

    #[test]
    fn whole_word_matching() {
        assert_eq!(
            cls("Postcode lookup"),
            RiskClass::ReversibleWrite,
            "unrecognised click is assumed reversible"
        );
        assert!(classify(&click("Postcode lookup")).uncertain);
        assert_eq!(cls("Payment methods overview"), RiskClass::ReversibleWrite);
        assert_eq!(
            cls("Don't pay"),
            RiskClass::MoneyIdentity,
            "negation is not understood; stricter"
        );
    }

    #[test]
    fn read_tools() {
        for t in ["navigate", "snapshot", "extract", "memory_recall"] {
            let c = classify(&ActionDesc {
                tool: t.into(),
                control_text: Some("Delete account".into()),
                ..Default::default()
            });
            assert_eq!(c.class, RiskClass::Read, "{t}");
        }
    }

    #[test]
    fn typing_and_unknown_tools() {
        let c = classify(&ActionDesc {
            tool: "type".into(),
            ..Default::default()
        });
        assert_eq!(c.class, RiskClass::ReversibleWrite);
        let c = classify(&ActionDesc {
            tool: "frobnicate".into(),
            ..Default::default()
        });
        assert_eq!(c.class, RiskClass::Irreversible);
        assert!(c.uncertain);
    }

    #[test]
    fn methods() {
        let mut a = click("Continue");
        a.method = Some("post".into());
        assert_eq!(
            classify(&a).class,
            RiskClass::ReversibleWrite,
            "benign label with POST"
        );
        a.control_text = Some("Go".into());
        let c = classify(&a);
        assert_eq!(c.class, RiskClass::Irreversible);
        assert!(c.uncertain);
        a.control_text = Some("Continue".into());
        a.method = Some("DELETE".into());
        assert_eq!(classify(&a).class, RiskClass::Irreversible);
        a.method = Some("GET".into());
        a.control_text = Some("Go".into());
        assert_eq!(classify(&a).class, RiskClass::ReversibleWrite);
    }

    #[test]
    fn form_submit_defaults_and_downgrades() {
        let f = |kind: &str, label: &str| FieldDesc {
            kind: Some(kind.into()),
            label: Some(label.into()),
        };
        let mut a = ActionDesc {
            tool: "form_submit".into(),
            ..Default::default()
        };
        assert_eq!(classify(&a).class, RiskClass::Irreversible);
        a.method = Some("GET".into());
        a.control_text = Some("Search".into());
        assert_eq!(
            classify(&a).class,
            RiskClass::ReversibleWrite,
            "GET search form"
        );
        a.method = Some("POST".into());
        assert_eq!(
            classify(&a).class,
            RiskClass::Irreversible,
            "POST search stays strict"
        );
        // login
        let login = ActionDesc {
            tool: "form_submit".into(),
            method: Some("POST".into()),
            control_text: Some("Sign in".into()),
            fields: vec![f("email", "Email"), f("password", "Password")],
            ..Default::default()
        };
        assert_eq!(classify(&login).class, RiskClass::ReversibleWrite);
        // change password
        let chg = ActionDesc {
            fields: vec![
                f("password", "Current password"),
                f("password", "New password"),
            ],
            control_text: Some("Save".into()),
            ..login.clone()
        };
        assert_eq!(classify(&chg).class, RiskClass::MoneyIdentity);
        // card fields
        let pay = ActionDesc {
            fields: vec![f("text", "Card number"), f("text", "CVV")],
            control_text: Some("Continue".into()),
            ..login.clone()
        };
        assert_eq!(classify(&pay).class, RiskClass::MoneyIdentity);
        // otp
        let otp = ActionDesc {
            fields: vec![f("text", "Verification code")],
            control_text: Some("Verify".into()),
            ..login.clone()
        };
        assert_eq!(classify(&otp).class, RiskClass::MoneyIdentity);
        // lone password with odd label is stricter and unsure
        let odd = ActionDesc {
            control_text: Some("Go".into()),
            fields: vec![f("password", "Password")],
            ..login
        };
        let c = classify(&odd);
        assert_eq!(c.class, RiskClass::Irreversible);
        assert!(c.uncertain);
    }

    #[test]
    fn form_action_paths() {
        let a = ActionDesc {
            tool: "form_submit".into(),
            method: Some("POST".into()),
            form_action: Some("https://shop.test/cart/payment/charge".into()),
            ..Default::default()
        };
        assert_eq!(classify(&a).class, RiskClass::MoneyIdentity);
        let a = ActionDesc {
            form_action: Some("/account/password".into()),
            ..a
        };
        assert_eq!(classify(&a).class, RiskClass::MoneyIdentity);
        let a = ActionDesc {
            form_action: Some("/items/42/delete".into()),
            ..a
        };
        assert_eq!(classify(&a).class, RiskClass::Irreversible);
        // host names do not count as path words
        let a = ActionDesc {
            form_action: Some("https://pay.example.com/".into()),
            method: Some("GET".into()),
            control_text: Some("Search".into()),
            ..a
        };
        assert_eq!(classify(&a).class, RiskClass::ReversibleWrite);
    }

    #[test]
    fn link_targets() {
        let a = ActionDesc {
            tool: "click".into(),
            target_url: Some("https://x.test/unsubscribe?u=1".into()),
            control_text: Some("Here".into()),
            ..Default::default()
        };
        let c = classify(&a);
        assert_eq!(c.class, RiskClass::Irreversible);
        assert!(c.uncertain);
        let a = ActionDesc {
            target_url: Some("https://x.test/checkout/payment".into()),
            control_text: Some("Go".into()),
            ..a
        };
        assert_eq!(classify(&a).class, RiskClass::ReversibleWrite);
    }

    #[test]
    fn long_and_odd_input_is_safe() {
        let t = "x ".repeat(10_000) + "pay";
        // truncated before analysis; must not panic
        let _ = cls(&t);
        let _ = cls("");
        let _ = cls("💳 Pay ✓");
        assert_eq!(cls("💳 Pay ✓"), RiskClass::MoneyIdentity);
    }

    #[test]
    fn summary_text() {
        let s = classify(&click("Remove")).summary();
        assert!(
            s.starts_with("irreversible:") && s.contains("unsure"),
            "{s}"
        );
    }

    fn policy() -> Policy {
        toml::from_str(
            r#"
            [default]
            mode = "normal"

            [domains."bank.example"]
            mode = "read_only"

            [domains."shop.example"]
            allow = ["irreversible", "money_identity"]

            [domains."docs.shop.example"]
            irreversible = "deny"
            money_identity = "allow"
            "#,
        )
        .unwrap()
    }

    #[test]
    fn defaults() {
        let p = Policy::default();
        assert_eq!(decide(&p, "a.test", RiskClass::Read), Decision::Allow);
        assert_eq!(
            decide(&p, "a.test", RiskClass::ReversibleWrite),
            Decision::Allow
        );
        assert!(matches!(
            decide(&p, "a.test", RiskClass::Irreversible),
            Decision::NeedsConfirm { .. }
        ));
        match decide(&p, "a.test", RiskClass::MoneyIdentity) {
            Decision::Deny { why } => assert!(why.contains("allow = [\"money_identity\"]")),
            d => panic!("{d:?}"),
        }
    }

    #[test]
    fn toml_policy() {
        let p = policy();
        assert_eq!(p.domains.len(), 3);
        assert!(matches!(
            decide(&p, "bank.example", RiskClass::Irreversible),
            Decision::Deny { .. }
        ));
        assert!(matches!(
            decide(
                &p,
                "https://www.bank.example/login",
                RiskClass::MoneyIdentity
            ),
            Decision::Deny { .. }
        ));
        assert_eq!(
            decide(&p, "bank.example", RiskClass::ReversibleWrite),
            Decision::Allow
        );
        assert_eq!(
            decide(&p, "shop.example", RiskClass::Irreversible),
            Decision::Allow
        );
        assert!(
            matches!(
                decide(&p, "shop.example", RiskClass::MoneyIdentity),
                Decision::NeedsConfirm { .. }
            ),
            "enabled money still confirms"
        );
        // longest match wins, overrides beat allow
        assert!(matches!(
            decide(&p, "docs.shop.example", RiskClass::Irreversible),
            Decision::Deny { .. }
        ));
        assert_eq!(
            decide(&p, "docs.shop.example", RiskClass::MoneyIdentity),
            Decision::Allow
        );
        // unrelated suffix does not match
        assert!(matches!(
            decide(&p, "evilshop.example", RiskClass::Irreversible),
            Decision::NeedsConfirm { .. }
        ));
        assert!(matches!(
            decide(&p, "SHOP.EXAMPLE:8443", RiskClass::Irreversible),
            Decision::Allow
        ));
    }

    #[test]
    fn read_only_ignores_allow_and_warns() {
        let p: Policy = toml::from_str(
            "[domains.\"b.test\"]\nmode = \"read_only\"\nallow = [\"money_identity\"]\n",
        )
        .unwrap();
        assert!(matches!(
            decide(&p, "b.test", RiskClass::MoneyIdentity),
            Decision::Deny { .. }
        ));
        assert!(!p.warnings().is_empty());
    }

    #[test]
    fn explicit_override_can_tighten_and_loosen() {
        let p: Policy = toml::from_str(
            "[domains.\"b.test\"]\nmode = \"read_only\"\nreversible_write = \"confirm\"\nirreversible = \"allow\"\n",
        )
        .unwrap();
        assert!(matches!(
            decide(&p, "b.test", RiskClass::ReversibleWrite),
            Decision::NeedsConfirm { .. }
        ));
        assert_eq!(
            decide(&p, "b.test", RiskClass::Irreversible),
            Decision::Allow
        );
    }

    #[test]
    fn default_table_applies_to_unlisted() {
        let p: Policy = toml::from_str("[default]\nmode = \"read_only\"\n").unwrap();
        assert!(matches!(
            decide(&p, "anything.test", RiskClass::Irreversible),
            Decision::Deny { .. }
        ));
    }

    #[test]
    fn typos_in_domain_tables_are_rejected() {
        assert!(toml::from_str::<Policy>("[domains.\"a.test\"]\nmod = \"read_only\"\n").is_err());
        assert!(toml::from_str::<Policy>("[domains.\"a.test\"]\nmode = \"readonly\"\n").is_err());
        assert!(
            toml::from_str::<Policy>("[domains.\"a.test\"]\nirreversible = \"maybe\"\n").is_err()
        );
    }

    #[test]
    fn host_normalisation() {
        assert_eq!(
            host_of("https://user:pw@Sub.Example.com:8443/a?b#c"),
            "sub.example.com"
        );
        assert_eq!(host_of("example.com."), "example.com");
        assert_eq!(host_of("[::1]:80"), "[::1]");
        assert_eq!(
            path_words("https://pay.example.com/a/Delete-it?x=pay"),
            vec!["a", "delete", "it"]
        );
    }
}
