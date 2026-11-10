//! Client-side sealing: what Qdrant stores is unreadable without a key that never leaves this machine.
//!
//! [`SealedStore`] wraps any [`Store`]. On the way in, each payload is split:
//!
//! * **kept in clear** (needed for Qdrant to filter, rank and expire): `tenant`, `wins`, `losses`, `verified_ts`;
//! * **keyed hashes** (HMAC-SHA-256, truncated) of the fields the code filters on: `key_norm`,
//!   `fingerprint`, `autocomplete`, `id_hint`, `host`, `labels_norm`. Equality filters still work;
//!   the server never learns the site names, field labels or profile keys;
//! * **point ids** are re-derived with the index key, so they reveal nothing about what a point holds;
//! * **sealed** (AES-256-GCM, random nonce, point id bound as associated data): everything else,
//!   i.e. fact values, descriptions, playbooks, episodes and the original filter values.
//!
//! On the way out the sealed part is opened and merged back, so [`crate::Memory`] sees the payload it wrote.
//! A point that fails to open (wrong key, tampering, moved to another id) is skipped, never returned.
//!
//! The clear fields are readable by the server but not editable without notice: from `v2` on their values
//! are part of the associated data, so flipping `tenant` (to hand one tenant's facts to another) or
//! inflating `wins` (to make `prune` keep junk) makes the point fail to open. `v1` blobs, written by earlier
//! builds, do not have that protection, so this build **refuses to read them** (they are skipped, and
//! [`probe_key`] reports [`KeyProbe::NeedsUpgrade`]). [`migrate_sealed`] (`ostler memory seal`) opens them
//! once, with the right key, and saves them again as `v2`.
//!
//! Payload rules (checked on write, because a violation would put text in clear on the server or lose data):
//! `_sealed` is reserved; `tenant` must be a string; `wins`, `losses` and `verified_ts` must be unsigned
//! integers; and the sealed part must read back identically (so absurd nesting, or a float that does not
//! survive a JSON round trip, is refused instead of stored unreadable).
//!
//! What this does **not** hide: the vector. The `text` passed to `upsert`/`search` is embedded by Qdrant
//! Cloud Inference (so the server sees it) or by a local model (so only the vector leaves). For full
//! privacy pair this with a local embedder. Point counts, timing, tenant name and win/loss counters are
//! also visible to whoever runs the server.

use std::sync::Arc;

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::Aes256Gcm;
use async_trait::async_trait;
use base64::engine::general_purpose::{STANDARD_NO_PAD as B64, URL_SAFE_NO_PAD as B64URL};
use base64::Engine;
use hmac::{Hmac, Mac};
use serde_json::{json, Map, Value};
use sha2::Sha256;

use crate::store::{Filter, Hit, Kind, Rank, Store};
use crate::{MemoryError, Result};

const CLEAR: &[&str] = &["tenant", "wins", "losses", "verified_ts"];
const HASHED: &[&str] = &[
    "key_norm",
    "fingerprint",
    "autocomplete",
    "id_hint",
    "host",
    "labels_norm",
];
const SEALED: &str = "_sealed";
/// Written by this build: associated data covers the point id and the clear fields.
const VERSION: &str = "v2";
/// Earlier format: associated data is the point id only. Not readable by default; see [`migrate_sealed`].
const VERSION_V1: &str = "v1";
/// Most points an adaptive read will fetch while skipping ones this key cannot open.
const READ_CAP: usize = 100_000;

/// A 32-byte master key. Two independent sub-keys are derived from it: one to encrypt, one to hash.
#[derive(Clone)]
pub struct SealKey {
    enc: [u8; 32],
    idx: [u8; 32],
}

impl std::fmt::Debug for SealKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SealKey(..)")
    }
}

pub(crate) fn hmac(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut m =
        <Hmac<Sha256> as KeyInit>::new_from_slice(key).expect("hmac accepts any key length");
    for p in parts {
        m.update(p);
    }
    m.finalize().into_bytes().into()
}

impl SealKey {
    pub fn from_bytes(master: [u8; 32]) -> Self {
        Self {
            enc: hmac(&master, &[b"ostler/seal/enc/v1"]),
            idx: hmac(&master, &[b"ostler/seal/idx/v1"]),
        }
    }

    /// A fresh random master key.
    pub fn generate() -> Result<[u8; 32]> {
        let mut k = [0u8; 32];
        getrandom::fill(&mut k).map_err(|e| MemoryError::Invalid(format!("no randomness: {e}")))?;
        Ok(k)
    }

    /// Parse the base64 form produced by [`SealKey::encode`].
    pub fn decode(s: &str) -> Result<[u8; 32]> {
        let raw = B64
            .decode(s.trim().trim_end_matches('='))
            .map_err(|_| MemoryError::Invalid("memory key is not valid base64".into()))?;
        <[u8; 32]>::try_from(raw.as_slice())
            .map_err(|_| MemoryError::Invalid("memory key must be 32 bytes".into()))
    }

    pub fn encode(master: &[u8; 32]) -> String {
        B64.encode(master)
    }

    fn cipher(&self) -> Aes256Gcm {
        Aes256Gcm::new_from_slice(&self.enc).expect("32-byte key")
    }

    fn hash(&self, field: &str, value: &str) -> String {
        let h = hmac(&self.idx, &[field.as_bytes(), &[0x1f], value.as_bytes()]);
        B64URL.encode(&h[..16])
    }

    /// Hash of a non-string value. A different domain from [`SealKey::hash`], so the number `5` and the
    /// string `"5"`, or `null` and `"null"`, never share a hash (and so never match each other's filters).
    fn hash_json(&self, field: &str, v: &Value) -> String {
        let h = hmac(
            &self.idx,
            &[
                b"json",
                &[0x1f],
                field.as_bytes(),
                &[0x1f],
                v.to_string().as_bytes(),
            ],
        );
        B64URL.encode(&h[..16])
    }

