//! Contributor rankings, levels and achievements for the AWENETwork
//! economy.
//!
//! The module derives every ranking, level and achievement strictly from
//! recorded earning events. It never mints ONECOIN, never touches the
//! ledger and cannot be fed by anything other than the economy's own
//! history, so a ranking can only reflect verified contribution. Earning
//! events are accepted only from outside: the caller (the economy
//! settlement paths) records what actually paid out.

use crate::identity::AweId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

pub const MAX_EVENTS: usize = 100_000;
pub const MAX_LEADERBOARD_SIZE: usize = 100;
pub const WEEK_SECS: u64 = 604_800;

pub const FIRST_EARNING_EVENTS: u64 = 1;
pub const FIVE_CONTRIBUTIONS_EVENTS: u64 = 5;
pub const STEADY_CONTRIBUTOR_WEEKS: u64 = 4;
pub const RESOURCE_VETERAN_EVENTS: u64 = 10;
pub const TRUSTED_SELLER_EVENTS: u64 = 10;
pub const PILLAR_OF_COMMUNITY_EVENTS: u64 = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum RankingsError {
    #[error("earning amount must be positive")]
    ZeroAmount,
    #[error("earning timestamp must be positive")]
    InvalidTime,
    #[error("earning history is full (max {MAX_EVENTS} events)")]
    HistoryFull,
    #[error("leaderboard window must be positive")]
    InvalidWindow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContributionCategory {
    Resource,
    Work,
    StoreRevenue,
}

impl ContributionCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Resource => "resource",
            Self::Work => "work",
            Self::StoreRevenue => "store_revenue",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EarningEvent {
    pub contributor: AweId,
    pub category: ContributionCategory,
    pub atoms: u128,
    pub at_unix: u64,
}

impl EarningEvent {
    pub fn new(
        contributor: AweId,
        category: ContributionCategory,
        atoms: u128,
        at_unix: u64,
    ) -> Result<Self, RankingsError> {
        if atoms == 0 {
            return Err(RankingsError::ZeroAmount);
        }
        if at_unix == 0 {
            return Err(RankingsError::InvalidTime);
        }
        Ok(Self {
            contributor,
            category,
            atoms,
            at_unix,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Achievement {
    FirstEarning,
    FiveContributions,
    SteadyContributor,
    ResourceVeteran,
    TrustedSeller,
    PillarOfCommunity,
}

impl Achievement {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FirstEarning => "first_earning",
            Self::FiveContributions => "five_contributions",
            Self::SteadyContributor => "steady_contributor",
            Self::ResourceVeteran => "resource_veteran",
            Self::TrustedSeller => "trusted_seller",
            Self::PillarOfCommunity => "pillar_of_community",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContributorLevel {
    Beginner,
    Contributor,
    AdvancedContributor,
    Expert,
    MasterContributor,
}

impl ContributorLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Beginner => "beginner",
            Self::Contributor => "contributor",
            Self::AdvancedContributor => "advanced_contributor",
            Self::Expert => "expert",
            Self::MasterContributor => "master_contributor",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContributorStats {
    pub resource_atoms: u128,
    pub work_atoms: u128,
    pub store_revenue_atoms: u128,
    pub event_count: u64,
    pub first_earning_unix: Option<u64>,
    pub last_earning_unix: Option<u64>,
    pub continuity_weeks: u64,
}

impl ContributorStats {
    pub fn total_atoms(&self) -> u128 {
        self.resource_atoms
            .saturating_add(self.work_atoms)
            .saturating_add(self.store_revenue_atoms)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rankings {
    events: Vec<EarningEvent>,
    per_contributor: BTreeMap<String, Vec<usize>>,
}

impl Rankings {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn events(&self) -> &[EarningEvent] {
        &self.events
    }

    pub fn event_count(&self) -> usize {
        self.events.len()
    }

    /// Records a paid-out earning. There is no silent drop: when the
    /// bounded history is full the caller is told and can decide how to
    /// archive.
    pub fn record(&mut self, event: EarningEvent) -> Result<(), RankingsError> {
        if event.atoms == 0 {
            return Err(RankingsError::ZeroAmount);
        }
        if event.at_unix == 0 {
            return Err(RankingsError::InvalidTime);
        }
        if self.events.len() >= MAX_EVENTS {
            return Err(RankingsError::HistoryFull);
        }
        let index = self.events.len();
        self.events.push(event.clone());
        self.per_contributor
            .entry(event.contributor.to_hex())
            .or_default()
            .push(index);
        Ok(())
    }

    /// Leaderboard for one category (or all categories when `category` is
    /// None) inside the window ending at `now_unix`. Ranked by earned
    /// atoms descending, ties broken by AweId so the order is stable.
    pub fn leaderboard(
        &self,
        category: Option<ContributionCategory>,
        window_secs: u64,
        now_unix: u64,
    ) -> Result<Vec<(AweId, u128)>, RankingsError> {
        self.leaderboard_for_period(category, now_unix.saturating_sub(window_secs), now_unix)
    }

    /// Leaderboard for an inclusive period [from_unix, to_unix]. `
    /// from_unix` may be 0, meaning "since the beginning".
    pub fn leaderboard_for_period(
        &self,
        category: Option<ContributionCategory>,
        from_unix: u64,
        to_unix: u64,
    ) -> Result<Vec<(AweId, u128)>, RankingsError> {
        if to_unix < from_unix {
            return Err(RankingsError::InvalidWindow);
        }
        let mut totals: BTreeMap<String, u128> = BTreeMap::new();
        let mut contributors: BTreeMap<String, AweId> = BTreeMap::new();
        for event in &self.events {
            if event.at_unix < from_unix || event.at_unix > to_unix {
                continue;
            }
            if let Some(wanted) = category {
                if event.category != wanted {
                    continue;
                }
            }
            let key = event.contributor.to_hex();
            *totals.entry(key.clone()).or_default() += event.atoms;
            contributors
                .entry(key)
                .or_insert_with(|| event.contributor.clone());
        }
        let mut ranked: Vec<(AweId, u128)> = totals
            .into_iter()
            .filter_map(|(key, atoms)| contributors.get(&key).cloned().map(|id| (id, atoms)))
            .collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_hex().cmp(&b.0.to_hex())));
        ranked.truncate(MAX_LEADERBOARD_SIZE);
        Ok(ranked)
    }

    /// Aggregated statistics for one contributor, derived from recorded
    /// events only.
    pub fn stats(&self, contributor: &AweId) -> ContributorStats {
        let mut stats = ContributorStats::default();
        let mut weeks: std::collections::BTreeSet<u64> = std::collections::BTreeSet::new();
        let key = contributor.to_hex();
        let indices = match self.per_contributor.get(&key) {
            Some(indices) => indices,
            None => return stats,
        };
        for &index in indices {
            let event = &self.events[index];
            match event.category {
                ContributionCategory::Resource => stats.resource_atoms += event.atoms,
                ContributionCategory::Work => stats.work_atoms += event.atoms,
                ContributionCategory::StoreRevenue => stats.store_revenue_atoms += event.atoms,
            }
            stats.event_count += 1;
            weeks.insert(event.at_unix / WEEK_SECS);
            stats.first_earning_unix = Some(match stats.first_earning_unix {
                Some(current) => current.min(event.at_unix),
                None => event.at_unix,
            });
            stats.last_earning_unix = Some(match stats.last_earning_unix {
                Some(current) => current.max(event.at_unix),
                None => event.at_unix,
            });
        }
        stats.continuity_weeks = weeks.len() as u64;
        stats
    }

    /// Achievements derived from verified events only. A store-revenue
    /// achievement counts only store-revenue events; the rest count any
    /// earning.
    pub fn achievements(&self, contributor: &AweId) -> Vec<Achievement> {
        let stats = self.stats(contributor);
        let mut achievements = Vec::new();
        if stats.event_count >= FIRST_EARNING_EVENTS {
            achievements.push(Achievement::FirstEarning);
        }
        if stats.event_count >= FIVE_CONTRIBUTIONS_EVENTS {
            achievements.push(Achievement::FiveContributions);
        }
        if stats.continuity_weeks >= STEADY_CONTRIBUTOR_WEEKS {
            achievements.push(Achievement::SteadyContributor);
        }
        if self.category_event_count(contributor, ContributionCategory::Resource)
            >= RESOURCE_VETERAN_EVENTS
        {
            achievements.push(Achievement::ResourceVeteran);
        }
        if self.category_event_count(contributor, ContributionCategory::StoreRevenue)
            >= TRUSTED_SELLER_EVENTS
        {
            achievements.push(Achievement::TrustedSeller);
        }
        if stats.event_count >= PILLAR_OF_COMMUNITY_EVENTS {
            achievements.push(Achievement::PillarOfCommunity);
        }
        achievements
    }

    /// Level derived from recorded event count. Levels reflect real work
    /// history and mint nothing.
    pub fn level(&self, contributor: &AweId) -> ContributorLevel {
        let events = self.stats(contributor).event_count;
        if events >= PILLAR_OF_COMMUNITY_EVENTS {
            ContributorLevel::MasterContributor
        } else if events >= TRUSTED_SELLER_EVENTS {
            ContributorLevel::Expert
        } else if events >= FIVE_CONTRIBUTIONS_EVENTS {
            ContributorLevel::AdvancedContributor
        } else if events >= FIRST_EARNING_EVENTS {
            ContributorLevel::Contributor
        } else {
            ContributorLevel::Beginner
        }
    }

    fn category_event_count(&self, contributor: &AweId, category: ContributionCategory) -> u64 {
        let key = contributor.to_hex();
        self.per_contributor
            .get(&key)
            .map(|indices| {
                indices
                    .iter()
                    .filter(|&&index| self.events[index].category == category)
                    .count() as u64
            })
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Username;

    fn contributor(name: &str) -> AweId {
        crate::identity::Identity::generate(Username::new(name).unwrap())
            .public
            .awe_id
    }

    #[test]
    fn events_are_validated_and_recorded() {
        let mut rankings = Rankings::new();
        let id = contributor("ranking-a");
        let event = EarningEvent::new(id.clone(), ContributionCategory::Work, 500, 100).unwrap();
        rankings.record(event).unwrap();
        assert_eq!(rankings.event_count(), 1);
        assert!(rankings
            .record(EarningEvent::new(
                id.clone(),
                ContributionCategory::Work,
                0,
                100
            ))
            .is_err());
        assert!(rankings
            .record(EarningEvent::new(id, ContributionCategory::Work, 500, 0))
            .is_err());
    }

    #[test]
    fn history_is_bounded_without_silent_drops() {
        let mut rankings = Rankings::new();
        let id = contributor("ranking-bounded");
        for i in 0..MAX_EVENTS {
            rankings
                .record(EarningEvent::new(
                    id.clone(),
                    ContributionCategory::Resource,
                    1,
                    i as u64 + 1,
                ))
                .unwrap();
        }
        let result = rankings.record(EarningEvent::new(
            id,
            ContributionCategory::Resource,
            1,
            MAX_EVENTS as u64 + 1,
        ));
        assert_eq!(result.unwrap_err(), RankingsError::HistoryFull);
        assert_eq!(rankings.event_count(), MAX_EVENTS);
    }

    #[test]
    fn leaderboard_filters_categories_windows_and_sorts() {
        let mut rankings = Rankings::new();
        let aram = contributor("aram");
        let karen = contributor("karen");
        let davit = contributor("davit");
        rankings
            .record(EarningEvent::new(
                aram.clone(),
                ContributionCategory::Resource,
                800,
                100,
            ))
            .unwrap();
        rankings
            .record(EarningEvent::new(
                karen.clone(),
                ContributionCategory::Resource,
                400,
                100,
            ))
            .unwrap();
        rankings
            .record(EarningEvent::new(
                davit.clone(),
                ContributionCategory::Work,
                900,
                100,
            ))
            .unwrap();
        rankings
            .record(EarningEvent::new(
                aram.clone(),
                ContributionCategory::Work,
                50,
                900,
            ))
            .unwrap();
        // all categories since the beginning: ranked by atoms descending
        let all = rankings.leaderboard(None, 1_000, 1_000).unwrap();
        assert_eq!(all[0], (davit.clone(), 900));
        assert_eq!(all[1], (aram.clone(), 850));
        assert_eq!(all[2], (karen.clone(), 400));
        // resource only
        let resource = rankings
            .leaderboard(Some(ContributionCategory::Resource), 1_000, 1_000)
            .unwrap();
        assert_eq!(resource[0], (aram.clone(), 800));
        assert_eq!(resource[1], (karen.clone(), 400));
        assert_eq!(resource.len(), 2);
        // a narrow window excludes the old resource earnings
        let fresh = rankings
            .leaderboard(Some(ContributionCategory::Resource), 100, 1_000)
            .unwrap();
        assert!(fresh.is_empty());
        let fresh_all = rankings.leaderboard(None, 100, 1_000).unwrap();
        assert_eq!(fresh_all[0], (aram.clone(), 50));
        assert!(rankings.leaderboard(None, 0, 0).is_err());
    }

    #[test]
    fn stats_and_continuity_weeks_are_derived_from_events() {
        let mut rankings = Rankings::new();
        let id = contributor("stats-id");
        let week = WEEK_SECS;
        rankings
            .record(EarningEvent::new(
                id.clone(),
                ContributionCategory::Resource,
                100,
                week + 10,
            ))
            .unwrap();
        rankings
            .record(EarningEvent::new(
                id.clone(),
                ContributionCategory::Work,
                200,
                week + 20,
            ))
            .unwrap();
        rankings
            .record(EarningEvent::new(
                id.clone(),
                ContributionCategory::StoreRevenue,
                300,
                week * 2 + 5,
            ))
            .unwrap();
        rankings
            .record(EarningEvent::new(
                id.clone(),
                ContributionCategory::Work,
                400,
                week * 9 + 5,
            ))
            .unwrap();
        let stats = rankings.stats(&id);
        assert_eq!(stats.resource_atoms, 100);
        assert_eq!(stats.work_atoms, 600);
        assert_eq!(stats.store_revenue_atoms, 300);
        assert_eq!(stats.total_atoms(), 1_000);
        assert_eq!(stats.event_count, 4);
        assert_eq!(stats.continuity_weeks, 3);
        assert_eq!(stats.first_earning_unix, Some(week + 10));
        assert_eq!(stats.last_earning_unix, Some(week * 9 + 5));
        let unknown = rankings.stats(&contributor("stats-unknown"));
        assert_eq!(unknown, ContributorStats::default());
    }

    #[test]
    fn achievements_and_levels_reflect_verified_work() {
        let mut rankings = Rankings::new();
        let steady = contributor("steady");
        for week in 0..STEADY_CONTRIBUTOR_WEEKS {
            rankings
                .record(EarningEvent::new(
                    steady.clone(),
                    ContributionCategory::Work,
                    10,
                    week * WEEK_SECS + 1,
                ))
                .unwrap();
        }
        let achievements = rankings.achievements(&steady);
        assert!(achievements.contains(&Achievement::FirstEarning));
        assert!(achievements.contains(&Achievement::FiveContributions));
        assert!(achievements.contains(&Achievement::SteadyContributor));
        assert!(!achievements.contains(&Achievement::ResourceVeteran));
        assert_eq!(
            rankings.level(&steady),
            ContributorLevel::AdvancedContributor
        );

        let seller = contributor("seller");
        for i in 0..TRUSTED_SELLER_EVENTS {
            rankings
                .record(EarningEvent::new(
                    seller.clone(),
                    ContributionCategory::StoreRevenue,
                    10,
                    i + 1,
                ))
                .unwrap();
        }
        assert!(rankings
            .achievements(&seller)
            .contains(&Achievement::TrustedSeller));
        assert_eq!(rankings.level(&seller), ContributorLevel::Expert);

        let pillar = contributor("pillar");
        for i in 0..PILLAR_OF_COMMUNITY_EVENTS {
            rankings
                .record(EarningEvent::new(
                    pillar.clone(),
                    ContributionCategory::Resource,
                    10,
                    i + 1,
                ))
                .unwrap();
        }
        let pillar_achievements = rankings.achievements(&pillar);
        assert!(pillar_achievements.contains(&Achievement::PillarOfCommunity));
        assert!(pillar_achievements.contains(&Achievement::ResourceVeteran));
        assert_eq!(rankings.level(&pillar), ContributorLevel::MasterContributor);

        let newcomer = contributor("newcomer");
        assert!(rankings.achievements(&newcomer).is_empty());
        assert_eq!(rankings.level(&newcomer), ContributorLevel::Beginner);
    }

    #[test]
    fn leaderboard_size_is_capped() {
        let mut rankings = Rankings::new();
        for i in 0..(MAX_LEADERBOARD_SIZE + 10) {
            let id = contributor(&format!("cap-{i}"));
            rankings
                .record(EarningEvent::new(id, ContributionCategory::Work, 10, 1))
                .unwrap();
        }
        let board = rankings.leaderboard(None, 1_000, 1_000).unwrap();
        assert_eq!(board.len(), MAX_LEADERBOARD_SIZE);
    }
}
