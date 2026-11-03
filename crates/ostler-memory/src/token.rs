//! Per-tenant Qdrant access tokens (JWT, HS256), so the *server* enforces tenant isolation.
//!
//! Qdrant (with `service.jwt_rbac: true`) accepts a JWT signed with the cluster API key whose `access`
//! claim limits which collections a bearer may touch. Qdrant 1.20 no longer supports restricting a token
//! to points matching a payload filter (the old `payload` claim is rejected as deprecated), so the
//! isolation unit is the **collection**: with `--isolation collection` every tenant gets its own three
//! collections ([`tenant_prefix`]), and a token minted here is valid for those and nothing else. A leaked
//! client token then exposes one tenant, not the cluster. Mint with the admin key and give the token to the
//! client as its API key. The admin key itself should never be given to a client.

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use base64::Engine;
use hmac::{Hmac, KeyInit, Mac};
use serde_json::json;
use sha2::Sha256;

use crate::store::Kind;
use crate::{MemoryError, Result};

/// The collection-name prefix for a tenant: `ostler_<slug>_`. Names that need cleaning (spaces, capitals,
/// punctuation) get a short hash suffix so two different tenants never collapse to the same prefix.
pub fn tenant_prefix(tenant: &str) -> String {
    let slug: String = tenant
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .take(40)
        .collect();
    let clean = slug == tenant
        && !slug.is_empty()
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    if clean {
        return format!("ostler_{slug}_");
    }
    let mut mac =
        <Hmac<Sha256> as KeyInit>::new_from_slice(b"ostler/tenant-prefix").expect("any length");
    mac.update(tenant.as_bytes());
    let h = B64URL
        .encode(&mac.finalize().into_bytes()[..6])
        .replace(['-', '_'], "x")
        .to_ascii_lowercase();
    format!("ostler_{slug}_{h}_")
}

/// A token for `tenant`'s collections, valid until `exp` (unix seconds). `read_only` limits it to search and scroll.
pub fn mint_tenant_token(
    admin_key: &str,
    tenant: &str,
    exp: u64,
    read_only: bool,
) -> Result<String> {
    if tenant.trim().is_empty() {
        return Err(MemoryError::Invalid("tenant must not be empty".into()));
    }
    if admin_key.is_empty() {
        return Err(MemoryError::Invalid("the admin API key is empty".into()));
    }
    let access: Vec<_> = Kind::ALL
        .iter()
        .map(|k| {
            json!({
                "collection": format!("{}{}", tenant_prefix(tenant), k.collection()),
                "access": if read_only { "r" } else { "rw" },
            })
        })
        .collect();
    let header = B64URL.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
    let claims = B64URL.encode(
        serde_json::to_vec(&json!({ "exp": exp, "access": access }))
            .map_err(|e| MemoryError::Invalid(e.to_string()))?,
    );
    let signing_input = format!("{header}.{claims}");
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(admin_key.as_bytes())
        .map_err(|e| MemoryError::Invalid(e.to_string()))?;
    mac.update(signing_input.as_bytes());
    Ok(format!(
        "{signing_input}.{}",
        B64URL.encode(mac.finalize().into_bytes())
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_has_three_parts_and_scopes_collections_to_the_tenant() {
        let t = mint_tenant_token("k", "alice", 4_000_000_000, false).unwrap();
        let parts: Vec<&str> = t.split('.').collect();
        assert_eq!(parts.len(), 3);
        let claims: serde_json::Value =
            serde_json::from_slice(&B64URL.decode(parts[1]).unwrap()).unwrap();
        let access = claims["access"].as_array().unwrap();
        assert_eq!(access.len(), 3);
        for a in access {
            assert!(a["collection"]
                .as_str()
                .unwrap()
                .starts_with("ostler_alice_"));
            assert_eq!(a["access"], "rw");
            assert!(
                a.get("payload").is_none(),
                "the payload claim is deprecated"
            );
        }
        assert_eq!(tenant_prefix("alice"), "ostler_alice_");
        assert_ne!(tenant_prefix("Alice"), tenant_prefix("alice"));
        assert_ne!(tenant_prefix("a b"), tenant_prefix("a_b"));
        assert!(tenant_prefix("Ünï cödé")
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'));
        assert!(mint_tenant_token("k", " ", 1, false).is_err());
        // Deterministic signature for a fixed input (HS256 over header.claims).
        assert_eq!(
            t,
            mint_tenant_token("k", "alice", 4_000_000_000, false).unwrap()
        );
    }
}
