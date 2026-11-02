//! Retention tiering and compaction planning for bounded local state.
//!
//! Deterministic and replay-friendly: pure data, no wall clock and no
//! threads. Records are classified by age into hot, warm and cold tiers;
//! cold records are archived largest-first within a bounded archive
//! budget, and whatever does not fit is planned for expiry.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Hot,
    Warm,
    Cold,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub id: String,
    pub age_ticks: u64,
    pub size_bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetentionPolicy {
    pub hot_ticks: u64,
    pub warm_ticks: u64,
    pub archive_budget_bytes: u64,
}

impl RetentionPolicy {
    pub fn new(hot_ticks: u64, warm_ticks: u64, archive_budget_bytes: u64) -> Self {
        Self {
            hot_ticks,
            warm_ticks,
            archive_budget_bytes,
        }
    }

    /// Classifies a record: hot first, then warm, then cold (archive
    /// candidates).
    pub fn tier(&self, record: &Record) -> Tier {
        if record.age_ticks < self.hot_ticks {
            Tier::Hot
        } else if record.age_ticks < self.warm_ticks {
            Tier::Warm
        } else {
            Tier::Cold
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CompactionPlan {
    pub hot: Vec<String>,
    pub warm: Vec<String>,
    pub archive: Vec<String>,
    pub expire: Vec<String>,
    pub archived_bytes: u64,
    pub reclaimed_bytes: u64,
}

/// Plans retention for `records` under `policy`. Cold records are
/// archived largest-first within the archive budget; the rest are planned
/// for expiry. All output lists are sorted for determinism.
pub fn plan(records: &[Record], policy: &RetentionPolicy) -> CompactionPlan {
    let mut cold: Vec<&Record> = records
        .iter()
        .filter(|r| policy.tier(r) == Tier::Cold)
        .collect();
    cold.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes).then(a.id.cmp(&b.id)));

    let mut result = CompactionPlan::default();
    let mut budget = policy.archive_budget_bytes;
    let mut archive_ids = Vec::new();
    let mut expire_ids = Vec::new();
    for record in cold {
        if record.size_bytes <= budget {
            budget -= record.size_bytes;
            result.archived_bytes += record.size_bytes;
            archive_ids.push(record.id.clone());
        } else {
            result.reclaimed_bytes += record.size_bytes;
            expire_ids.push(record.id.clone());
        }
    }
    for record in records {
        match policy.tier(record) {
            Tier::Hot => result.hot.push(record.id.clone()),
            Tier::Warm => result.warm.push(record.id.clone()),
            Tier::Cold => {}
        }
    }
    archive_ids.sort();
    expire_ids.sort();
    result.hot.sort();
    result.warm.sort();
    result.archive = archive_ids;
    result.expire = expire_ids;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, age_ticks: u64, size_bytes: u64) -> Record {
        Record {
            id: id.to_string(),
            age_ticks,
            size_bytes,
        }
    }

    #[test]
    fn tiers_by_age() {
        let policy = RetentionPolicy::new(10, 100, 1_000);
        assert_eq!(policy.tier(&record("a", 9, 1)), Tier::Hot);
        assert_eq!(policy.tier(&record("b", 10, 1)), Tier::Warm);
        assert_eq!(policy.tier(&record("c", 99, 1)), Tier::Warm);
        assert_eq!(policy.tier(&record("d", 100, 1)), Tier::Cold);
    }

    #[test]
    fn budget_archives_largest_first() {
        let policy = RetentionPolicy::new(0, 0, 150);
        let records = vec![
            record("small", 100, 40),
            record("large", 100, 100),
            record("medium", 100, 60),
        ];
        let plan = plan(&records, &policy);
        // largest-first: 100 + 40 fits in 150; medium (60) is expired.
        assert_eq!(plan.archive, vec!["large", "small"]);
        assert_eq!(plan.expire, vec!["medium"]);
        assert_eq!(plan.archived_bytes, 140);
        assert_eq!(plan.reclaimed_bytes, 60);
    }

    #[test]
    fn hot_and_warm_are_kept() {
        let policy = RetentionPolicy::new(10, 100, 0);
        let records = vec![
            record("h", 5, 10),
            record("w", 50, 10),
            record("c", 500, 10),
        ];
        let plan = plan(&records, &policy);
        assert_eq!(plan.hot, vec!["h"]);
        assert_eq!(plan.warm, vec!["w"]);
        assert_eq!(plan.archive, Vec::<String>::new());
        assert_eq!(plan.expire, vec!["c"]);
    }

    #[test]
    fn deterministic_order_and_empty_input() {
        let policy = RetentionPolicy::new(0, 0, 1_000);
        let records = vec![
            record("z", 10, 5),
            record("a", 10, 5),
            record("m", 10, 5),
        ];
        let plan = plan(&records, &policy);
        assert_eq!(plan.archive, vec!["a", "m", "z"]);
        assert_eq!(plan.archived_bytes, 15);
        let empty = plan(&[], &policy);
        assert_eq!(empty, CompactionPlan::default());
    }
}