    fn hash_value(&self, field: &str, v: &Value) -> Value {
        match v {
            Value::String(s) => json!(self.hash(field, s)),
            Value::Array(a) => Value::Array(a.iter().map(|x| self.hash_value(field, x)).collect()),
            other => json!(self.hash_json(field, other)),
        }
    }

    /// The id a point is stored under. Derived with the index key, so the server cannot test guesses such as
    /// `uuid5("default", "profile", "passport number")` against the ids it sees.
    fn point_id(&self, kind: Kind, id: &str) -> String {
        let h = hmac(
            &self.idx,
            &[
                b"id",
                &[0x1f],
                kind.collection().as_bytes(),
                &[0x1f],
                id.as_bytes(),
            ],
        );
        let mut b = [0u8; 16];
        b.copy_from_slice(&h[..16]);
        uuid::Uuid::from_bytes(b).to_string()
    }

    /// Encrypt with the current format (`v2`). `aad` is bound but not stored.
    pub(crate) fn seal(&self, aad: &str, plaintext: &[u8]) -> Result<String> {
        let mut nonce = [0u8; 12];
        getrandom::fill(&mut nonce)
            .map_err(|e| MemoryError::Invalid(format!("no randomness: {e}")))?;
        let ct = self
            .cipher()
            .encrypt(
                (&nonce).into(),
                Payload {
                    msg: plaintext,
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| MemoryError::Invalid("encryption failed".into()))?;
        Ok(format!(
            "{VERSION}.{}.{}",
            B64.encode(nonce),
            B64.encode(ct)
        ))
    }

    /// Same as [`SealKey::seal`] but in the retired `v1` format. Only for migration tests.
    fn seal_v1(&self, aad: &str, plaintext: &[u8]) -> Result<String> {
        let sealed = self.seal(aad, plaintext)?;
        // `seal` binds `aad` whatever the label says, so relabelling is enough to make a faithful v1 blob.
        Ok(format!(
            "{VERSION_V1}{}",
            sealed.strip_prefix(VERSION).unwrap_or(&sealed)
        ))
    }

    /// Decrypt. `aad_v1` is used for a `v1` blob (only when `allow_v1`), `aad_v2` for a `v2` one; any other
    /// version, a malformed blob, or trailing parts yield `None`.
    pub(crate) fn open(
        &self,
        aad_v1: &str,
        aad_v2: &str,
        sealed: &str,
        allow_v1: bool,
    ) -> Option<Vec<u8>> {
        let mut parts = sealed.split('.');
        let aad = match parts.next()? {
            VERSION => aad_v2,
            VERSION_V1 if allow_v1 => aad_v1,
            _ => return None,
        };
        let nonce: [u8; 12] = B64.decode(parts.next()?).ok()?.try_into().ok()?;
        let ct = B64.decode(parts.next()?).ok()?;
        if parts.next().is_some() {
            return None;
        }
        self.cipher()
            .decrypt(
                (&nonce).into(),
                Payload {
                    msg: &ct,
                    aad: aad.as_bytes(),
                },
            )
            .ok()
    }
}

pub struct SealedStore {
    inner: Arc<dyn Store>,
    key: SealKey,
    /// Open retired `v1` blobs too. Only the migration sets this.
    accept_v1: bool,
}

impl SealedStore {
    pub fn new(inner: Arc<dyn Store>, master: [u8; 32]) -> Self {
        Self {
            inner,
            key: SealKey::from_bytes(master),
            accept_v1: false,
        }
    }

    /// A view that also opens `v1` blobs. For [`migrate_sealed`] and [`probe_key`]; ordinary reads must not.
    fn accepting_v1(mut self) -> Self {
        self.accept_v1 = true;
        self
    }

    /// Associated data of a `v1` blob: which point it belongs to.
    fn aad_v1(kind: Kind, id: &str) -> String {
        format!("{}/{id}", kind.collection())
    }

    /// Associated data of a `v2` blob: the point, plus the clear fields as the server holds them. Each is
    /// length-prefixed so no two different sets of values can produce the same string.
    fn aad_v2(kind: Kind, id: &str, clear: &Map<String, Value>) -> String {
        let mut a = Self::aad_v1(kind, id);
        for k in CLEAR {
            let v = match clear.get(*k) {
                None => "-".to_string(),
                Some(Value::String(s)) => format!("s{}:{s}", s.len()),
                Some(Value::Number(n)) => match n.as_u64() {
                    Some(u) => format!("n{u}"),
                    None => format!("f{n}"),
                },
                Some(other) => {
                    let t = other.to_string();
                    format!("j{}:{t}", t.len())
                }
            };
            a.push('\u{1f}');
            a.push_str(&v);
        }
        a
    }

    /// Open a stored point's blob, if this key can.
    fn open_stored(&self, kind: Kind, id: &str, stored: &Map<String, Value>) -> Option<Vec<u8>> {
        let Some(Value::String(sealed)) = stored.get(SEALED) else {
            return None;
        };
        self.key.open(
            &Self::aad_v1(kind, id),
            &Self::aad_v2(kind, id, stored),
            sealed,
            self.accept_v1,
        )
    }

    fn seal_payload(&self, kind: Kind, id: &str, payload: Value) -> Result<Value> {
        self.seal_payload_as(kind, id, payload, false)
    }

