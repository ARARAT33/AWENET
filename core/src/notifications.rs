//! Bounded notification center for the desktop product.
//!
//! Feeds from alerts, transfers, storage and economy events. Hard capacity
//! bound so notifications never grow memory without limit.

use crate::alerts::AlertSeverity;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

pub const DEFAULT_NOTIFICATION_CAPACITY: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notification {
    pub id: u64,
    pub severity: AlertSeverity,
    pub category: String,
    pub title: String,
    pub body: String,
    pub read: bool,
}

/// Bounded FIFO notification store with stable monotonically increasing ids.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NotificationCenter {
    next_id: u64,
    capacity: usize,
    items: VecDeque<Notification>,
}

impl NotificationCenter {
    pub fn new(capacity: usize) -> Self {
        NotificationCenter {
            next_id: 1,
            capacity: capacity.max(1),
            items: VecDeque::new(),
        }
    }

    /// Pushes a notification and returns its id. The oldest notification is
    /// dropped when the capacity bound is reached.
    pub fn push(
        &mut self,
        severity: AlertSeverity,
        category: &str,
        title: &str,
        body: &str,
    ) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.items.push_back(Notification {
            id,
            severity,
            category: category.to_string(),
            title: title.to_string(),
            body: body.to_string(),
            read: false,
        });
        while self.items.len() > self.capacity {
            self.items.pop_front();
        }
        id
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn unread_count(&self) -> usize {
        self.items.iter().filter(|n| !n.read).count()
    }

    pub fn mark_read(&mut self, id: u64) -> bool {
        let mut found = false;
        for n in &mut self.items {
            if n.id == id {
                n.read = true;
                found = true;
            }
        }
        found
    }

    pub fn mark_all_read(&mut self) -> usize {
        let mut changed = 0;
        for n in &mut self.items {
            if !n.read {
                n.read = true;
                changed += 1;
            }
        }
        changed
    }

    pub fn by_category(&self, category: &str) -> Vec<Notification> {
        self.items
            .iter()
            .filter(|n| n.category == category)
            .cloned()
            .collect()
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }
}

impl Default for NotificationCenter {
    fn default() -> Self {
        Self::new(DEFAULT_NOTIFICATION_CAPACITY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pushes_and_bounds_capacity() {
        let mut center = NotificationCenter::new(3);
        assert_eq!(center.capacity(), 3);
        for i in 0..5 {
            center.push(AlertSeverity::Info, "alerts", "t", &format!("b{i}"));
        }
        assert_eq!(center.len(), 3);
        assert_eq!(center.unread_count(), 3);
        // Oldest notifications were dropped; ids stay monotonically ordered.
        let bodies: Vec<&str> = center.items.iter().map(|n| n.body.as_str()).collect();
        assert_eq!(bodies, vec!["b2", "b3", "b4"]);
    }

    #[test]
    fn marks_read() {
        let mut center = NotificationCenter::default();
        let first = center.push(AlertSeverity::Warning, "alerts", "cpu", "high");
        let second = center.push(AlertSeverity::Critical, "storage", "disk", "low");
        assert!(center.mark_read(first));
        assert!(!center.mark_read(999));
        assert_eq!(center.unread_count(), 1);
        assert_eq!(center.mark_all_read(), 1);
        assert_eq!(center.unread_count(), 0);
        assert_eq!(second, 2);
    }

    #[test]
    fn filters_by_category_and_clears() {
        let mut center = NotificationCenter::new(4);
        center.push(AlertSeverity::Info, "alerts", "a", "a");
        center.push(AlertSeverity::Info, "storage", "s", "s");
        assert_eq!(center.by_category("alerts").len(), 1);
        center.clear();
        assert!(center.is_empty());
        assert_eq!(center.unread_count(), 0);
    }

    #[test]
    fn serializes_to_json() {
        let mut center = NotificationCenter::default();
        center.push(AlertSeverity::Info, "alerts", "t", "b");
        let json = serde_json::to_string(&center).unwrap();
        let back: NotificationCenter = serde_json::from_str(&json).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back.unread_count(), 1);
    }
}
