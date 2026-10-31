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

mod embed;
mod qdrant;
mod store;

pub use embed::{Embedder, HashEmbedder};
pub use qdrant::{QdrantConfig, QdrantStore, VectorSource};
pub use store::{Filter, Hit, InMemoryStore, Kind, Store};

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
}

pub type Result<T> = std::result::Result<T, MemoryError>;

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Stable point id from arbitrary parts, so re-saving the same thing is an upsert.
fn point_id(parts: &[&str]) -> String {
    Uuid::new_v5(&Uuid::NAMESPACE_OID, parts.join("\u{1f}").as_bytes()).to_string()
}

/// Keys/values we refuse to persist or auto-fill.
pub fn looks_sensitive(key: &str, value: &str) -> bool {
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
    if KEYS.iter().any(|m| k.contains(m)) {
        return true;
    }
    // 13-19 digit runs (card numbers), allowing spaces/dashes.
    let digits: String = value
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect();
    digits.len() >= 13 && digits.len() <= 19 && digits.chars().all(|c| c.is_ascii_digit())
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
    /// How we found it: "playbook", "autocomplete" or "semantic".
    pub via: &'static str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybookField {
    pub label: String,
    pub kind: String,
    /// Profile key that supplied this field's value last time, if any.
    pub profile_key: Option<String>,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Playbook {
    pub host: String,
    pub fingerprint: String,
    pub headings: Vec<String>,
    pub fields: Vec<PlaybookField>,
    pub uses: u64,
    pub last_success: u64,
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

fn norm(s: &str) -> String {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// High-level memory API for one tenant.
#[derive(Clone)]
pub struct Memory {
    store: Arc<dyn Store>,
    pub tenant: String,
}

impl Memory {
    pub fn new(store: Arc<dyn Store>, tenant: impl Into<String>) -> Self {
        Self {
            store,
            tenant: tenant.into(),
        }
    }

    pub async fn init(&self) -> Result<()> {
        self.store.ensure().await
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
        if looks_sensitive(key, &text_value) {
            return Err(MemoryError::Invalid(format!(
                "{key:?} looks like a secret (password, card, token…). Waymarker never stores those."
            )));
        }
        let ac = autocomplete
            .map(String::from)
            .or_else(|| default_autocomplete(key).map(String::from));
        let updated_at = now();
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
                &point_id(&[&self.tenant, "profile", &norm(key)]),
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

    pub async fn forget(&self, key: &str) -> Result<bool> {
        let existed = self.fact(key).await?.is_some();
        self.store
            .delete(
                Kind::Profile,
                &point_id(&[&self.tenant, "profile", &norm(key)]),
            )
            .await?;
        Ok(existed)
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
        for k in [Kind::Profile, Kind::Playbook, Kind::Episode] {
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
        let prior = self.playbook(fingerprint).await?;
        let pb = Playbook {
            host: host.into(),
            fingerprint: fingerprint.into(),
            headings: headings.to_vec(),
            fields,
            uses: prior.map(|p| p.uses).unwrap_or(0) + 1,
            last_success: now(),
        };
        let text = format!(
            "{host} {} {}",
            pb.headings.join(" "),
            pb.fields
                .iter()
                .map(|f| f.label.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        self.store
            .upsert(
                Kind::Playbook,
                &point_id(&[&self.tenant, "playbook", fingerprint]),
                &text,
                json!({"tenant": self.tenant, "host": host, "fingerprint": fingerprint, "playbook": pb}),
            )
            .await?;
        Ok(pb)
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
        let id = point_id(&[
            &self.tenant,
            "episode",
            fingerprint,
            &norm(field),
            problem,
            detail,
        ]);
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
        self.store
            .upsert(
                Kind::Episode,
                &id,
                &format!("{host} {field} {problem} {}", ep.detail),
                json!({"tenant": self.tenant, "fingerprint": fingerprint, "id_hint": id, "episode": ep}),
            )
            .await
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
        if let Some(existing) = self.fact(key).await? {
            if existing.source != "learned" {
                return Ok(None);
            }
        }
        self.remember(key, value, None, None, "learned")
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
                .find(|f| norm(&f.label) == want)
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
        // 3. Semantic match on everything we can say about the field.
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
            vec![PlaybookField {
                label: "Contact".into(),
                kind: "email".into(),
                profile_key: Some("work_email".into()),
                required: true,
            }],
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
