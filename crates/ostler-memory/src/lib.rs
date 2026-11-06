//! Agent memory on Qdrant.
//!
//! Three kinds of memory, one collection each, all multi-tenant by a `tenant`
//! payload field (indexed with `is_tenant`, as Qdrant recommends):
//!
//! * **profile**: facts about the user ("email", "company") used to fill fields.
//! * **playbooks**: what worked on a given form, as structure only (never values).
//! * **episodes**: what went wrong on a form and why.
//!
//! Secrets never go in: [`looks_sensitive`] refuses them at the API boundary.
//!
//! ## The learning loop
//!
//! Playbooks are Ostler's *skills*: procedural memory distilled from fills that the page accepted.
//! Every outcome patches the skill instead of rewriting it ([`Memory::record_outcome`]):
//!
//! 1. **Recall.** Exact form fingerprint first; if the form was redesigned or is new, a playbook from
//!    a *similar* form on the same site that has a field with the same label ([`Memory::suggest`],
//!    via `similar_form`). Candidates are ranked inside Qdrant by a score-boosting formula over
//!    similarity, smoothed win rate and recency ([`Rank`]).
//! 2. **Verify.** The browser reads every value back; only verified, accepted fills count as wins.
//! 3. **Reinforce or demote.** Wins and losses are kept per field mapping and per playbook. A mapping
//!    that loses more than it wins is dropped; the field falls back to safer signals.
//! 4. **Forget.** [`Memory::prune`] removes stale, unproven skills and old episodes.

mod embed;
pub mod fusion;
mod local;
mod qdrant;
mod sealed;
mod source;
pub mod sparse;
mod store;
mod token;

pub use embed::{Embedder, HashEmbedder, OllamaEmbedder};
pub use local::LocalStore;
pub use qdrant::{QdrantConfig, QdrantStore, VectorSource, MIN_HYBRID_VERSION};
pub use sealed::{
    migrate_sealed, probe_key, seal_legacy, write_v1_point_for_tests, KeyProbe, MigrateReport,
    SealKey, SealReport, SealedStore,
};
pub use source::{
    SaveSourcesReport, SourceChunk, SourceHit, SourceRecallOptions, SourceStats, SourceSummary,
    DEFAULT_SOURCE_RETAIN_DAYS, MAX_CHUNKS_PER_URL, MAX_CHUNK_CHARS,
};
pub use sparse::{SparseVector, TermHasher};
pub use store::{Filter, Hit, HybridHit, HybridQuery, InMemoryStore, Kind, Rank, Store};
pub use token::{mint_tenant_token, tenant_prefix};

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("qdrant request failed: {0}")]
    Http(String),
    #[error("qdrant returned {status}: {body}")]
    Status { status: u16, body: String },
    #[error("{0}")]
    Invalid(String),
    #[error("embedding failed: {0}")]
    Embed(String),
    #[error("qdrant is unavailable (recent failures: {0}); memory is skipped until it recovers")]
    Unavailable(String),
}

pub type Result<T> = std::result::Result<T, MemoryError>;

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Stable point id from arbitrary parts, so re-saving the same thing is an upsert.
///
/// Parts are joined with U+001F. If a part itself contains that character the join is ambiguous
/// (`["a\x1fb", "c"]` and `["a", "b\x1fc"]` would collide, which across tenants means one tenant overwriting
/// another's point), so such input is length-prefixed instead. Ordinary input keeps the original encoding,
/// so ids already stored stay valid.
fn point_id(parts: &[&str]) -> String {
    let joined = if parts.iter().any(|p| p.contains('\u{1f}')) {
        parts
            .iter()
            .map(|p| format!("{}:{p}", p.len()))
            .collect::<Vec<_>>()
            .join("\u{1e}")
    } else {
        parts.join("\u{1f}")
    };
    Uuid::new_v5(&Uuid::NAMESPACE_OID, joined.as_bytes()).to_string()
}

/// Every point matching `filter`, however many there are. A plain `scroll` silently stops at its limit, which
/// for `prune` or a key probe means "everything" quietly becomes "the first page".
pub(crate) async fn scroll_all(store: &dyn Store, kind: Kind, filter: &Filter) -> Result<Vec<Hit>> {
    const START: usize = 10_000;
    const MAX: usize = 4_000_000;
    let mut limit = START;
    loop {
        let hits = store.scroll(kind, filter, limit).await?;
        if hits.len() < limit {
            return Ok(hits);
        }
        if limit >= MAX {
            return Err(MemoryError::Invalid(format!(
                "more than {MAX} points in {}: refusing to act on a partial view",
                kind.collection()
            )));
        }
        limit = (limit * 4).min(MAX);
    }
}

/// Read-modify-write on one point (episode counters, playbook wins and losses) must not interleave with
/// another in the same process, or both read the same count and one increment is lost. Striped by id so
/// unrelated points rarely wait for each other. Other processes sharing a server are not covered: counters
/// are exact within a process and best-effort across them.
fn stripe(id: &str) -> &'static tokio::sync::Mutex<()> {
    use std::sync::OnceLock;
    const N: usize = 64;
    static LOCKS: OnceLock<Vec<tokio::sync::Mutex<()>>> = OnceLock::new();
    let locks = LOCKS.get_or_init(|| (0..N).map(|_| tokio::sync::Mutex::new(())).collect());
    let h = id.bytes().fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    });
    &locks[(h % N as u64) as usize]
}

/// Keys/values we refuse to persist or auto-fill.
pub fn looks_sensitive(key: &str, value: &str) -> bool {
    key_looks_sensitive(key) || contains_card_number(value)
}

fn key_looks_sensitive(key: &str) -> bool {
    let k = key.to_lowercase();
    const KEYS: &[&str] = &[
        "password",
        "passcode",
        "secret",
        "token",
        "api_key",
        "apikey",
        "cvv",
        "cvc",
        "card",
        "ssn",
        "social_security",
        "pin",
        "otp",
        "iban",
        "routing",
        "account_number",
        "private_key",
    ];
    KEYS.iter().any(|m| k.contains(m))
}

/// Like [`looks_sensitive`] for a JSON value: every nested key and every string or number inside it is checked,
/// so a card number cannot get in by being wrapped in an object or array.
fn value_looks_sensitive(v: &Value) -> bool {
    match v {
        Value::String(s) => contains_card_number(s),
        Value::Number(n) => contains_card_number(&n.to_string()),
        Value::Array(a) => a.iter().any(value_looks_sensitive),
        Value::Object(o) => o
            .iter()
            .any(|(k, v)| key_looks_sensitive(k) || value_looks_sensitive(v)),
        _ => false,
    }
}

/// A 13-19 digit number in any script, written plainly or grouped by spaces, dashes, dots or underscores.
/// A value that is nothing but such a number is refused outright; inside longer text it must also pass the
/// Luhn check, so "order 1234567890123" is not mistaken for a card but "my card is 4111 1111 1111 1111" is.
fn contains_card_number(s: &str) -> bool {
    let chars: Vec<char> = s.chars().filter(|c| !invisible(*c)).collect();
    let whole = !chars.iter().any(|c| c.is_alphabetic());
    // A run is digits with any punctuation or spaces in between ("4111 - 1111,1111/1111"); letters end it.
    let mut run: Vec<u32> = vec![];
    for c in chars.iter().copied().chain(std::iter::once('a')) {
        if c.is_numeric() {
            run.push(c.to_digit(10).unwrap_or(0));
        } else if c.is_alphabetic() {
            // A value that is nothing but digits and punctuation is refused when it holds a card-length
            // run, whatever its checksum (this also catches two cards, or 23 digits, written together).
            if run.len() >= 13 && whole {
                return true;
            }
            // Inside longer text a window must also pass Luhn, so "order 1234567890123" is not a card.
            for len in 13..=19usize.min(run.len()) {
                if run.windows(len).any(luhn) {
                    return true;
                }
            }
            run.clear();
        }
    }
    false
}

