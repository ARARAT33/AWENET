//! Local search index over stored resources.
//! A small inverted index with term-frequency scoring, used by the Browser
//! and Store screens to find content without contacting remote nodes.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SearchIndex {
    /// term -> document id -> term frequency
    postings: BTreeMap<String, BTreeMap<String, u32>>,
    /// document id -> distinct terms
    doc_terms: BTreeMap<String, BTreeSet<String>>,
}

fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

impl SearchIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn doc_count(&self) -> usize {
        self.doc_terms.len()
    }

    pub fn is_empty(&self) -> bool {
        self.doc_terms.is_empty()
    }

    pub fn len(&self) -> usize {
        self.doc_count()
    }

    /// Indexes or re-indexes a document identified by the given id.
    pub fn index(&mut self, doc_id: &str, text: &str) {
        self.remove(doc_id);
        let mut counts: BTreeMap<String, u32> = BTreeMap::new();
        for term in tokenize(text) {
            *counts.entry(term).or_insert(0) += 1;
        }
        let mut terms = BTreeSet::new();
        for (term, freq) in counts {
            terms.insert(term.clone());
            self.postings
                .entry(term)
                .or_default()
                .insert(doc_id.to_string(), freq);
        }
        self.doc_terms.insert(doc_id.to_string(), terms);
    }

    pub fn remove(&mut self, doc_id: &str) -> bool {
        let Some(terms) = self.doc_terms.remove(doc_id) else {
            return false;
        };
        for term in terms {
            if let Some(docs) = self.postings.get_mut(&term) {
                docs.remove(doc_id);
                if docs.is_empty() {
                    self.postings.remove(&term);
                }
            }
        }
        true
    }

    /// Returns up to the given number of results as (doc id, score) pairs,
    /// highest score first. Scoring is the summed term frequency of the
    /// query terms.
    pub fn query(&self, text: &str, limit: usize) -> Vec<(String, u32)> {
        let mut scores: BTreeMap<String, u32> = BTreeMap::new();
        for term in tokenize(text) {
            if let Some(docs) = self.postings.get(&term) {
                for (doc, freq) in docs {
                    *scores.entry(doc.clone()).or_insert(0) += freq;
                }
            }
        }
        let mut hits: Vec<(String, u32)> = scores.into_iter().collect();
        hits.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        hits.truncate(limit);
        hits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_query_and_remove() {
        let mut idx = SearchIndex::new();
        idx.index("doc1", "AWENET decentralized storage layer");
        idx.index("doc2", "ONECOIN payments and wallets");
        idx.index("doc3", "decentralized storage storage and repair");

        assert_eq!(idx.doc_count(), 3);
        assert!(!idx.is_empty());

        let hits = idx.query("decentralized storage", 10);
        assert_eq!(hits[0].0, "doc3");
        assert_eq!(hits.len(), 2);

        assert!(idx.remove("doc3"));
        assert!(!idx.remove("doc3"));
        assert_eq!(idx.doc_count(), 2);
        assert!(idx.query("repair", 10).is_empty());
    }

    #[test]
    fn case_insensitive_and_bounded() {
        let mut idx = SearchIndex::new();
        idx.index("a", "Hello WORLD of AWE");
        assert_eq!(idx.query("world", 5).len(), 1);
        assert_eq!(idx.query("awe OF", 5).len(), 1);
        idx.index("b", "world world world");
        let hits = idx.query("world", 1);
        assert_eq!(hits, vec![("b".to_string(), 3)]);
    }

    #[test]
    fn reindex_replaces_content() {
        let mut idx = SearchIndex::new();
        idx.index("doc", "alpha beta");
        idx.index("doc", "gamma delta");
        assert_eq!(idx.doc_count(), 1);
        assert!(idx.query("alpha", 10).is_empty());
        assert_eq!(idx.query("gamma", 10).len(), 1);
    }
}
