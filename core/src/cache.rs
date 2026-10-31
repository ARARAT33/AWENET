//! Bounded LRU cache with deterministic eviction.
//!
//! Deterministic and replay-friendly: pure data, no wall clock and no
//! threads. Lookups promote entries to most-recently-used; inserts evict the
//! least-recently-used entry once the capacity is reached.

use std::collections::{BTreeMap, VecDeque};
use thiserror::Error;

pub const MAX_CAPACITY: usize = 65_536;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum CacheError {
    #[error("cache capacity must be greater than zero")]
    ZeroCapacity,
    #[error("cache capacity exceeds the maximum of {MAX_CAPACITY}")]
    CapacityTooLarge,
}

pub struct LruCache<K: Ord + Clone, V: Clone> {
    capacity: usize,
    map: BTreeMap<K, V>,
    order: VecDeque<K>,
}

impl<K: Ord + Clone, V: Clone> LruCache<K, V> {
    pub fn new(capacity: usize) -> Result<Self, CacheError> {
        if capacity == 0 {
            return Err(CacheError::ZeroCapacity);
        }
        if capacity > MAX_CAPACITY {
            return Err(CacheError::CapacityTooLarge);
        }
        Ok(Self {
            capacity,
            map: BTreeMap::new(),
            order: VecDeque::new(),
        })
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Presence check that does not change recency.
    pub fn contains_key(&self, key: &K) -> bool {
        self.map.contains_key(key)
    }

    /// Looks up a key and promotes it to most-recently-used.
    pub fn get(&mut self, key: &K) -> Option<V> {
        if !self.map.contains_key(key) {
            return None;
        }
        self.touch(key);
        self.map.get(key).cloned()
    }

    /// Inserts or updates a key. Updating an existing key keeps the cache
    /// size and promotes the key; inserting a new key evicts the
    /// least-recently-used entry when the cache is full.
    pub fn insert(&mut self, key: K, value: V) {
        if self.map.contains_key(&key) {
            self.map.insert(key.clone(), value);
            self.touch(&key);
            return;
        }
        if self.map.len() >= self.capacity {
            if let Some(evicted) = self.order.pop_front() {
                self.map.remove(&evicted);
            }
        }
        self.order.push_back(key.clone());
        self.map.insert(key, value);
    }

    pub fn remove(&mut self, key: &K) -> Option<V> {
        self.order.retain(|k| k != key);
        self.map.remove(key)
    }

    pub fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
    }

    fn touch(&mut self, key: &K) {
        self.order.retain(|k| k != key);
        self.order.push_back(key.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_capacity() {
        assert_eq!(LruCache::<String, String>::new(0).unwrap_err(), CacheError::ZeroCapacity);
        assert_eq!(
            LruCache::<String, String>::new(MAX_CAPACITY + 1).unwrap_err(),
            CacheError::CapacityTooLarge
        );
        assert!(LruCache::<String, String>::new(1).is_ok());
    }

    #[test]
    fn evicts_least_recently_used() {
        let mut cache: LruCache<String, u32> = LruCache::new(2).unwrap();
        cache.insert("a".into(), 1);
        cache.insert("b".into(), 2);
        cache.insert("c".into(), 3);
        assert_eq!(cache.get(&"a".to_string()), None);
        assert_eq!(cache.get(&"b".to_string()), Some(2));
        assert_eq!(cache.get(&"c".to_string()), Some(3));
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn get_promotes_entries() {
        let mut cache: LruCache<String, u32> = LruCache::new(2).unwrap();
        cache.insert("a".into(), 1);
        cache.insert("b".into(), 2);
        assert_eq!(cache.get(&"a".to_string()), Some(1));
        cache.insert("c".into(), 3);
        assert_eq!(cache.get(&"b".to_string()), None);
        assert_eq!(cache.get(&"a".to_string()), Some(1));
    }

    #[test]
    fn update_keeps_size() {
        let mut cache: LruCache<String, u32> = LruCache::new(2).unwrap();
        cache.insert("a".into(), 1);
        cache.insert("a".into(), 10);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.get(&"a".to_string()), Some(10));
    }

    #[test]
    fn contains_and_remove() {
        let mut cache: LruCache<String, u32> = LruCache::new(4).unwrap();
        cache.insert("a".into(), 1);
        assert!(cache.contains_key(&"a".to_string()));
        assert_eq!(cache.remove(&"a".to_string()), Some(1));
        assert!(cache.is_empty());
        assert!(!cache.contains_key(&"a".to_string()));
        cache.insert("b".into(), 2);
        cache.clear();
        assert!(cache.is_empty());
    }
}