fn luhn(digits: &[u32]) -> bool {
    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(i, d)| {
            if i % 2 == 1 {
                let x = d * 2;
                if x > 9 {
                    x - 9
                } else {
                    x
                }
            } else {
                *d
            }
        })
        .sum();
    matches!(sum % 10, 0)
}

/// Mask personal data a page may echo back in an error message ("ada@x.com is not valid").
/// Defence in depth: callers already scrub the values they typed, but this runs for every episode
/// before it is embedded (cloud inference) or stored, so no caller can leak by forgetting.
/// Masks email addresses, runs of 4+ digits (phones, ZIP/PIN codes, card and ID numbers) and URL queries.
pub fn redact_detail(s: &str) -> String {
    // Masking a number can swallow the whitespace between two words (`98765 -43210`), which changes how the
    // next pass tokenises. Run to a fixed point so that redacting twice is always the same as once.
    let mut cur = redact_once(s);
    for _ in 0..8 {
        let next = redact_once(&cur);
        if next == cur {
            break;
        }
        cur = next;
    }
    cur
}

fn redact_once(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        if word.is_empty() {
            return;
        }
        let w = word.as_str();
        // A query string or fragment on anything URL-shaped (with or without a scheme) can carry tokens.
        let urlish = w.contains("://") || w.contains('.') || w.contains('/');
        let cut = if urlish { w.find(['?', '#']) } else { None }.filter(|i| *i > 0);
        // Judge the email check on what survives the cut, so `ada@x.com?utm=1` is still an email while
        // an address that only appears inside a dropped query is simply dropped with it.
        // Our own `?[query]` marker is not page text: ignoring it keeps a second pass identical to the first.
        let head = cut.map_or_else(|| w.strip_suffix("?[query]").unwrap_or(w), |i| &w[..i]);
        if email_like(head) {
            out.push_str("[email]");
        } else if let Some(i) = cut {
            out.push_str(&w[..i]);
            out.push_str("?[query]");
        } else {
            out.push_str(w);
        }
        word.clear();
    };
    for c in s.chars() {
        if c.is_whitespace() {
            flush(&mut word, &mut out);
            out.push(c);
        } else {
            word.push(c);
        }
    }
    flush(&mut word, &mut out);
    mask_digit_runs(&out)
}

/// `@`, its fullwidth form and the small-commercial-at: all read as an address separator.
fn is_at(c: char) -> bool {
    matches!(c, '@' | '\u{ff20}' | '\u{fe6b}')
}

/// Any `@` that is not the first character and has something alphanumeric after it. Looks at every
/// `@`, so a leading one (`@ada@x.com`) cannot hide a later one.
fn email_like(w: &str) -> bool {
    w.char_indices()
        .any(|(i, c)| is_at(c) && i > 0 && w[i + c.len_utf8()..].chars().any(char::is_alphanumeric))
}

/// Characters that render as nothing (zero-width, bidi controls, soft hyphen, variation selectors,
/// combining marks). They must not be usable to cut a number in two.
fn invisible(c: char) -> bool {
    matches!(c,
        '\u{ad}' | '\u{34f}' | '\u{300}'..='\u{36f}' | '\u{61c}' | '\u{180e}'
        | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}'
        | '\u{2066}'..='\u{2069}' | '\u{fe00}'..='\u{fe0f}' | '\u{feff}')
}

/// Replace runs of 4+ digits (any script) with `[number]`. A run may span one or two separator characters
/// between digits, so phone numbers, dates and grouped card numbers are caught: `(555) 123-4567`, `12/05/1990`.
/// Invisible characters inside a run are transparent.
fn mask_digit_runs(s: &str) -> String {
    const SEP: &[char] = &[' ', '-', '.', '/', '(', ')'];
    let chars: Vec<char> = s.chars().collect();
    let mut res = String::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_numeric() {
            res.push(chars[i]);
            i += 1;
            continue;
        }
        let (mut j, mut digits, mut end) = (i, 0, i);
        loop {
            while j < chars.len() && (chars[j].is_numeric() || invisible(chars[j])) {
                if chars[j].is_numeric() {
                    digits += 1;
                    end = j + 1;
                }
                j += 1;
            }
            // Look past up to two separators (and any invisibles) for another digit.
            let (mut k, mut seps) = (j, 0);
            while k < chars.len() {
                if invisible(chars[k]) {
                    k += 1;
                } else if seps < 2 && SEP.contains(&chars[k]) {
                    seps += 1;
                    k += 1;
                } else {
                    break;
                }
            }
            if k > j && k < chars.len() && chars[k].is_numeric() {
                j = k;
            } else {
                break;
            }
        }
        if digits >= 4 {
            res.push_str("[number]");
        } else {
            res.extend(&chars[i..end]);
        }
        i = end;
    }
    res
}