    fn seal_payload_as(&self, kind: Kind, id: &str, payload: Value, v1: bool) -> Result<Value> {
        let Value::Object(obj) = payload else {
            return Err(MemoryError::Invalid("payload must be an object".into()));
        };
        if obj.contains_key(SEALED) {
            return Err(MemoryError::Invalid(format!(
                "{SEALED:?} is a reserved payload field"
            )));
        }
        // These stay in clear on the server, so they must be exactly what Qdrant needs and nothing else.
        if obj.get("tenant").is_some_and(|v| !v.is_string()) {
            return Err(MemoryError::Invalid("`tenant` must be a string".into()));
        }
        for f in ["wins", "losses", "verified_ts"] {
            if obj.get(f).is_some_and(|v| v.as_u64().is_none()) {
                return Err(MemoryError::Invalid(format!(
                    "`{f}` is stored in clear and must be an unsigned integer"
                )));
            }
        }
        let mut out = Map::new();
        let mut secret = Map::new();
        for (k, v) in obj {
            if CLEAR.contains(&k.as_str()) {
                out.insert(k, v);
                continue;
            }
            if HASHED.contains(&k.as_str()) {
                out.insert(k.clone(), self.key.hash_value(&k, &v));
            }
            secret.insert(k, v);
        }
        let secret = Value::Object(secret);
        let bytes = serde_json::to_vec(&secret).map_err(|e| MemoryError::Invalid(e.to_string()))?;
        // Never store what cannot be read back: serde_json refuses deep nesting on parse and, without its
        // `float_roundtrip` feature, can change the last digit of a float. Failing now beats losing it later.
        if serde_json::from_slice::<Value>(&bytes).ok().as_ref() != Some(&secret) {
            return Err(MemoryError::Invalid(
                "this value would not read back identically (nesting deeper than 128, or a number JSON cannot round-trip exactly); it was not saved".into(),
            ));
        }
        let blob = if v1 {
            self.key.seal_v1(&Self::aad_v1(kind, id), &bytes)?
        } else {
            self.key.seal(&Self::aad_v2(kind, id, &out), &bytes)?
        };
        out.insert(SEALED.into(), json!(blob));
        Ok(Value::Object(out))
    }

    /// `None` when the point cannot be opened with this key.
    fn unseal(&self, kind: Kind, hit: Hit) -> Option<Hit> {
        let Value::Object(mut stored) = hit.payload.clone() else {
            return None;
        };
        // Written before sealing was on: its filter fields are plaintext and will never match a
        // hashed query. Hide it rather than hand back data that bypasses the seal.
        let bytes = self.open_stored(kind, &hit.id, &stored)?;
        let Value::Object(secret) = serde_json::from_slice(&bytes).ok()? else {
            return None;
        };
        // The lookup fields travel in clear as keyed hashes and are not part of the AEAD data. Recompute
        // them from what was actually sealed: a server that swapped the hashes of two points (so a lookup
        // for "email" returns the "home address" fact, or a playbook moves to another site) is caught here.
        for f in HASHED {
            let want = secret.get(*f).map(|v| self.key.hash_value(f, v));
            if want.as_ref() != stored.get(*f) {
                return None;
            }
        }
        stored.remove(SEALED);
        let mut payload = secret;
        for k in CLEAR {
            if let Some(v) = stored.remove(*k) {
                payload.insert((*k).into(), v);
            }
        }
        Some(Hit {
            payload: Value::Object(payload),
            ..hit
        })
    }

    fn unseal_all(&self, kind: Kind, hits: Vec<Hit>) -> Vec<Hit> {
        hits.into_iter()
            .filter_map(|h| self.unseal(kind, h))
            .collect()
    }

    /// Read `limit` openable points. Points this key cannot open (another key's, tampered, or written before
    /// sealing) are dropped after the server applied its limit, so a plain read could return nothing while
    /// openable points exist. Ask for more until enough opened or the server has no more.
    async fn read<'a, F, Fut>(&'a self, kind: Kind, limit: usize, fetch: F) -> Result<Vec<Hit>>
    where
        F: Fn(usize) -> Fut,
        Fut: std::future::Future<Output = Result<Vec<Hit>>> + 'a,
    {
        if limit == 0 {
            return Ok(vec![]);
        }
        let cap = limit.max(READ_CAP);
        let mut want = limit;
        loop {
            let hits = fetch(want).await?;
            let fetched = hits.len();
            let mut opened = self.unseal_all(kind, hits);
            if opened.len() >= limit || fetched < want || want >= cap {
                opened.truncate(limit);
                return Ok(opened);
            }
            want = want.saturating_mul(4).min(cap);
        }
    }

    fn filter(&self, f: &Filter) -> Filter {
        Filter {
            must: f
                .must
                .iter()
                .map(|(k, v)| {
                    if HASHED.contains(&k.as_str()) {
                        (k.clone(), self.key.hash_value(k, v))
                    } else {
                        (k.clone(), v.clone())
                    }
                })
                .collect(),
            below: f.below.clone(),
        }
    }

    /// Points in `kind` for this tenant that were stored without sealing (a store created before
    /// sealing was turned on). Used by the migration; `Memory` never sees them.
    pub async fn count_unsealed(&self, kind: Kind, tenant: &str) -> Result<usize> {
        let hits = crate::scroll_all(self.inner.as_ref(), kind, &Filter::tenant(tenant)).await?;
        Ok(hits
            .iter()
            .filter(|h| h.payload.get(SEALED).is_none())
            .count())
    }
}

#[async_trait]
impl Store for SealedStore {
    fn degraded(&self) -> Option<String> {
        self.inner.degraded()
    }

    async fn ensure(&self) -> Result<()> {
        self.inner.ensure().await
    }

    async fn upsert(&self, kind: Kind, id: &str, text: &str, payload: Value) -> Result<()> {
        let id = self.key.point_id(kind, id);
        let sealed = self.seal_payload(kind, &id, payload)?;
        self.inner.upsert(kind, &id, text, sealed).await
    }

    async fn search(
        &self,
        kind: Kind,
        text: &str,
        filter: &Filter,
        limit: usize,
    ) -> Result<Vec<Hit>> {
        let f = self.filter(filter);
        self.read(kind, limit, |n| self.inner.search(kind, text, &f, n))
            .await
    }

    async fn search_ranked(
        &self,
        kind: Kind,
        text: &str,
        filter: &Filter,
        limit: usize,
        rank: &Rank,
    ) -> Result<Vec<Hit>> {
        let f = self.filter(filter);
        self.read(kind, limit, |n| {
            self.inner.search_ranked(kind, text, &f, n, rank)
        })
        .await
    }

