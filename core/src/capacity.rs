//! Capacity forecasting and headroom recommendations.
//!
//! Deterministic and replay-friendly: pure data, no wall clock and no
//! threads. Given current usage, a limit and a growth-per-tick estimate,
//! the forecast reports the remaining headroom, the number of ticks until
//! the limit is reached and how urgent a capacity decision is.

use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum CapacityError {
    #[error("capacity limit must be greater than zero")]
    ZeroLimit,
    #[error("growth per tick exceeds the limit")]
    GrowthExceedsLimit,
    #[error("critical headroom percent must stay below the warn percent")]
    InvalidThresholds,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Urgency {
    Ok,
    Warn,
    Critical,
    Full,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CapacityConfig {
    pub limit: u64,
    pub growth_per_tick: u64,
    pub warn_headroom_percent: u32,
    pub critical_headroom_percent: u32,
}

impl CapacityConfig {
    pub fn new(limit: u64) -> Result<Self, CapacityError> {
        if limit == 0 {
            return Err(CapacityError::ZeroLimit);
        }
        Ok(Self {
            limit,
            growth_per_tick: 0,
            warn_headroom_percent: 20,
            critical_headroom_percent: 10,
        })
    }

    pub fn with_growth(mut self, growth_per_tick: u64) -> Result<Self, CapacityError> {
        if growth_per_tick > self.limit {
            return Err(CapacityError::GrowthExceedsLimit);
        }
        self.growth_per_tick = growth_per_tick;
        Ok(self)
    }

    pub fn with_thresholds(
        mut self,
        warn_headroom_percent: u32,
        critical_headroom_percent: u32,
    ) -> Result<Self, CapacityError> {
        if critical_headroom_percent >= warn_headroom_percent {
            return Err(CapacityError::InvalidThresholds);
        }
        self.warn_headroom_percent = warn_headroom_percent;
        self.critical_headroom_percent = critical_headroom_percent;
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CapacityForecast {
    pub limit: u64,
    pub used: u64,
    pub headroom: u64,
    pub headroom_percent: u32,
    pub ticks_to_limit: Option<u64>,
    pub urgency: Urgency,
}

impl CapacityForecast {
    pub fn recommendation(&self) -> &'static str {
        match self.urgency {
            Urgency::Ok => "headroom is healthy; no action needed",
            Urgency::Warn => "plan capacity within the forecast window",
            Urgency::Critical => "add capacity or reduce usage before the limit is reached",
            Urgency::Full => "limit reached; shed load or raise the limit",
        }
    }
}

/// Forecasts headroom and time-to-limit for `used` bytes under `config`.
pub fn forecast(used: u64, config: &CapacityConfig) -> CapacityForecast {
    let headroom = config.limit.saturating_sub(used);
    let headroom_percent = (headroom as u128 * 100 / config.limit as u128) as u32;
    let full = used >= config.limit;
    let ticks_to_limit = if full {
        Some(0)
    } else if config.growth_per_tick == 0 {
        None
    } else {
        Some(headroom.div_ceil(config.growth_per_tick))
    };
    let urgency = if full {
        Urgency::Full
    } else if headroom_percent <= config.critical_headroom_percent {
        Urgency::Critical
    } else if headroom_percent <= config.warn_headroom_percent {
        Urgency::Warn
    } else {
        Urgency::Ok
    };
    CapacityForecast {
        limit: config.limit,
        used,
        headroom,
        headroom_percent,
        ticks_to_limit,
        urgency,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_config() {
        assert_eq!(
            CapacityConfig::new(0).unwrap_err(),
            CapacityError::ZeroLimit
        );
        let config = CapacityConfig::new(100).unwrap();
        assert_eq!(
            config.with_growth(101).unwrap_err(),
            CapacityError::GrowthExceedsLimit
        );
        assert_eq!(
            CapacityConfig::new(100)
                .unwrap()
                .with_thresholds(10, 10)
                .unwrap_err(),
            CapacityError::InvalidThresholds
        );
        assert!(CapacityConfig::new(100).unwrap().with_growth(10).is_ok());
    }

    #[test]
    fn healthy_headroom() {
        let config = CapacityConfig::new(1_000).unwrap();
        let f = forecast(250, &config);
        assert_eq!(f.headroom, 750);
        assert_eq!(f.headroom_percent, 75);
        assert_eq!(f.ticks_to_limit, None);
        assert_eq!(f.urgency, Urgency::Ok);
        assert_eq!(f.recommendation(), "headroom is healthy; no action needed");
    }

    #[test]
    fn ticks_to_limit_is_ceiled() {
        let config = CapacityConfig::new(1_000).unwrap().with_growth(3).unwrap();
        let f = forecast(998, &config);
        assert_eq!(f.headroom, 2);
        assert_eq!(f.ticks_to_limit, Some(1));
        let f = forecast(995, &config);
        assert_eq!(f.headroom, 5);
        assert_eq!(f.ticks_to_limit, Some(2));
    }

    #[test]
    fn urgency_bands() {
        let config = CapacityConfig::new(100).unwrap();
        assert_eq!(forecast(80, &config).urgency, Urgency::Warn);
        assert_eq!(forecast(90, &config).urgency, Urgency::Critical);
        assert_eq!(forecast(100, &config).urgency, Urgency::Full);
        assert_eq!(forecast(150, &config).headroom, 0);
        assert_eq!(forecast(150, &config).urgency, Urgency::Full);
        assert_eq!(forecast(100, &config).ticks_to_limit, Some(0));
    }

    #[test]
    fn custom_thresholds() {
        let config = CapacityConfig::new(100)
            .unwrap()
            .with_thresholds(50, 40)
            .unwrap();
        assert_eq!(forecast(40, &config).urgency, Urgency::Ok);
        assert_eq!(forecast(50, &config).urgency, Urgency::Warn);
        assert_eq!(forecast(65, &config).urgency, Urgency::Critical);
    }
}