/// Conventional `autocomplete` tokens for common profile keys.
pub fn default_autocomplete(key: &str) -> Option<&'static str> {
    Some(match key.to_lowercase().replace([' ', '-'], "_").as_str() {
        "name" | "full_name" => "name",
        "first_name" | "given_name" | "firstname" => "given-name",
        "last_name" | "family_name" | "surname" | "lastname" => "family-name",
        "email" | "email_address" => "email",
        "phone" | "phone_number" | "mobile" | "tel" => "tel",
        "address" | "street" | "street_address" | "address_line1" => "street-address",
        "city" | "town" => "address-level2",
        "state" | "region" | "province" => "address-level1",
        "zip" | "zipcode" | "zip_code" | "postal_code" | "postcode" => "postal-code",
        "country" => "country-name",
        "company" | "organization" | "organisation" | "employer" => "organization",
        "job_title" | "title" => "organization-title",
        "username" => "username",
        "birthday" | "birthdate" | "date_of_birth" | "dob" => "bday",
        "website" | "url" => "url",
        _ => return None,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fact {
    pub key: String,
    pub value: Value,
    pub description: Option<String>,
    pub autocomplete: Option<String>,
    pub source: String,
    pub updated_at: u64,
}

impl Fact {
    fn from_payload(p: &Value) -> Option<Self> {
        Some(Self {
            key: p["key"].as_str()?.to_string(),
            value: p["value"].clone(),
            description: p["description"].as_str().map(String::from),
            autocomplete: p["autocomplete"].as_str().map(String::from),
            source: p["source"].as_str().unwrap_or("user").to_string(),
            updated_at: p["updated_at"].as_u64().unwrap_or(0),
        })
    }
}

#[derive(Debug, Clone)]
pub struct FieldQuery<'a> {
    pub host: &'a str,
    pub fingerprint: &'a str,
    pub label: &'a str,
    pub name: Option<&'a str>,
    pub kind: &'a str,
    pub placeholder: Option<&'a str>,
    pub autocomplete: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub struct Recalled {
    pub fact: Fact,
    pub confidence: f32,
    /// How we found it: "playbook", "autocomplete", "similar_form" or "semantic".
    pub via: &'static str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybookField {
    pub label: String,
    pub kind: String,
    /// Profile key that supplied this field's value last time, if any.
    pub profile_key: Option<String>,
    pub required: bool,
    /// Verified, accepted fills of this field through this playbook.
    #[serde(default)]
    pub wins: u32,
    /// Fills of this field that failed verification or were rejected by the page.
    #[serde(default)]
    pub losses: u32,
}

impl PlaybookField {
    pub fn new(
        label: impl Into<String>,
        kind: impl Into<String>,
        profile_key: Option<String>,
    ) -> Self {
        Self {
            label: label.into(),
            kind: kind.into(),
            profile_key,
            required: false,
            wins: 0,
            losses: 0,
        }
    }
    /// A mapping is trusted while it has not lost more often than it won.
    pub fn trusted(&self) -> bool {
        self.profile_key.is_some() && self.losses <= self.wins
    }
}

/// What happened to one field in one fill, as reported by the browser's read-back.
#[derive(Debug, Clone)]
pub struct FieldOutcome {
    pub label: String,
    pub kind: String,
    /// The profile fact that filled it, if memory supplied the value.
    pub profile_key: Option<String>,
    /// Verified (or legitimately reformatted) and not flagged by the page.
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Playbook {
    pub host: String,
    pub fingerprint: String,
    pub headings: Vec<String>,
    pub fields: Vec<PlaybookField>,
    pub uses: u64,
    pub last_success: u64,
    /// Accepted submits through this playbook.
    #[serde(default)]
    pub wins: u64,
    /// Rejected submits or failed fills.
    #[serde(default)]
    pub losses: u64,
    /// Last time the page accepted it (seconds since epoch).
    #[serde(default)]
    pub verified_ts: u64,
    /// Fingerprint of the playbook this one was first learned from, when knowledge transferred
    /// from a similar form (a redesign, or a sibling form on the same site).
    #[serde(default)]
    pub learned_from: Option<String>,
}

impl Playbook {
    pub fn win_rate(&self) -> f32 {
        (self.wins as f32 + 1.0) / ((self.wins + self.losses) as f32 + 2.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Episode {
    pub host: String,
    pub fingerprint: String,
    pub field: String,
    pub problem: String,
    pub detail: String,
    pub count: u64,
    pub last_seen: u64,
}

/// Minimum lead the best semantic hit needs over the second best.
const SEMANTIC_MARGIN: f32 = 0.05;

/// A skill not re-verified for this long counts half as much in ranking.
const HALF_LIFE_SECS: u64 = 30 * 24 * 3600;

/// Episodes (page problems seen while filling) are diagnostics, not knowledge: they expire after this
/// long without being seen again.
pub const EPISODE_TTL_SECS: u64 = 30 * 24 * 3600;

/// Most episodes kept per tenant. When a new one would exceed it the least recently seen are dropped, so
/// the collection cannot grow without bound however many pages misbehave.
pub const MAX_EPISODES_PER_TENANT: usize = 500;

/// Ranking used for learned records: similarity first, then evidence, then freshness.
fn rank(min_similarity: f32) -> Rank {
    Rank {
        now: now(),
        success_weight: 0.3,
        recency_weight: 0.15,
        half_life_secs: HALF_LIFE_SECS,
        min_similarity,
    }
}

fn structure_text(host: &str, headings: &[String], labels: &[&str]) -> String {
    format!("{host} {} {}", headings.join(" "), labels.join(", "))
}

fn mentions_other_person(label_norm: &str) -> bool {
    const OTHERS: &[&str] = &[
        "spouse",
        "partner",
        "friend",
        "referr",
        "emergency",
        "reference",
        "guardian",
        "parent",
        "mother",
        "father",
        "colleague",
        "manager",
        "supervisor",
        "beneficiary",
        "relative",
        "next of kin",
        "recipient",
        "sponsor",
    ];
    OTHERS.iter().any(|w| label_norm.contains(w))
}

/// Combining marks. They belong to the letter before them, so they must not split a word: `cafe` + U+0301 is
/// a different word from `cafe`, not the same one.
fn combining(c: char) -> bool {
    matches!(c, '\u{300}'..='\u{36f}' | '\u{1ab0}'..='\u{1aff}' | '\u{1dc0}'..='\u{1dff}'
        | '\u{20d0}'..='\u{20ff}' | '\u{fe20}'..='\u{fe2f}')
}

/// Lower-cased words of `s`, single-spaced: "Work-Email", "work_email" and " WORK  email " are one key. A
/// string with no letters or digits at all ("😀", "***") would normalise to nothing and every such key would
/// collapse into one, so it keeps its own (lower-cased, trimmed) text behind a `~`, which a real word never has.
fn norm(s: &str) -> String {
    let words = s
        .to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || combining(c)))
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if words.is_empty() && !s.trim().is_empty() {
        return format!("~{}", s.trim().to_lowercase());
    }
    words
}

/// High-level memory API for one tenant.
#[derive(Clone)]
pub struct Memory {
    store: Arc<dyn Store>,
    pub tenant: String,
    /// Days a fetched page stays in source memory; `0` keeps it. See [`Memory::prune_sources`].
    source_retain_days: u64,
}

impl Memory {
    pub fn new(store: Arc<dyn Store>, tenant: impl Into<String>) -> Self {
        Self {
            store,
            tenant: tenant.into(),
            source_retain_days: DEFAULT_SOURCE_RETAIN_DAYS,
        }
    }

    pub async fn init(&self) -> Result<()> {
        self.store.ensure().await
    }

    /// `Some(reason)` while the store is failing and lookups are being skipped, so callers can tell
    /// the user memory is off instead of silently returning nothing.
    pub fn degraded(&self) -> Option<String> {
        self.store.degraded()
    }

    // ---------------------------------------------------------------- profile

    pub async fn remember(
        &self,
        key: &str,
        value: Value,
        description: Option<&str>,
        autocomplete: Option<&str>,
        source: &str,
    ) -> Result<Fact> {
        let key = key.trim();
        if key.is_empty() {
            return Err(MemoryError::Invalid("key must not be empty".into()));
        }
        let text_value = match &value {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        if looks_sensitive(key, &text_value) || value_looks_sensitive(&value) {
            return Err(MemoryError::Invalid(format!(
                "{key:?} looks like a secret (password, card, token…). Ostler never stores those."
            )));
        }
        let _guard = stripe(&self.fact_id(key)).lock().await;
        self.put_fact(key, value, description, autocomplete, source, None)
            .await
    }

    /// The id `remember(key)` stores a fact under for `tenant` (before sealing keys it). Tests only.
    #[doc(hidden)]
    pub fn fact_id_for_tests(tenant: &str, key: &str) -> String {
        point_id(&[tenant, "profile", &norm(key)])
    }

    fn fact_id(&self, key: &str) -> String {
        point_id(&[&self.tenant, "profile", &norm(key)])
    }

    /// Store a fact without the secret check. `remember` is the checked entry point; the sealing
    /// migration uses this to carry over facts saved before a rule existed (a key like "shipping address"
    /// once tripped the "pin" rule) and keeps their original `updated_at`.
    pub(crate) async fn put_fact(
        &self,
        key: &str,
        value: Value,
        description: Option<&str>,
        autocomplete: Option<&str>,
        source: &str,
        updated_at: Option<u64>,
    ) -> Result<Fact> {
        let ac = autocomplete
            .map(String::from)
            .or_else(|| default_autocomplete(key).map(String::from));
        let updated_at = updated_at.unwrap_or_else(now);
        let embed_text = format!(
            "{}{}{}",
            key.replace('_', " "),
            description.map(|d| format!(". {d}")).unwrap_or_default(),
            ac.as_deref()
                .map(|a| format!(". {}", a.replace('-', " ")))
                .unwrap_or_default()
        );
        self.store
            .upsert(
                Kind::Profile,
                &self.fact_id(key),
                &embed_text,
                json!({
                    "tenant": self.tenant, "key": key, "key_norm": norm(key), "value": value,
                    "description": description, "autocomplete": ac, "source": source,
                    "updated_at": updated_at,
                }),
            )
            .await?;
        Ok(Fact {
            key: key.into(),
            value,
            description: description.map(String::from),
            autocomplete: ac,
            source: source.into(),
            updated_at,
        })
    }

    pub async fn facts(&self, limit: usize) -> Result<Vec<Fact>> {
        let hits = self
            .store
            .scroll(Kind::Profile, &Filter::tenant(&self.tenant), limit)
            .await?;
        let mut v: Vec<Fact> = hits
            .iter()
            .filter_map(|h| Fact::from_payload(&h.payload))
            .collect();
        v.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(v)
    }

    pub async fn fact(&self, key: &str) -> Result<Option<Fact>> {
        let f = Filter::tenant(&self.tenant).and("key_norm", json!(norm(key)));
        Ok(self
            .store
            .scroll(Kind::Profile, &f, 1)
            .await?
            .first()
            .and_then(|h| Fact::from_payload(&h.payload)))
    }

    /// Forget a fact and what was derived from it: playbook mappings that point at the key are cleared
    /// (the form's structure and history stay, the field falls back to the agent), and episodes whose
    /// text echoes the fact's value are deleted. Derived records go first and the fact last, so a failure
    /// part-way leaves the fact in place and `forget` can simply be called again.
    pub async fn forget(&self, key: &str) -> Result<bool> {
        let fact = self.fact(key).await?;
        self.cascade_forget(key, fact.as_ref()).await?;
        self.store.delete(Kind::Profile, &self.fact_id(key)).await?;
        Ok(fact.is_some())
    }

    async fn cascade_forget(&self, key: &str, fact: Option<&Fact>) -> Result<()> {
        let key_norm = norm(key);
        let tenant = Filter::tenant(&self.tenant);
        for h in scroll_all(self.store.as_ref(), Kind::Playbook, &tenant).await? {
            let Ok(pb) = serde_json::from_value::<Playbook>(h.payload["playbook"].clone()) else {
                continue;
            };
            let refers = |f: &PlaybookField| {
                f.profile_key
                    .as_deref()
                    .is_some_and(|k| norm(k) == key_norm)
            };
            if !pb.fields.iter().any(refers) {
                continue;
            }
            let _guard = stripe(&self.playbook_id(&pb.fingerprint)).lock().await;
            // Re-read under the lock: an outcome may have been recorded since the scan.
            let Some(mut pb) = self.playbook(&pb.fingerprint).await? else {
                continue;
            };
            let mut changed = false;
            for f in pb.fields.iter_mut().filter(|f| refers(f)) {
                f.profile_key = None;
                changed = true;
            }
            if changed {
                self.save_playbook(&pb).await?;
            }
        }
        // Episodes never name a fact, but their page text can echo its value.
        let needle = fact
            .and_then(|f| f.value.as_str())
            .map(|v| v.trim().to_lowercase())
            .filter(|v| v.chars().count() >= 3);
        if let Some(needle) = needle {
            for h in scroll_all(self.store.as_ref(), Kind::Episode, &tenant).await? {
                let text = h.payload["episode"]["detail"]
                    .as_str()
                    .unwrap_or_default()
                    .to_lowercase();
                if text.contains(&needle) {
                    self.store.delete(Kind::Episode, &h.id).await?;
                }
            }
            // Remembered page text can echo it too.
            self.forget_sources_echoing(&needle).await?;
        }
        Ok(())
    }

    /// Semantic search over what we know about the user.
    pub async fn recall(&self, query: &str, limit: usize) -> Result<Vec<(Fact, f32)>> {
        let hits = self
            .store
            .search(Kind::Profile, query, &Filter::tenant(&self.tenant), limit)
            .await?;
        Ok(hits
            .iter()
            .filter_map(|h| Fact::from_payload(&h.payload).map(|f| (f, h.score)))
            .collect())
    }

    pub async fn wipe(&self) -> Result<()> {
        for k in Kind::ALL {
            self.store
                .delete_matching(k, &Filter::tenant(&self.tenant))
                .await?;
        }
        Ok(())
    }

    // ---------------------------------------------------------------- playbooks

    pub async fn playbook(&self, fingerprint: &str) -> Result<Option<Playbook>> {
        let f = Filter::tenant(&self.tenant).and("fingerprint", json!(fingerprint));
        Ok(self
            .store
            .scroll(Kind::Playbook, &f, 1)
            .await?
            .first()
            .and_then(|h| serde_json::from_value(h.payload["playbook"].clone()).ok()))
    }

    /// Record a form that was filled and accepted. Stores structure only, never values.
    pub async fn record_success(
        &self,
        host: &str,
        fingerprint: &str,
        headings: &[String],
        fields: Vec<PlaybookField>,
    ) -> Result<Playbook> {
        let outcomes = fields
            .into_iter()
            .map(|f| FieldOutcome {
                label: f.label,
                kind: f.kind,
                profile_key: f.profile_key,
                ok: true,
            })
            .collect();
        self.record_outcome(host, fingerprint, headings, outcomes, true, None)
            .await
    }

    /// Patch the playbook for this form with what just happened. Never a blind rewrite:
    /// fields not mentioned keep their history, mappings earn trust by winning and lose it by failing.
    pub async fn record_outcome(
        &self,
        host: &str,
        fingerprint: &str,
        headings: &[String],
        outcomes: Vec<FieldOutcome>,
        accepted: bool,
        learned_from: Option<&str>,
    ) -> Result<Playbook> {
        let _guard = stripe(&self.playbook_id(fingerprint)).lock().await;
        let prior = self.playbook(fingerprint).await?;
        let mut fields = prior.as_ref().map(|p| p.fields.clone()).unwrap_or_default();
        for o in outcomes {
            let i = match fields.iter().position(|f| norm(&f.label) == norm(&o.label)) {
                Some(i) => i,
                None => {
                    fields.push(PlaybookField::new(&o.label, &o.kind, None));
                    fields.len() - 1
                }
            };
            let f = &mut fields[i];
            f.kind = o.kind;
            if o.ok && accepted {
                f.wins += 1;
                if o.profile_key.is_some() {
                    if f.profile_key != o.profile_key {
                        // A different fact filled it and was accepted: start that mapping's record fresh.
                        f.wins = 1;
                        f.losses = 0;
                    }
                    f.profile_key = o.profile_key;
                }
            } else if !o.ok {
                f.losses += 1;
                if f.losses > f.wins {
                    // Demote: this mapping now loses more than it wins. Forget it; the field falls back
                    // to the autocomplete and semantic signals, or to the agent.
                    f.profile_key = None;
                }
            }
        }
        let now = now();
        let pb = Playbook {
            host: host.into(),
            fingerprint: fingerprint.into(),
            headings: if headings.is_empty() {
                prior
                    .as_ref()
                    .map(|p| p.headings.clone())
                    .unwrap_or_default()
            } else {
                headings.to_vec()
            },
            fields,
            uses: prior.as_ref().map(|p| p.uses).unwrap_or(0) + u64::from(accepted),
            last_success: if accepted {
                now
            } else {
                prior.as_ref().map(|p| p.last_success).unwrap_or(0)
            },
            wins: prior.as_ref().map(|p| p.wins).unwrap_or(0) + u64::from(accepted),
            losses: prior.as_ref().map(|p| p.losses).unwrap_or(0) + u64::from(!accepted),
            verified_ts: if accepted {
                now
            } else {
                prior.as_ref().map(|p| p.verified_ts).unwrap_or(0)
            },
            learned_from: prior
                .as_ref()
                .and_then(|p| p.learned_from.clone())
                .or_else(|| learned_from.map(String::from)),
        };
        self.save_playbook(&pb).await?;
        Ok(pb)
    }

    fn playbook_id(&self, fingerprint: &str) -> String {
        point_id(&[&self.tenant, "playbook", fingerprint])
    }

    async fn save_playbook(&self, pb: &Playbook) -> Result<()> {
        let labels: Vec<&str> = pb.fields.iter().map(|f| f.label.as_str()).collect();
        let labels_norm: Vec<String> = pb.fields.iter().map(|f| norm(&f.label)).collect();
        self.store
            .upsert(
                Kind::Playbook,
                &self.playbook_id(&pb.fingerprint),
                &structure_text(&pb.host, &pb.headings, &labels),
                json!({
                    "tenant": self.tenant, "host": pb.host, "fingerprint": pb.fingerprint,
                    "labels_norm": labels_norm,
                    // Top-level copies for Qdrant's filter and score-boosting formula.
                    "wins": pb.wins, "losses": pb.losses, "verified_ts": pb.verified_ts,
                    "playbook": pb,
                }),
            )
            .await
    }

    /// Playbooks from other forms on this site, ranked by structural similarity, evidence and recency.
    pub async fn similar_playbooks(
        &self,
        host: &str,
        fingerprint: &str,
        headings: &[String],
        labels: &[&str],
        limit: usize,
    ) -> Result<Vec<(Playbook, f32)>> {
        let hits = self
            .store
            .search_ranked(
                Kind::Playbook,
                &structure_text(host, headings, labels),
                &Filter::tenant(&self.tenant).and("host", json!(host)),
                limit.saturating_add(1),
                &rank(0.5),
            )
            .await?;
        Ok(hits
            .into_iter()
            .filter_map(|h| {
                serde_json::from_value::<Playbook>(h.payload["playbook"].clone())
                    .ok()
                    .map(|p| (p, h.score))
            })
            .filter(|(p, _)| p.fingerprint != fingerprint)
            .take(limit)
            .collect())
    }

    /// Everything learned, best-proven first: the "skills" view.
    pub async fn skills(&self, limit: usize) -> Result<Vec<Playbook>> {
        let mut v: Vec<Playbook> = self
            .store
            .scroll(Kind::Playbook, &Filter::tenant(&self.tenant), limit)
            .await?
            .iter()
            .filter_map(|h| serde_json::from_value(h.payload["playbook"].clone()).ok())
            .collect();
        v.sort_by(|a, b| {
            b.win_rate()
                .total_cmp(&a.win_rate())
                .then(b.verified_ts.cmp(&a.verified_ts))
        });
        Ok(v)
    }

    /// Forget what has gone stale: playbooks not verified for `max_age_secs` that are unproven
    /// (losses at least equal to wins), and episodes not seen for that long, plus remembered pages older than
    /// [`Memory::source_retain_days`] (their own clock, `max_age_secs` does not apply to them). Returns how
    /// many went.
    pub async fn prune(&self, max_age_secs: u64) -> Result<usize> {
        let cutoff = now().saturating_sub(max_age_secs) as f64;
        let stale = Filter::tenant(&self.tenant).below("verified_ts", cutoff);
        let mut n = 0;
        for h in scroll_all(self.store.as_ref(), Kind::Playbook, &stale).await? {
            let (w, l) = (
                h.payload["wins"].as_u64().unwrap_or(0),
                h.payload["losses"].as_u64().unwrap_or(0),
            );
            if l >= w {
                self.store.delete(Kind::Playbook, &h.id).await?;
                n += 1;
            }
        }
        n += self.store.count(Kind::Episode, &stale).await? as usize;
        self.store.delete_matching(Kind::Episode, &stale).await?;
        n += self.prune_sources(self.source_retain_days).await?;
        Ok(n)
    }

    // ---------------------------------------------------------------- episodes

    pub async fn record_episode(
        &self,
        host: &str,
        fingerprint: &str,
        field: &str,
        problem: &str,
        detail: &str,
    ) -> Result<()> {
        let detail = &redact_detail(detail);
        let id = point_id(&[
            &self.tenant,
            "episode",
            fingerprint,
            &norm(field),
            problem,
            detail,
        ]);
        let _guard = stripe(&id).lock().await;
        let prior = self
            .store
            .scroll(
                Kind::Episode,
                &Filter::tenant(&self.tenant).and("id_hint", json!(id)),
                1,
            )
            .await?
            .first()
            .and_then(|h| h.payload["episode"]["count"].as_u64())
            .unwrap_or(0);
        let ep = Episode {
            host: host.into(),
            fingerprint: fingerprint.into(),
            field: field.into(),
            problem: problem.into(),
            detail: detail.chars().take(300).collect(),
            count: prior + 1,
            last_seen: now(),
        };
        self.save_episode(&id, &ep).await?;
        if prior == 0 {
            // Housekeeping only when the set grew, and never at the cost of the write that just succeeded.
            let _ = self
                .trim_episodes(EPISODE_TTL_SECS, MAX_EPISODES_PER_TENANT)
                .await;
        }
        Ok(())
    }

    /// Enforce episode retention for this tenant: drop episodes not seen for `ttl_secs`, then the least
    /// recently seen beyond `max`. Returns how many went.
    pub async fn trim_episodes(&self, ttl_secs: u64, max: usize) -> Result<usize> {
        let tenant = Filter::tenant(&self.tenant);
        let mut n = 0;
        let expired = tenant
            .clone()
            .below("verified_ts", now().saturating_sub(ttl_secs) as f64);
        n += self.store.count(Kind::Episode, &expired).await? as usize;
        self.store.delete_matching(Kind::Episode, &expired).await?;
        if self.store.count(Kind::Episode, &tenant).await? as usize > max {
            let mut all = scroll_all(self.store.as_ref(), Kind::Episode, &tenant).await?;
            all.sort_by_key(|h| std::cmp::Reverse(h.payload["verified_ts"].as_u64().unwrap_or(0)));
            for h in all.into_iter().skip(max) {
                self.store.delete(Kind::Episode, &h.id).await?;
                n += 1;
            }
        }
        Ok(n)
    }

    async fn save_episode(&self, id: &str, ep: &Episode) -> Result<()> {
        self.store
            .upsert(
                Kind::Episode,
                id,
                &format!("{} {} {} {}", ep.host, ep.field, ep.problem, ep.detail),
                json!({"tenant": self.tenant, "host": ep.host, "fingerprint": ep.fingerprint, "id_hint": id,
                       "verified_ts": ep.last_seen, "episode": ep}),
            )
            .await
    }

    /// Write a point read back from the store (already opened) again under the current format and
    /// normalisation, for `migrate_sealed`. Returns the id it was saved under, or `None` when the payload
    /// is not one this build understands. A fact never replaces a newer one that already sits at its
    /// current id.
    pub(crate) async fn resave(&self, kind: Kind, payload: &Value) -> Result<Option<String>> {
        Ok(match kind {
            Kind::Profile => {
                let Some(f) = Fact::from_payload(payload) else {
                    return Ok(None);
                };
                let id = self.fact_id(&f.key);
                let newer = self
                    .fact(&f.key)
                    .await?
                    .is_some_and(|e| e.updated_at >= f.updated_at);
                if !newer {
                    self.put_fact(
                        &f.key,
                        f.value,
                        f.description.as_deref(),
                        f.autocomplete.as_deref(),
                        &f.source,
                        Some(f.updated_at),
                    )
                    .await?;
                }
                Some(id)
            }
            Kind::Playbook => {
                let Ok(pb) = serde_json::from_value::<Playbook>(payload["playbook"].clone()) else {
                    return Ok(None);
                };
                self.save_playbook(&pb).await?;
                Some(self.playbook_id(&pb.fingerprint))
            }
            Kind::Episode => {
                let (Some(id), Ok(ep)) = (
                    payload["id_hint"].as_str(),
                    serde_json::from_value::<Episode>(payload["episode"].clone()),
                ) else {
                    return Ok(None);
                };
                self.save_episode(id, &ep).await?;
                Some(id.to_string())
            }
            Kind::Source => self.resave_source(payload).await?,
        })
    }

    pub async fn episodes(&self, fingerprint: &str, limit: usize) -> Result<Vec<Episode>> {
        let f = Filter::tenant(&self.tenant).and("fingerprint", json!(fingerprint));
        let mut v: Vec<Episode> = self
            .store
            .scroll(Kind::Episode, &f, limit)
            .await?
            .iter()
            .filter_map(|h| serde_json::from_value(h.payload["episode"].clone()).ok())
            .collect();
        v.sort_by_key(|e| std::cmp::Reverse(e.last_seen));
        Ok(v)
    }

    /// Save a value observed on a page. Unlike [`Memory::remember`] this never replaces a fact the
    /// user supplied: page labels are chosen by the page ("Email" on a refer-a-friend form), not by the user.
    pub async fn learn(&self, key: &str, value: Value) -> Result<Option<Fact>> {
        // Check-then-write under the fact's lock, or a `remember` landing in between would be overwritten.
        let _guard = stripe(&self.fact_id(key.trim())).lock().await;
        if let Some(existing) = self.fact(key).await? {
            if existing.source != "learned" {
                return Ok(None);
            }
        }
        let key = key.trim();
        if key.is_empty() {
            return Err(MemoryError::Invalid("key must not be empty".into()));
        }
        let text_value = match &value {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        if looks_sensitive(key, &text_value) || value_looks_sensitive(&value) {
            return Err(MemoryError::Invalid(format!(
                "{key:?} looks like a secret (password, card, token…). Ostler never stores those."
            )));
        }
        self.put_fact(key, value, None, None, "learned", None)
            .await
            .map(Some)
    }

    // ---------------------------------------------------------------- filling

    /// Best remembered value for a form field, preferring exact signals over semantic ones.
    pub async fn suggest(&self, q: &FieldQuery<'_>, min_semantic: f32) -> Result<Option<Recalled>> {
        // 1. A playbook from a previous successful fill told us which fact fed this field.
        if let Some(pb) = self.playbook(q.fingerprint).await? {
            let want = norm(q.label);
            if let Some(key) = pb
                .fields
                .iter()
                .find(|f| norm(&f.label) == want && f.trusted())
                .and_then(|f| f.profile_key.clone())
            {
                if let Some(fact) = self.fact(&key).await? {
                    return Ok(Some(Recalled {
                        fact,
                        confidence: 0.95,
                        via: "playbook",
                    }));
                }
            }
        }
        // 2. The field declares what it is (autocomplete="email") and we hold a fact with that token.
        if let Some(ac) = q.autocomplete {
            let token = ac.split_whitespace().last().unwrap_or(ac);
            let f = Filter::tenant(&self.tenant).and("autocomplete", json!(token));
            if let Some(fact) = self
                .store
                .scroll(Kind::Profile, &f, 1)
                .await?
                .first()
                .and_then(|h| Fact::from_payload(&h.payload))
            {
                return Ok(Some(Recalled {
                    fact,
                    confidence: 0.9,
                    via: "autocomplete",
                }));
            }
        }
        // 3. Transfer: a similar form on this site (a redesign, or a sibling form) has a field with this
        //    exact label and a trusted mapping. Qdrant filters by label and host, then ranks the
        //    candidates by similarity, win rate and recency in one query.
        if !q.host.is_empty() && !norm(q.label).is_empty() && !mentions_other_person(&norm(q.label))
        {
            let want = norm(q.label);
            let f = Filter::tenant(&self.tenant)
                .and("host", json!(q.host))
                .and("labels_norm", json!(want));
            let hits = self
                .store
                .search_ranked(
                    Kind::Playbook,
                    &structure_text(q.host, &[], &[q.label]),
                    &f,
                    5,
                    &rank(0.0),
                )
                .await?;
            for h in hits {
                let Ok(pb) = serde_json::from_value::<Playbook>(h.payload["playbook"].clone())
                else {
                    continue;
                };
                if pb.fingerprint == q.fingerprint {
                    continue;
                }
                let key = pb
                    .fields
                    .iter()
                    .find(|f| norm(&f.label) == want && f.kind == q.kind && f.trusted())
                    .and_then(|f| f.profile_key.clone());
                if let Some(key) = key {
                    if let Some(fact) = self.fact(&key).await? {
                        return Ok(Some(Recalled {
                            fact,
                            confidence: 0.85,
                            via: "similar_form",
                        }));
                    }
                }
            }
        }
        // 4. Semantic match on everything we can say about the field.
        let query = [Some(q.label), q.name, q.placeholder, Some(q.kind)]
            .into_iter()
            .flatten()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        if query.trim().is_empty() {
            return Ok(None);
        }
        // Labels about another person must never pull in the user's own facts.
        if mentions_other_person(&norm(q.label)) {
            return Ok(None);
        }
        let hits = self
            .store
            .search(Kind::Profile, &query, &Filter::tenant(&self.tenant), 2)
            .await?;
        // Short labels in one category ("first name", "last name") embed close together. If the runner-up is
        // nearly as good we cannot tell which fact belongs here, and a wrong value typed confidently is worse
        // than an empty field.
        if let (Some(a), Some(b)) = (hits.first(), hits.get(1)) {
            if a.score - b.score < SEMANTIC_MARGIN {
                return Ok(None);
            }
        }
        Ok(hits
            .first()
            .filter(|h| h.score >= min_semantic)
            .and_then(|h| {
                Fact::from_payload(&h.payload).map(|fact| Recalled {
                    fact,
                    confidence: h.score.min(0.89),
                    via: "semantic",
                })
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Memory {
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new(Arc::new(HashEmbedder::default())));
        Memory::new(store, "t1")
    }

    #[test]
    fn sensitive_guard() {
        assert!(looks_sensitive("password", "x"));
        assert!(looks_sensitive("note", "4111 1111 1111 1111"));
        assert!(looks_sensitive("api_key", "abc"));
        assert!(!looks_sensitive("email", "a@b.co"));
        assert!(!looks_sensitive("phone", "5551234567"));
    }

    #[tokio::test]
    async fn remember_recall_forget() {
        let m = mem();
        m.remember("email", json!("ada@example.com"), None, None, "user")
            .await
            .unwrap();
        m.remember(
            "company",
            json!("Analytical Engines"),
            Some("employer"),
            None,
            "user",
        )
        .await
        .unwrap();
        assert_eq!(m.facts(10).await.unwrap().len(), 2);
        assert_eq!(
            m.fact("Email").await.unwrap().unwrap().value,
            json!("ada@example.com")
        );
        // Upsert, not duplicate.
        m.remember("email", json!("new@example.com"), None, None, "user")
            .await
            .unwrap();
        assert_eq!(m.facts(10).await.unwrap().len(), 2);
        assert!(m.forget("email").await.unwrap());
        assert!(m.fact("email").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn refuses_secrets() {
        let m = mem();
        assert!(m
            .remember("password", json!("hunter2"), None, None, "user")
            .await
            .is_err());
        assert!(m
            .remember("note", json!("4111111111111111"), None, None, "user")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn tenants_are_isolated() {
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new(Arc::new(HashEmbedder::default())));
        let a = Memory::new(store.clone(), "a");
        let b = Memory::new(store, "b");
        a.remember("email", json!("a@x.co"), None, None, "user")
            .await
            .unwrap();
        assert!(b.fact("email").await.unwrap().is_none());
        assert!(b.recall("email", 5).await.unwrap().is_empty());
        b.wipe().await.unwrap();
        assert!(a.fact("email").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn suggest_prefers_playbook_then_autocomplete_then_semantic() {
        let m = mem();
        m.remember("email", json!("ada@example.com"), None, None, "user")
            .await
            .unwrap();
        m.remember(
            "work_email",
            json!("ada@work.example"),
            None,
            Some("work-email"),
            "user",
        )
        .await
        .unwrap();
        // autocomplete token path
        let q = FieldQuery {
            host: "h",
            fingerprint: "fp",
            label: "Contact",
            name: None,
            kind: "email",
            placeholder: None,
            autocomplete: Some("email"),
        };
        let r = m.suggest(&q, 0.5).await.unwrap().unwrap();
        assert_eq!((r.via, r.fact.key.as_str()), ("autocomplete", "email"));
        // a playbook overrides: this form's "Contact" field is fed by work_email
        m.record_success(
            "h",
            "fp",
            &[],
            vec![PlaybookField::new(
                "Contact",
                "email",
                Some("work_email".into()),
            )],
        )
        .await
        .unwrap();
        let r = m.suggest(&q, 0.5).await.unwrap().unwrap();
        assert_eq!((r.via, r.fact.key.as_str()), ("playbook", "work_email"));
        // semantic path
        let q2 = FieldQuery {
            host: "h",
            fingerprint: "other",
            label: "Email address",
            name: Some("email"),
            kind: "email",
            placeholder: None,
            autocomplete: None,
        };
        let r = m.suggest(&q2, 0.3).await.unwrap().unwrap();
        assert_eq!(r.via, "semantic");
        // nothing relevant: no suggestion at a high threshold
        let q3 = FieldQuery {
            host: "h",
            fingerprint: "other",
            label: "Favourite dinosaur",
            name: None,
            kind: "text",
            placeholder: None,
            autocomplete: None,
        };
        assert!(m.suggest(&q3, 0.8).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn playbook_counts_uses_and_holds_no_values() {
        let m = mem();
        for _ in 0..3 {
            m.record_success("h", "fp", &["Contact".into()], vec![])
                .await
                .unwrap();
        }
        assert_eq!(m.playbook("fp").await.unwrap().unwrap().uses, 3);
    }

    #[tokio::test]
    async fn episodes_dedupe_and_count() {
        let m = mem();
        for _ in 0..2 {
            m.record_episode("h", "fp", "ZIP", "validation_error", "ZIP must be 5 digits")
                .await
                .unwrap();
        }
        let eps = m.episodes("fp", 10).await.unwrap();
        assert_eq!(eps.len(), 1);
        assert_eq!(eps[0].count, 2);
    }
}

#[cfg(test)]
mod safety_tests {
    use super::*;

    fn mem() -> Memory {
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new(Arc::new(HashEmbedder::default())));
        Memory::new(store, "t1")
    }

    #[tokio::test]
    async fn learned_values_never_overwrite_user_facts() {
        let m = mem();
        m.remember("email", json!("me@example.com"), None, None, "user")
            .await
            .unwrap();
        assert!(m
            .learn("Email", json!("friend@example.com"))
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            m.fact("email").await.unwrap().unwrap().value,
            json!("me@example.com")
        );
        // A previously learned value may be refreshed.
        m.learn("company", json!("Acme")).await.unwrap();
        m.learn("company", json!("Acme Inc")).await.unwrap();
        assert_eq!(
            m.fact("company").await.unwrap().unwrap().value,
            json!("Acme Inc")
        );
    }

    #[tokio::test]
    async fn labels_about_someone_else_do_not_pull_in_the_users_facts() {
        let m = mem();
        m.remember("email", json!("me@example.com"), None, None, "user")
            .await
            .unwrap();
        for label in [
            "Spouse email",
            "Emergency contact email",
            "Reference email",
            "Friend's email",
        ] {
            let q = FieldQuery {
                host: "x.com",
                fingerprint: "none",
                label,
                name: None,
                kind: "email",
                placeholder: None,
                autocomplete: None,
            };
            assert!(m.suggest(&q, 0.0).await.unwrap().is_none(), "{label}");
        }
    }

    #[test]
    fn other_person_detection() {
        assert!(mentions_other_person("emergency contact phone"));
        assert!(!mentions_other_person("email address"));
        assert!(!mentions_other_person("company name"));
    }
}

#[cfg(test)]
mod learning_tests {
    use super::*;

    fn mem() -> Memory {
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new(Arc::new(HashEmbedder::default())));
        Memory::new(store, "t1")
    }

    fn q<'a>(host: &'a str, fp: &'a str, label: &'a str, kind: &'a str) -> FieldQuery<'a> {
        FieldQuery {
            host,
            fingerprint: fp,
            label,
            name: None,
            kind,
            placeholder: None,
            autocomplete: None,
        }
    }

    fn ok(label: &str, kind: &str, key: Option<&str>) -> FieldOutcome {
        FieldOutcome {
            label: label.into(),
            kind: kind.into(),
            profile_key: key.map(String::from),
            ok: true,
        }
    }

    #[tokio::test]
    async fn failing_mapping_is_demoted_then_forgotten() {
        let m = mem();
        m.remember("work_email", json!("ada@work.example"), None, None, "user")
            .await
            .unwrap();
        m.record_outcome(
            "h",
            "fp",
            &[],
            vec![ok("Contact", "email", Some("work_email"))],
            true,
            None,
        )
        .await
        .unwrap();
        let r = m
            .suggest(&q("h", "fp", "Contact", "email"), 0.99)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(r.via, "playbook");
        // Two failures beat one win: the mapping is dropped and the playbook no longer offers it.
        for _ in 0..2 {
            let mut o = ok("Contact", "email", Some("work_email"));
            o.ok = false;
            m.record_outcome("h", "fp", &[], vec![o], false, None)
                .await
                .unwrap();
        }
        let pb = m.playbook("fp").await.unwrap().unwrap();
        assert_eq!((pb.wins, pb.losses), (1, 2));
        assert!(!pb.fields[0].trusted());
        assert!(m
            .suggest(&q("h", "fp", "Contact", "email"), 0.99)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn outcomes_patch_the_skill_instead_of_rewriting_it() {
        let m = mem();
        m.record_outcome(
            "h",
            "fp",
            &["Signup".into()],
            vec![
                ok("Full name", "text", Some("name")),
                ok("Org", "text", Some("company")),
            ],
            true,
            None,
        )
        .await
        .unwrap();
        // A later fill that only touched one field keeps the other field's history and the headings.
        m.record_outcome(
            "h",
            "fp",
            &[],
            vec![ok("Full name", "text", Some("name"))],
            true,
            None,
        )
        .await
        .unwrap();
        let pb = m.playbook("fp").await.unwrap().unwrap();
        assert_eq!(pb.headings, vec!["Signup".to_string()]);
        assert_eq!(pb.fields.len(), 2);
        assert_eq!(pb.fields[0].wins, 2);
        assert_eq!(pb.fields[1].wins, 1);
        assert_eq!(pb.fields[1].profile_key.as_deref(), Some("company"));
    }

    #[tokio::test]
    async fn knowledge_transfers_to_a_redesigned_form_on_the_same_site_only() {
        let m = mem();
        m.remember("company", json!("Analytical Engines"), None, None, "user")
            .await
            .unwrap();
        m.record_outcome(
            "shop.example",
            "fp-old",
            &[],
            vec![ok("Org", "text", Some("company"))],
            true,
            None,
        )
        .await
        .unwrap();
        // Same site, new fingerprint (redesign): the "Org" mapping transfers.
        let r = m
            .suggest(&q("shop.example", "fp-new", "Org", "text"), 0.99)
            .await
            .unwrap()
            .unwrap();
        assert_eq!((r.via, r.fact.key.as_str()), ("similar_form", "company"));
        assert!(r.confidence < 0.95);
        // Another site never borrows it, and a different field kind does not match.
        assert!(m
            .suggest(&q("other.example", "fp-x", "Org", "text"), 0.99)
            .await
            .unwrap()
            .is_none());
        assert!(m
            .suggest(&q("shop.example", "fp-new", "Org", "select"), 0.99)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn proven_skills_outrank_failing_ones() {
        let m = mem();
        m.remember("company", json!("Analytical Engines"), None, None, "user")
            .await
            .unwrap();
        m.remember("employer", json!("Babbage & Co"), None, None, "user")
            .await
            .unwrap();
        // fp-a maps Org to employer but keeps failing; fp-b maps it to company and keeps winning.
        m.record_outcome(
            "h",
            "fp-a",
            &[],
            vec![ok("Org", "text", Some("employer"))],
            true,
            None,
        )
        .await
        .unwrap();
        for _ in 0..3 {
            m.record_outcome(
                "h",
                "fp-a",
                &[],
                vec![ok("Name", "text", None)],
                false,
                None,
            )
            .await
            .unwrap();
        }
        for _ in 0..4 {
            m.record_outcome(
                "h",
                "fp-b",
                &[],
                vec![ok("Org", "text", Some("company"))],
                true,
                None,
            )
            .await
            .unwrap();
        }
        let r = m
            .suggest(&q("h", "fp-new", "Org", "text"), 0.99)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(r.fact.key, "company");
    }

    #[tokio::test]
    async fn prune_forgets_stale_unproven_skills_but_keeps_proven_ones() {
        let m = mem();
        m.record_outcome("h", "proven", &[], vec![ok("A", "text", None)], true, None)
            .await
            .unwrap();
        m.record_outcome(
            "h",
            "unproven",
            &[],
            vec![ok("A", "text", None)],
            false,
            None,
        )
        .await
        .unwrap();
        m.record_episode("h", "proven", "A", "validation_error", "old")
            .await
            .unwrap();
        let n = m.prune(3600).await.unwrap();
        assert_eq!(n, 1);
        assert!(m.playbook("proven").await.unwrap().is_some());
        assert!(m.playbook("unproven").await.unwrap().is_none());
        assert_eq!(
            m.episodes("proven", 5).await.unwrap().len(),
            1,
            "recent episodes stay"
        );
    }

    #[test]
    fn rank_formula_matches_its_definition() {
        let r = Rank {
            now: 1_000_000,
            success_weight: 0.3,
            recency_weight: 0.2,
            half_life_secs: 1000,
            min_similarity: 0.0,
        };
        // untested, verified exactly one half-life ago: 0.5 + 0.3*0.5 + 0.2*0.5
        let s = r.score(
            0.5,
            &json!({"wins": 0, "losses": 0, "verified_ts": 999_000}),
        );
        assert!((s - 0.75).abs() < 1e-4, "{s}");
        let q = r.to_qdrant_query(json!([1.0, 0.0]), &Filter::tenant("t"), 3);
        assert_eq!(q["prefetch"]["score_threshold"], json!(0.0));
        assert_eq!(q["query"]["formula"]["sum"][0], json!("$score"));
        assert_eq!(q["limit"], json!(3));
    }
}

#[cfg(test)]
mod redact_tests {
    use super::redact_detail as r;

    #[test]
    fn masks_emails_numbers_and_queries_but_keeps_the_message() {
        assert_eq!(
            r("ada@example.com is not a valid email"),
            "[email] is not a valid email"
        );
        assert_eq!(
            r("Phone 98765 43210 is invalid"),
            "Phone [number] is invalid"
        );
        assert_eq!(
            r("ZIP 411029 must be 5 digits"),
            "ZIP [number] must be 5 digits"
        );
        assert_eq!(
            r("Card 4111-1111-1111-1111 rejected"),
            "Card [number] rejected"
        );
        assert_eq!(
            r("see https://x.test/f?id=7&e=a@b.co"),
            "see https://x.test/f?[query]"
        );
        assert_eq!(
            r("Age must be between 18 and 120"),
            "Age must be between 18 and 120"
        );
    }
}

#[cfg(test)]
mod card_tests {
    use super::looks_sensitive as s;

    #[test]
    fn card_numbers_are_caught_through_odd_separators_and_not_in_ordinary_text() {
        for v in [
            "4111111111111111",
            "4111 1111 1111 1111",
            "4111 - 1111 - 1111 - 1111",
            "4111,1111,1111,1111",
            "4111/1111/1111/1111",
            "4111 1111 1111 1111 4111 1111 1111 1111",
            "my card is 4111 1111 1111 1111 ok",
            "id=4111-1111-1111-1111",
        ] {
            assert!(s("note", v), "should be refused: {v}");
        }
        for v in [
            "order 1234567890123 shipped",
            "call 555 123 4567",
            "ada@example.com",
            "Pune 411029",
            "2026-11-02",
        ] {
            assert!(!s("note", v), "should be allowed: {v}");
        }
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;

    fn mem() -> Memory {
        Memory::new(
            Arc::new(InMemoryStore::new(Arc::new(embed::HashEmbedder::default()))),
            "t1",
        )
    }

    async fn episode_count(m: &Memory) -> u64 {
        m.store
            .count(Kind::Episode, &Filter::tenant(&m.tenant))
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn forget_clears_playbook_mappings_and_echoing_episodes_but_keeps_the_rest() {
        let m = mem();
        m.remember("home_street", json!("Hawthorn Lane"), None, None, "user")
            .await
            .unwrap();
        m.remember("city", json!("Pune"), None, None, "user")
            .await
            .unwrap();
        m.record_success(
            "h",
            "fp",
            &[],
            vec![
                PlaybookField::new("Email", "email", Some("home_street".into())),
                PlaybookField::new("City", "text", Some("city".into())),
            ],
        )
        .await
        .unwrap();
        m.record_episode(
            "h",
            "fp",
            "Email",
            "validation_error",
            "Hawthorn Lane is not served",
        )
        .await
        .unwrap();
        m.record_episode("h", "fp", "City", "validation_error", "pick a city")
            .await
            .unwrap();

        assert!(m.forget("Home Street").await.unwrap());

        let pb = m.playbook("fp").await.unwrap().unwrap();
        let by = |l: &str| pb.fields.iter().find(|f| f.label == l).unwrap().clone();
        assert_eq!(
            by("Email").profile_key,
            None,
            "mapping to the forgotten fact"
        );
        assert_eq!(by("Email").wins, 1, "the field's history is kept");
        assert_eq!(by("City").profile_key.as_deref(), Some("city"));
        assert!(m.fact("home_street").await.unwrap().is_none());
        assert!(m.fact("city").await.unwrap().is_some());
        let eps = m.episodes("fp", 10).await.unwrap();
        assert_eq!(eps.len(), 1, "{eps:?}");
        assert_eq!(eps[0].field, "City");
    }

    #[tokio::test]
    async fn forget_cascades_within_the_tenant_only() {
        let store: Arc<dyn Store> =
            Arc::new(InMemoryStore::new(Arc::new(embed::HashEmbedder::default())));
        let a = Memory::new(store.clone(), "a");
        let b = Memory::new(store, "b");
        for m in [&a, &b] {
            m.remember("city", json!("Pune"), None, None, "user")
                .await
                .unwrap();
            m.record_success(
                "h",
                "fp",
                &[],
                vec![PlaybookField::new("City", "text", Some("city".into()))],
            )
            .await
            .unwrap();
            m.record_episode("h", "fp", "City", "p", "Pune is not served")
                .await
                .unwrap();
        }
        a.forget("city").await.unwrap();
        let pb = b.playbook("fp").await.unwrap().unwrap();
        assert_eq!(pb.fields[0].profile_key.as_deref(), Some("city"));
        assert_eq!(b.episodes("fp", 10).await.unwrap().len(), 1);
        assert!(b.fact("city").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn forgetting_an_unknown_key_is_false_and_harmless() {
        let m = mem();
        m.record_success("h", "fp", &[], vec![PlaybookField::new("A", "text", None)])
            .await
            .unwrap();
        assert!(!m.forget("nothing").await.unwrap());
        assert!(m.playbook("fp").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn episodes_are_capped_per_tenant_and_the_oldest_go_first() {
        let m = mem();
        for i in 0..6u64 {
            let ep = Episode {
                host: "h".into(),
                fingerprint: "fp".into(),
                field: format!("f{i}"),
                problem: "p".into(),
                detail: String::new(),
                count: 1,
                last_seen: now() - (6 - i) * 10,
            };
            m.save_episode(&format!("id{i}"), &ep).await.unwrap();
        }
        let dropped = m.trim_episodes(EPISODE_TTL_SECS, 4).await.unwrap();
        assert_eq!(dropped, 2);
        let mut left: Vec<String> = m
            .episodes("fp", 10)
            .await
            .unwrap()
            .into_iter()
            .map(|e| e.field)
            .collect();
        left.sort();
        assert_eq!(
            left,
            ["f2", "f3", "f4", "f5"],
            "the two oldest were dropped"
        );
    }

    #[tokio::test]
    async fn expired_episodes_are_dropped_when_a_new_one_arrives() {
        let m = mem();
        let old = Episode {
            host: "h".into(),
            fingerprint: "fp".into(),
            field: "old".into(),
            problem: "p".into(),
            detail: String::new(),
            count: 9,
            last_seen: now() - EPISODE_TTL_SECS - 100,
        };
        m.save_episode("old-id", &old).await.unwrap();
        m.record_episode("h", "fp", "new", "p", "d").await.unwrap();
        let eps = m.episodes("fp", 10).await.unwrap();
        assert_eq!(eps.len(), 1, "{eps:?}");
        assert_eq!(eps[0].field, "new");
    }

    #[tokio::test]
    async fn other_tenants_episodes_do_not_count_against_the_cap() {
        let store: Arc<dyn Store> =
            Arc::new(InMemoryStore::new(Arc::new(embed::HashEmbedder::default())));
        let a = Memory::new(store.clone(), "a");
        let b = Memory::new(store, "b");
        for i in 0..3 {
            a.record_episode("h", "fp", &format!("f{i}"), "p", "d")
                .await
                .unwrap();
        }
        assert_eq!(b.trim_episodes(EPISODE_TTL_SECS, 0).await.unwrap(), 0);
        assert_eq!(episode_count(&a).await, 3);
    }
}