    async fn scroll(&self, kind: Kind, filter: &Filter, limit: usize) -> Result<Vec<Hit>> {
        let f = self.filter(filter);
        self.read(kind, limit, |n| self.inner.scroll(kind, &f, n))
            .await
    }

    async fn delete(&self, kind: Kind, id: &str) -> Result<()> {
        // `id` is either one Memory computed (stored under its keyed form) or a `Hit::id` (already keyed).
        // Deleting a point that does not exist is a no-op, so remove both.
        self.inner
            .delete(kind, &self.key.point_id(kind, id))
            .await?;
        self.inner.delete(kind, id).await
    }

    async fn delete_matching(&self, kind: Kind, filter: &Filter) -> Result<()> {
        self.inner.delete_matching(kind, &self.filter(filter)).await
    }

    async fn count(&self, kind: Kind, filter: &Filter) -> Result<u64> {
        self.inner.count(kind, &self.filter(filter)).await
    }
}

/// Whether a key can open the sealed points already stored for a tenant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyProbe {
    /// No sealed points for this tenant: any key is safe to start with.
    Empty,
    /// Every sealed point opens with this key and is in the current format.
    Ok,
    /// Every point opens with this key, but some are in the retired `v1` format (not read any more) or
    /// were saved under an older key normalisation (their fact key no longer maps to their id, so a lookup
    /// misses them). `ostler memory seal` ([`migrate_sealed`]) fixes both.
    NeedsUpgrade { v1: usize, stale_keys: usize },
    /// Sealed points exist that this key cannot open (wrong, replaced or regenerated key, or tampering).
    /// Writing with this key would shadow them, and a later wipe or prune would destroy them.
    Mismatch { unopenable: usize },
}

/// Does an opened point sit under an id / lookup value that today's normalisation would not produce?
/// (Facts: `key_norm` and the id come from `norm(key)`; playbooks: `labels_norm` from the field labels.)
fn is_stale(sealed: &SealedStore, kind: Kind, tenant: &str, id: &str, secret: &Value) -> bool {
    match kind {
        Kind::Profile => {
            let Some(key) = secret["key"].as_str() else {
                return false;
            };
            let want = crate::norm(key);
            secret["key_norm"].as_str() != Some(want.as_str())
                || id
                    != sealed
                        .key
                        .point_id(kind, &crate::point_id(&[tenant, "profile", &want]))
        }
        Kind::Playbook => {
            let Some(fields) = secret["playbook"]["fields"].as_array() else {
                return false;
            };
            let want: Vec<Value> = fields
                .iter()
                .map(|f| json!(crate::norm(f["label"].as_str().unwrap_or(""))))
                .collect();
            secret["labels_norm"].as_array() != Some(&want)
        }
        Kind::Episode => false,
    }
}

/// Check, without changing anything, whether `master` opens the tenant's existing sealed points.
/// Unsealed (legacy) points are ignored; see [`SealedStore::count_unsealed`].
pub async fn probe_key(raw: Arc<dyn Store>, master: [u8; 32], tenant: &str) -> Result<KeyProbe> {
    let sealed = SealedStore::new(raw.clone(), master).accepting_v1();
    let (mut total, mut unopenable, mut v1, mut stale) = (0usize, 0usize, 0usize, 0usize);
    for kind in Kind::ALL {
        for h in crate::scroll_all(raw.as_ref(), kind, &Filter::tenant(tenant)).await? {
            let Value::Object(stored) = &h.payload else {
                continue;
            };
            let Some(Value::String(blob)) = stored.get(SEALED) else {
                continue;
            };
            total += 1;
            let Some(bytes) = sealed.open_stored(kind, &h.id, stored) else {
                unopenable += 1;
                continue;
            };
            if blob.starts_with(&format!("{VERSION_V1}.")) {
                v1 += 1;
            }
            if let Ok(secret) = serde_json::from_slice::<Value>(&bytes) {
                if is_stale(&sealed, kind, tenant, &h.id, &secret) {
                    stale += 1;
                }
            }
        }
    }
    Ok(if total == 0 {
        KeyProbe::Empty
    } else if unopenable > 0 {
        KeyProbe::Mismatch { unopenable }
    } else if v1 > 0 || stale > 0 {
        KeyProbe::NeedsUpgrade {
            v1,
            stale_keys: stale,
        }
    } else {
        KeyProbe::Ok
    })
}

/// What [`migrate_sealed`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MigrateReport {
    /// `v1` points saved again as `v2`.
    pub v1_upgraded: usize,
    /// Points whose fact key / label lookup value no longer matched today's normalisation, saved again under
    /// the current one (the stray old copy is removed).
    pub renormalized: usize,
    /// Points that could not be saved again (unreadable payload) and were left untouched.
    pub skipped: usize,
}

