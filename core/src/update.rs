//! Node software version contract and upgrade policy.
//!
//! Keeps upgrade decisions deterministic: a release either leaves the node
//! up to date, is recommended, is required, or is incompatible with the
//! protocol major version of the running node.

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Version {
            major,
            minor,
            patch,
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl FromStr for Version {
    type Err = VersionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() != 3 {
            return Err(VersionError::InvalidFormat);
        }
        let parse = |part: &str| -> Result<u32, VersionError> {
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return Err(VersionError::InvalidFormat);
            }
            part.parse::<u32>().map_err(|_| VersionError::InvalidFormat)
        };
        Ok(Version {
            major: parse(parts[0])?,
            minor: parse(parts[1])?,
            patch: parse(parts[2])?,
        })
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum VersionError {
    #[error("expected MAJOR.MINOR.PATCH")]
    InvalidFormat,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpgradeDecision {
    UpToDate,
    Recommended(Version),
    Required(Version),
    Incompatible(Version),
}

/// Decides what to do with an available release, given the protocol major
/// versions of the running node and the release.
pub fn decide(
    current: Version,
    available: Version,
    current_protocol_major: u16,
    available_protocol_major: u16,
) -> UpgradeDecision {
    if available_protocol_major != current_protocol_major {
        return UpgradeDecision::Incompatible(available);
    }
    match available.cmp(&current) {
        Ordering::Less | Ordering::Equal => UpgradeDecision::UpToDate,
        Ordering::Greater if available.major > current.major => {
            UpgradeDecision::Required(available)
        }
        Ordering::Greater => UpgradeDecision::Recommended(available),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_displays_versions() {
        let v: Version = "1.2.3".parse().unwrap();
        assert_eq!(v, Version::new(1, 2, 3));
        assert_eq!(v.to_string(), "1.2.3");
        assert_eq!("".parse::<Version>(), Err(VersionError::InvalidFormat));
        assert_eq!("1.2".parse::<Version>(), Err(VersionError::InvalidFormat));
        assert_eq!("a.2.3".parse::<Version>(), Err(VersionError::InvalidFormat));
        assert_eq!("1..3".parse::<Version>(), Err(VersionError::InvalidFormat));
        let long: Result<Version, _> = "1.2.3.4".parse();
        assert_eq!(long, Err(VersionError::InvalidFormat));
    }

    #[test]
    fn orders_versions() {
        assert!(Version::new(1, 2, 3) < Version::new(1, 3, 0));
        assert!(Version::new(1, 2, 3) < Version::new(2, 0, 0));
        assert!(Version::new(1, 2, 3) < Version::new(1, 2, 4));
        assert_eq!(Version::new(1, 0, 0), Version::new(1, 0, 0));
    }

    #[test]
    fn decides_upgrades() {
        let current = Version::new(1, 2, 0);
        assert_eq!(
            decide(current, Version::new(1, 1, 9), 1, 1),
            UpgradeDecision::UpToDate
        );
        assert_eq!(
            decide(current, Version::new(1, 2, 0), 1, 1),
            UpgradeDecision::UpToDate
        );
        assert_eq!(
            decide(current, Version::new(1, 3, 0), 1, 1),
            UpgradeDecision::Recommended(Version::new(1, 3, 0))
        );
        assert_eq!(
            decide(current, Version::new(2, 0, 0), 1, 1),
            UpgradeDecision::Required(Version::new(2, 0, 0))
        );
        assert_eq!(
            decide(current, Version::new(1, 4, 0), 1, 2),
            UpgradeDecision::Incompatible(Version::new(1, 4, 0))
        );
    }
}
