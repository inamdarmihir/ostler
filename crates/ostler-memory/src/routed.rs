//! A store that keeps each kind where it belongs.
//!
//! Source chunks are large, grow with every page read, are searched fuzzily over unstructured text and are
//! the one kind several agents on several machines may share: that is what a vector server is for. Saved
//! facts, form playbooks and error episodes are tens to hundreds of small records looked up by exact key,
//! for one person: a local file does that without a server, without a network round trip and (with
//! `--embeddings cloud`) without sending field labels to a cloud service to be embedded.
//!
//! `RoutedStore` sends [`Kind::Source`] to one store and every other kind to another. It adds no logic of
//! its own: each call goes to exactly one of the two.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::sparse::{SparseVector, TermHasher};
use crate::store::{Filter, Hit, HybridHit, HybridQuery, Kind, Rank, Store};
use crate::Result;

pub struct RoutedStore {
    /// Where [`Kind::Source`] lives.
    sources: Arc<dyn Store>,
    /// Where profile, playbook and episode records live.
    rest: Arc<dyn Store>,
}

impl RoutedStore {
    pub fn new(sources: Arc<dyn Store>, rest: Arc<dyn Store>) -> Self {
        Self { sources, rest }
    }

    fn pick(&self, kind: Kind) -> &Arc<dyn Store> {
        if kind == Kind::Source {
            &self.sources
        } else {
            &self.rest
        }
    }
}

#[async_trait]
impl Store for RoutedStore {
    fn term_hasher(&self) -> TermHasher {
        self.sources.term_hasher()
    }
    async fn upsert_source(
        &self,
        id: &str,
        text: &str,
        sparse: SparseVector,
        payload: Value,
    ) -> Result<()> {
        self.sources.upsert_source(id, text, sparse, payload).await
    }
    async fn search_hybrid(&self, q: &HybridQuery<'_>) -> Result<Vec<HybridHit>> {
        self.sources.search_hybrid(q).await
    }
    async fn replace_payload(&self, kind: Kind, id: &str, payload: Value) -> Result<()> {
        self.pick(kind).replace_payload(kind, id, payload).await
    }
    /// Either half being skipped is worth saying: both are named when both are.
    fn degraded(&self) -> Option<String> {
        match (self.sources.degraded(), self.rest.degraded()) {
            (None, None) => None,
            (Some(s), None) => Some(format!("source memory: {s}")),
            (None, Some(r)) => Some(r),
            (Some(s), Some(r)) => Some(format!("{r}; source memory: {s}")),
        }
    }
    async fn ensure(&self) -> Result<()> {
        self.rest.ensure().await?;
        self.sources.ensure().await
    }
    async fn upsert(&self, kind: Kind, id: &str, text: &str, payload: Value) -> Result<()> {
        self.pick(kind).upsert(kind, id, text, payload).await
    }
    async fn search(
        &self,
        kind: Kind,
        text: &str,
        filter: &Filter,
        limit: usize,
    ) -> Result<Vec<Hit>> {
        self.pick(kind).search(kind, text, filter, limit).await
    }
    async fn search_ranked(
        &self,
        kind: Kind,
        text: &str,
        filter: &Filter,
        limit: usize,
        rank: &Rank,
    ) -> Result<Vec<Hit>> {
        self.pick(kind)
            .search_ranked(kind, text, filter, limit, rank)
            .await
    }
    async fn scroll(&self, kind: Kind, filter: &Filter, limit: usize) -> Result<Vec<Hit>> {
        self.pick(kind).scroll(kind, filter, limit).await
    }
    async fn delete(&self, kind: Kind, id: &str) -> Result<()> {
        self.pick(kind).delete(kind, id).await
    }
    async fn delete_matching(&self, kind: Kind, filter: &Filter) -> Result<()> {
        self.pick(kind).delete_matching(kind, filter).await
    }
    async fn count(&self, kind: Kind, filter: &Filter) -> Result<u64> {
        self.pick(kind).count(kind, filter).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::HashEmbedder;
    use crate::store::InMemoryStore;
    use serde_json::json;

    fn mem() -> Arc<dyn Store> {
        Arc::new(InMemoryStore::new(Arc::new(HashEmbedder::default())))
    }

    #[tokio::test]
    async fn each_kind_lands_in_exactly_one_half() {
        let (sources, rest) = (mem(), mem());
        let routed = RoutedStore::new(sources.clone(), rest.clone());
        routed.ensure().await.unwrap();
        routed
            .upsert(
                Kind::Profile,
                "a",
                "email",
                json!({"tenant": "t", "key": "email"}),
            )
            .await
            .unwrap();
        routed
            .upsert(
                Kind::Source,
                "s",
                "some page text",
                json!({"tenant": "t", "url": "https://x"}),
            )
            .await
            .unwrap();
        let all = Filter::default();
        assert_eq!(rest.count(Kind::Profile, &all).await.unwrap(), 1);
        assert_eq!(sources.count(Kind::Profile, &all).await.unwrap(), 0);
        assert_eq!(sources.count(Kind::Source, &all).await.unwrap(), 1);
        assert_eq!(rest.count(Kind::Source, &all).await.unwrap(), 0);
        assert_eq!(routed.count(Kind::Profile, &all).await.unwrap(), 1);
        assert_eq!(routed.count(Kind::Source, &all).await.unwrap(), 1);
    }
}