/// Bring a tenant's sealed points up to date: re-save `v1` blobs as `v2`, and re-save facts and playbooks
/// whose lookup keys were derived with an older `norm()` (keys with emoji or decomposed accents used to
/// normalise differently, which left their points unreachable by today's lookups).
///
/// Only points this key opens are touched; others are never deleted. A point is written again first and
/// the old copy removed only when it sits under a different id and the new one reads back. A fact never
/// overwrites a newer one already stored under its current id. Safe to run again after an interruption.
pub async fn migrate_sealed(
    raw: Arc<dyn Store>,
    master: [u8; 32],
    tenant: &str,
) -> Result<MigrateReport> {
    let reader = SealedStore::new(raw.clone(), master).accepting_v1();
    let writer: Arc<dyn Store> = Arc::new(SealedStore::new(raw.clone(), master));
    let mem = crate::Memory::new(writer, tenant);
    let mut rep = MigrateReport::default();
    for kind in Kind::ALL {
        for h in crate::scroll_all(raw.as_ref(), kind, &Filter::tenant(tenant)).await? {
            let is_v1 = matches!(h.payload.get(SEALED), Some(Value::String(b)) if b.starts_with(&format!("{VERSION_V1}.")));
            if h.payload.get(SEALED).is_none() {
                continue; // unsealed legacy points are `seal_legacy`'s business
            }
            let old_id = h.id.clone();
            let Some(opened) = reader.unseal(kind, h) else {
                continue;
            };
            let stale = is_stale(&reader, kind, tenant, &old_id, &opened.payload);
            if !is_v1 && !stale {
                continue;
            }
            let Some(new_mem_id) = mem.resave(kind, &opened.payload).await? else {
                rep.skipped += 1;
                continue;
            };
            let new_id = reader.key.point_id(kind, &new_mem_id);
            if new_id != old_id {
                // The point now lives under a different id: confirm it is there before dropping the old one.
                let there = crate::scroll_all(raw.as_ref(), kind, &Filter::tenant(tenant))
                    .await?
                    .iter()
                    .any(|x| x.id == new_id);
                if !there {
                    return Err(MemoryError::Invalid(format!(
                        "a point in {} did not read back after re-saving; the old copy was left in place",
                        kind.collection()
                    )));
                }
                raw.delete(kind, &old_id).await?;
            }
            if is_v1 {
                rep.v1_upgraded += 1;
            }
            if stale {
                rep.renormalized += 1;
            }
        }
    }
    Ok(rep)
}

/// Write one point in the retired `v1` format, as an older build did. Only for tests of the migration
/// (here and in the CLI); not part of the API.
#[doc(hidden)]
pub async fn write_v1_point_for_tests(
    raw: Arc<dyn Store>,
    master: [u8; 32],
    kind: Kind,
    id: &str,
    text: &str,
    payload: Value,
) -> Result<()> {
    let s = SealedStore::new(raw.clone(), master);
    let id = s.key.point_id(kind, id);
    let sealed = s.seal_payload_as(kind, &id, payload, true)?;
    raw.upsert(kind, &id, text, sealed).await
}

/// What [`seal_legacy`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SealReport {
    /// Facts saved before sealing, now re-saved sealed.
    pub facts_migrated: usize,
    /// Unsealed points removed from the server: the old facts, plus old playbooks and episodes (not
    /// carried over; they are relearned).
    pub legacy_points_removed: usize,
}

/// Move a tenant's pre-sealing memory into sealed form without ever holding the only copy in RAM.
///
/// Order matters: (1) read the legacy plaintext facts, (2) write sealed copies, (3) read them back through
/// the sealed view and check every one is there, and only then (4) delete the unsealed points. A crash or
/// error before step 4 leaves both copies; running it again finishes the job. Sealed points are never
/// deleted, so a wrong key or a half-finished earlier run cannot destroy them.
pub async fn seal_legacy(
    raw: Arc<dyn Store>,
    master: [u8; 32],
    tenant: &str,
) -> Result<SealReport> {
    const MAX: usize = 100_000;
    let sealed: Arc<dyn Store> = Arc::new(SealedStore::new(raw.clone(), master));
    let mem = crate::Memory::new(sealed, tenant);

    let mut legacy_ids: Vec<(Kind, String)> = vec![];
    let mut facts = vec![];
    for kind in Kind::ALL {
        let hits = raw.scroll(kind, &Filter::tenant(tenant), MAX).await?;
        if hits.len() >= MAX {
            return Err(MemoryError::Invalid(format!(
                "more than {MAX} points in {}: refusing to migrate a partial view",
                kind.collection()
            )));
        }
        for h in hits.into_iter().filter(|h| h.payload.get(SEALED).is_none()) {
            if kind == Kind::Profile {
                if let Some(f) = crate::Fact::from_payload(&h.payload) {
                    facts.push(f);
                }
            }
            legacy_ids.push((kind, h.id));
        }
    }
    for f in &facts {
        // A fact saved after sealing was turned on is newer than the plaintext copy that was hidden from
        // you meanwhile; migrating the old one over it would silently bring back stale data (also on a
        // rerun after an interruption).
        if let Some(existing) = mem.fact(&f.key).await? {
            if existing.updated_at >= f.updated_at {
                continue;
            }
        }
        mem.put_fact(
            &f.key,
            f.value.clone(),
            f.description.as_deref(),
            f.autocomplete.as_deref(),
            &f.source,
            Some(f.updated_at),
        )
        .await?;
    }
    let now_sealed: std::collections::HashSet<String> =
        mem.facts(MAX).await?.into_iter().map(|f| f.key).collect();
    if let Some(missing) = facts.iter().find(|f| !now_sealed.contains(&f.key)) {
        return Err(MemoryError::Invalid(format!(
            "fact {:?} did not read back after sealing; the old copies were left in place",
            missing.key
        )));
    }
    for (kind, id) in &legacy_ids {
        raw.delete(*kind, id).await?;
    }
    Ok(SealReport {
        facts_migrated: facts.len(),
        legacy_points_removed: legacy_ids.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HashEmbedder, InMemoryStore, Memory};

    fn pair(key: [u8; 32]) -> (Arc<InMemoryStore>, Memory) {
        let raw = Arc::new(InMemoryStore::new(Arc::new(HashEmbedder::default())));
        let sealed: Arc<dyn Store> = Arc::new(SealedStore::new(raw.clone(), key));
        (raw, Memory::new(sealed, "t1"))
    }

    #[tokio::test]
    async fn server_side_data_holds_no_plaintext_but_memory_round_trips() {
        let (raw, m) = pair([7; 32]);
        m.remember(
            "email",
            json!("ada@example.com"),
            Some("work email"),
            None,
            "user",
        )
        .await
        .unwrap();
        m.record_episode(
            "shop.example",
            "fp1",
            "ZIP",
            "validation_error",
            "ZIP must be 5 digits",
        )
        .await
        .unwrap();

        // What the server holds, for every kind.
        for kind in Kind::ALL {
            for h in raw.scroll(kind, &Filter::tenant("t1"), 100).await.unwrap() {
                // The id must not be the guessable unkeyed one.
                assert_ne!(h.id, crate::point_id(&["t1", "profile", "email"]));
                let s = h.payload.to_string();
                for leak in [
                    "ada@example.com",
                    "work email",
                    "shop.example",
                    "ZIP",
                    "email",
                ] {
                    assert!(!s.contains(leak), "{kind:?} payload leaks {leak:?}: {s}");
                }
                assert_eq!(h.payload["tenant"], "t1");
            }
        }

        // What Memory sees.
        let facts = m.facts(10).await.unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].value, json!("ada@example.com"));
        assert_eq!(facts[0].description.as_deref(), Some("work email"));
        let eps = m.episodes("fp1", 10).await.unwrap();
        assert_eq!(eps.len(), 1);
        assert_eq!(eps[0].host, "shop.example");
    }

    #[tokio::test]
    async fn dictionary_guessed_ids_do_not_match_and_prune_and_forget_still_work() {
        let (raw, m) = pair([7; 32]);
        m.remember("passport number", json!("X1"), None, None, "user")
            .await
            .unwrap();
        let guess = crate::point_id(&["t1", "profile", "passport number"]);
        let ids: Vec<String> = raw
            .scroll(Kind::Profile, &Filter::tenant("t1"), 10)
            .await
            .unwrap()
            .into_iter()
            .map(|h| h.id)
            .collect();
        assert_eq!(ids.len(), 1);
        assert_ne!(ids[0], guess);
        assert!(m.forget("passport number").await.unwrap());
        assert!(m.facts(10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn seal_legacy_keeps_every_fact_and_never_touches_sealed_points() {
        let raw: Arc<InMemoryStore> =
            Arc::new(InMemoryStore::new(Arc::new(HashEmbedder::default())));
        // Legacy: plaintext memory, including a key the secret rule would refuse today.
        let plain = Memory::new(raw.clone(), "t1");
        plain
            .remember("email", json!("a@b.co"), None, None, "user")
            .await
            .unwrap();
        plain
            .put_fact(
                "shipping address",
                json!("1 Road"),
                None,
                None,
                "user",
                Some(42),
            )
            .await
            .unwrap();
        plain
            .record_episode("old.example", "fp", "ZIP", "validation_error", "bad")
            .await
            .unwrap();
        // Already sealed from an earlier run.
        let sealed: Arc<dyn Store> = Arc::new(SealedStore::new(raw.clone(), [3; 32]));
        let m = Memory::new(sealed, "t1");
        m.remember("city", json!("Pune"), None, None, "user")
            .await
            .unwrap();

        let rep = seal_legacy(raw.clone(), [3; 32], "t1").await.unwrap();
        assert_eq!(rep.facts_migrated, 2);
        assert_eq!(rep.legacy_points_removed, 3); // two facts + one episode

        let keys: Vec<String> = m
            .facts(10)
            .await
            .unwrap()
            .into_iter()
            .map(|f| f.key)
            .collect();
        assert_eq!(keys.len(), 3, "{keys:?}"); // city survived, both legacy facts carried over
        let moved = m.fact("shipping address").await.unwrap().unwrap();
        assert_eq!(moved.updated_at, 42);
        // Nothing unsealed is left on the server, and running it again is a no-op.
        for kind in Kind::ALL {
            for h in raw.scroll(kind, &Filter::tenant("t1"), 100).await.unwrap() {
                assert!(
                    h.payload.get("_sealed").is_some(),
                    "{kind:?} still has plaintext"
                );
            }
        }
        let again = seal_legacy(raw, [3; 32], "t1").await.unwrap();
        assert_eq!((again.facts_migrated, again.legacy_points_removed), (0, 0));
    }

    #[tokio::test]
    async fn filters_still_work_on_hashed_fields() {
        let (_raw, m) = pair([7; 32]);
        m.remember("email", json!("a@b.co"), None, None, "user")
            .await
            .unwrap();
        m.remember("city", json!("Pune"), None, None, "user")
            .await
            .unwrap();
        assert!(m.forget("city").await.is_ok());
        let left = m.facts(10).await.unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].key, "email");
    }

    #[tokio::test]
    async fn wrong_key_sees_nothing_and_tampering_is_rejected() {
        let raw = Arc::new(InMemoryStore::new(Arc::new(HashEmbedder::default())));
        let a: Arc<dyn Store> = Arc::new(SealedStore::new(raw.clone(), [1; 32]));
        let b: Arc<dyn Store> = Arc::new(SealedStore::new(raw.clone(), [2; 32]));
        Memory::new(a, "t1")
            .remember("email", json!("a@b.co"), None, None, "user")
            .await
            .unwrap();
        assert!(Memory::new(b, "t1").facts(10).await.unwrap().is_empty());

        // Move a sealed blob to another point id: the id is bound as associated data, so it must not open.
        let key = SealKey::from_bytes([1; 32]);
        let s = key.seal("ostler_profile/id-1", b"{}").unwrap();
        assert!(key.open("x", "ostler_profile/id-1", &s, false).is_some());
        assert!(key
            .open("ostler_profile/id-1", "ostler_profile/id-2", &s, true)
            .is_none());
    }

    #[tokio::test]
    async fn probe_key_tells_empty_ok_and_mismatch_apart() {
        let raw = Arc::new(InMemoryStore::new(Arc::new(HashEmbedder::default())));
        let dynraw: Arc<dyn Store> = raw.clone();
        assert_eq!(
            probe_key(dynraw.clone(), [1; 32], "t1").await.unwrap(),
            KeyProbe::Empty
        );
        // Legacy plaintext alone does not count as sealed data.
        Memory::new(raw.clone(), "t1")
            .remember("email", json!("a@b.co"), None, None, "user")
            .await
            .unwrap();
        assert_eq!(
            probe_key(dynraw.clone(), [1; 32], "t1").await.unwrap(),
            KeyProbe::Empty
        );
        let a: Arc<dyn Store> = Arc::new(SealedStore::new(raw.clone(), [1; 32]));
        let m = Memory::new(a, "t1");
        m.remember("city", json!("Pune"), None, None, "user")
            .await
            .unwrap();
        m.record_episode("shop.example", "fp1", "ZIP", "validation_error", "bad")
            .await
            .unwrap();
        assert_eq!(
            probe_key(dynraw.clone(), [1; 32], "t1").await.unwrap(),
            KeyProbe::Ok
        );
        assert_eq!(
            probe_key(dynraw.clone(), [2; 32], "t1").await.unwrap(),
            KeyProbe::Mismatch { unopenable: 2 }
        );
        // Another tenant's data is not this tenant's problem.
        assert_eq!(
            probe_key(dynraw, [2; 32], "other").await.unwrap(),
            KeyProbe::Empty
        );
    }

    #[tokio::test]
    async fn probe_key_flags_a_tampered_point_even_with_the_right_key() {
        let raw = Arc::new(InMemoryStore::new(Arc::new(HashEmbedder::default())));
        let a: Arc<dyn Store> = Arc::new(SealedStore::new(raw.clone(), [1; 32]));
        Memory::new(a, "t1")
            .remember("city", json!("Pune"), None, None, "user")
            .await
            .unwrap();
        let hit = raw
            .scroll(Kind::Profile, &Filter::tenant("t1"), 10)
            .await
            .unwrap()
            .remove(0);
        let mut payload = hit.payload.clone();
        payload["_sealed"] = json!("v1.AAAA.AAAA");
        raw.upsert(Kind::Profile, &hit.id, "x", payload)
            .await
            .unwrap();
        assert_eq!(
            probe_key(raw, [1; 32], "t1").await.unwrap(),
            KeyProbe::Mismatch { unopenable: 1 }
        );
    }

    #[test]
    fn key_round_trips_and_rejects_bad_input() {
        let k = SealKey::generate().unwrap();
        assert_eq!(SealKey::decode(&SealKey::encode(&k)).unwrap(), k);
        assert!(SealKey::decode("not base64!!").is_err());
        assert!(SealKey::decode("AAAA").is_err());
    }
}

#[cfg(test)]
mod review_tests {
    use super::*;
    use crate::{HashEmbedder, InMemoryStore, Memory};

    #[tokio::test]
    async fn a_server_that_swaps_lookup_hashes_cannot_make_one_fact_answer_for_another() {
        let raw = Arc::new(InMemoryStore::new(Arc::new(HashEmbedder::default())));
        let sealed: Arc<dyn Store> = Arc::new(SealedStore::new(raw.clone(), [5; 32]));
        let m = Memory::new(sealed, "t1");
        m.remember("email", json!("a@b.co"), None, None, "user")
            .await
            .unwrap();
        m.remember("home address", json!("1 Secret Rd"), None, None, "user")
            .await
            .unwrap();
        let mut pts = raw
            .scroll(Kind::Profile, &Filter::tenant("t1"), 10)
            .await
            .unwrap();
        assert_eq!(pts.len(), 2);
        let (a, b) = (pts.remove(0), pts.remove(0));
        // The server swaps key_norm between the two points (everything else, ciphertext included, untouched).
        let (ka, kb) = (a.payload["key_norm"].clone(), b.payload["key_norm"].clone());
        let mut pa = a.payload.clone();
        let mut pb = b.payload.clone();
        pa["key_norm"] = kb;
        pb["key_norm"] = ka;
        raw.replace_payload_for_test(&a.id, pa);
        raw.replace_payload_for_test(&b.id, pb);
        let got = m.fact("email").await.unwrap();
        assert!(got.is_none(), "a swapped hash answered a lookup: {got:?}");
    }

    #[tokio::test]
    async fn seal_legacy_does_not_overwrite_a_newer_sealed_fact() {
        let raw: Arc<InMemoryStore> =
            Arc::new(InMemoryStore::new(Arc::new(HashEmbedder::default())));
        let plain = Memory::new(raw.clone(), "t1");
        plain
            .put_fact(
                "email",
                json!("OLD@legacy.co"),
                None,
                None,
                "user",
                Some(100),
            )
            .await
            .unwrap();
        let sealed: Arc<dyn Store> = Arc::new(SealedStore::new(raw.clone(), [3; 32]));
        let m = Memory::new(sealed, "t1");
        m.put_fact("email", json!("NEW@now.co"), None, None, "user", Some(200))
            .await
            .unwrap();
        seal_legacy(raw, [3; 32], "t1").await.unwrap();
        assert_eq!(
            m.fact("email").await.unwrap().unwrap().value,
            json!("NEW@now.co")
        );
    }

    fn fact_payload(key: &str, key_norm: &str, value: &str, at: u64) -> Value {
        json!({"tenant": "t1", "key": key, "key_norm": key_norm, "value": value,
               "description": null, "autocomplete": null, "source": "user", "updated_at": at})
    }

    fn fresh() -> (Arc<InMemoryStore>, Memory) {
        let raw = Arc::new(InMemoryStore::new(Arc::new(HashEmbedder::default())));
        let sealed: Arc<dyn Store> = Arc::new(SealedStore::new(raw.clone(), [5; 32]));
        (raw, Memory::new(sealed, "t1"))
    }

    #[tokio::test]
    async fn v1_blobs_are_not_read_are_reported_and_migrate_to_v2() {
        let (raw, m) = fresh();
        let dynraw: Arc<dyn Store> = raw.clone();
        let id = crate::point_id(&["t1", "profile", "city"]);
        write_v1_point_for_tests(
            dynraw.clone(),
            [5; 32],
            Kind::Profile,
            &id,
            "city",
            fact_payload("city", "city", "Pune", 7),
        )
        .await
        .unwrap();
        // The stored blob really is v1, and the key is right.
        let stored = raw
            .scroll(Kind::Profile, &Filter::tenant("t1"), 5)
            .await
            .unwrap();
        assert!(stored[0].payload["_sealed"]
            .as_str()
            .unwrap()
            .starts_with("v1."));
        // Not readable through the normal view, by lookup or by listing.
        assert!(m.fact("city").await.unwrap().is_none());
        assert!(m.facts(10).await.unwrap().is_empty());
        assert_eq!(
            probe_key(dynraw.clone(), [5; 32], "t1").await.unwrap(),
            KeyProbe::NeedsUpgrade {
                v1: 1,
                stale_keys: 0
            }
        );
        // A wrong key still reads as a mismatch, not as an upgrade.
        assert_eq!(
            probe_key(dynraw.clone(), [6; 32], "t1").await.unwrap(),
            KeyProbe::Mismatch { unopenable: 1 }
        );
        // A wrong key must not be able to "migrate" anything.
        let rep = migrate_sealed(dynraw.clone(), [6; 32], "t1").await.unwrap();
        assert_eq!(rep, MigrateReport::default());
        let rep = migrate_sealed(dynraw.clone(), [5; 32], "t1").await.unwrap();
        assert_eq!((rep.v1_upgraded, rep.renormalized, rep.skipped), (1, 0, 0));
        let f = m.fact("city").await.unwrap().unwrap();
        assert_eq!((f.value, f.updated_at), (json!("Pune"), 7));
        let stored = raw
            .scroll(Kind::Profile, &Filter::tenant("t1"), 5)
            .await
            .unwrap();
        assert_eq!(stored.len(), 1);
        assert!(stored[0].payload["_sealed"]
            .as_str()
            .unwrap()
            .starts_with("v2."));
        assert_eq!(
            probe_key(dynraw.clone(), [5; 32], "t1").await.unwrap(),
            KeyProbe::Ok
        );
        // Idempotent.
        assert_eq!(
            migrate_sealed(dynraw, [5; 32], "t1").await.unwrap(),
            MigrateReport::default()
        );
    }

    #[tokio::test]
    async fn keys_stored_under_an_older_norm_are_found_detected_and_reconciled() {
        let (raw, m) = fresh();
        let dynraw: Arc<dyn Store> = raw.clone();
        let sealed = SealedStore::new(dynraw.clone(), [5; 32]);
        // Older builds normalised these to nothing / dropped the combining mark, so the point sits under an
        // id and a `key_norm` today's lookups never ask for.
        for (key, old_norm, value) in [("😀", "", "smile"), ("cafe\u{301}", "cafe", "latte")] {
            let id = crate::point_id(&["t1", "profile", old_norm]);
            sealed
                .upsert(
                    Kind::Profile,
                    &id,
                    key,
                    fact_payload(key, old_norm, value, 3),
                )
                .await
                .unwrap();
            // Same id is shared by the two (both old norms differ, so they are different points).
        }
        assert!(m.fact("😀").await.unwrap().is_none());
        assert!(m.fact("cafe\u{301}").await.unwrap().is_none());
        assert_eq!(
            probe_key(dynraw.clone(), [5; 32], "t1").await.unwrap(),
            KeyProbe::NeedsUpgrade {
                v1: 0,
                stale_keys: 2
            }
        );
        let rep = migrate_sealed(dynraw.clone(), [5; 32], "t1").await.unwrap();
        assert_eq!((rep.v1_upgraded, rep.renormalized), (0, 2));
        assert_eq!(m.fact("😀").await.unwrap().unwrap().value, json!("smile"));
        assert_eq!(
            m.fact("cafe\u{301}").await.unwrap().unwrap().value,
            json!("latte")
        );
        // The orphans are gone: exactly two points remain.
        assert_eq!(
            raw.scroll(Kind::Profile, &Filter::tenant("t1"), 10)
                .await
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            probe_key(dynraw, [5; 32], "t1").await.unwrap(),
            KeyProbe::Ok
        );
    }

    #[tokio::test]
    async fn a_stale_key_never_overwrites_a_newer_fact_and_playbook_labels_are_reconciled() {
        let (raw, m) = fresh();
        let dynraw: Arc<dyn Store> = raw.clone();
        let sealed = SealedStore::new(dynraw.clone(), [5; 32]);
        let old = crate::point_id(&["t1", "profile", ""]);
        sealed
            .upsert(Kind::Profile, &old, "x", fact_payload("😀", "", "old", 1))
            .await
            .unwrap();
        m.put_fact("😀", json!("newer"), None, None, "user", Some(50))
            .await
            .unwrap();
        // Playbook whose `labels_norm` was derived differently.
        let pb = crate::Playbook {
            host: "shop.example".into(),
            fingerprint: "fp1".into(),
            headings: vec![],
            fields: vec![crate::PlaybookField::new("E-mail", "email", None)],
            uses: 1,
            last_success: 1,
            wins: 0,
            losses: 0,
            verified_ts: 1,
            learned_from: None,
        };
        let pid = crate::point_id(&["t1", "playbook", "fp1"]);
        sealed
            .upsert(
                Kind::Playbook,
                &pid,
                "shop",
                json!({"tenant": "t1", "host": "shop.example", "fingerprint": "fp1",
                       "labels_norm": ["stale"], "wins": 0, "losses": 0, "verified_ts": 1, "playbook": pb}),
            )
            .await
            .unwrap();
        assert_eq!(
            probe_key(dynraw.clone(), [5; 32], "t1").await.unwrap(),
            KeyProbe::NeedsUpgrade {
                v1: 0,
                stale_keys: 2
            }
        );
        let rep = migrate_sealed(dynraw.clone(), [5; 32], "t1").await.unwrap();
        assert_eq!(rep.renormalized, 2);
        assert_eq!(m.fact("😀").await.unwrap().unwrap().value, json!("newer"));
        let got = sealed
            .scroll(
                Kind::Playbook,
                &Filter::tenant("t1").and("labels_norm", json!("e mail")),
                5,
            )
            .await
            .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(
            probe_key(dynraw, [5; 32], "t1").await.unwrap(),
            KeyProbe::Ok
        );
    }
}
