//! Runtime resource measurement and independent usage verification.
//!
//! The resource economy pays for measured, multi-signal usage. This module
//! wires two pieces of that contract: a meter that reads real host
//! counters (CPU time, memory pressure, network counters, uptime and the
//! free capacity of the contributed storage directory from /proc and
//! statvfs on Linux) and converts them into per-period MeasuredUsage, and
//! signed usage reports from independent verifiers whose conservative
//! consensus becomes the network-side measurement of the settlement
//! evidence. GPU usage cannot be read from generic OS counters, so that
//! signal still comes from the work runner's delivered results; nothing
//! here invents numbers the host did not report.

use crate::identity::{AweId, Identity};
use crate::resource_economy::{MeasuredUsage, UsageEvidence};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::io;

pub const MIN_INDEPENDENT_VERIFIERS: usize = 2;
pub const MAX_VERIFIER_REPORTS: usize = 16;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SystemCounters {
    pub cpu_core_ticks: u128,
    pub ram_total_kb: u64,
    pub ram_available_kb: u64,
    pub net_rx_bytes: u64,
    pub net_tx_bytes: u64,
    pub uptime_secs: u64,
    pub disk_total_kb: u64,
    pub disk_available_kb: u64,
}

pub trait SystemMeter {
    fn read(&mut self) -> io::Result<SystemCounters>;
}

/// Reads the real counters of the host. On Linux the source is /proc and
/// the filesystem statistics of the contributed storage directory; on
/// every other platform reading reports that host metering is not wired
/// yet, so the caller falls back to its own measurements instead of
/// being paid for numbers the meter never observed.
#[derive(Clone, Debug)]
pub struct HostMeter {
    storage_dir: std::path::PathBuf,
}

impl HostMeter {
    pub fn new() -> Self {
        Self {
            storage_dir: std::path::PathBuf::from("."),
        }
    }

    /// Meters the free capacity of this directory as the contributed
    /// storage space.
    pub fn with_storage_dir(storage_dir: impl Into<std::path::PathBuf>) -> Self {
        Self {
            storage_dir: storage_dir.into(),
        }
    }
}

impl Default for HostMeter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "linux")]
impl SystemMeter for HostMeter {
    fn read(&mut self) -> io::Result<SystemCounters> {
        let cpu_core_ticks = parse_proc_stat_cpu_ticks(&std::fs::read_to_string("/proc/stat")?)
            .ok_or_else(|| invalid_data("cannot parse /proc/stat cpu line"))?;
        let (ram_total_kb, ram_available_kb) =
            parse_proc_meminfo(&std::fs::read_to_string("/proc/meminfo")?)
                .ok_or_else(|| invalid_data("cannot parse /proc/meminfo"))?;
        let (net_rx_bytes, net_tx_bytes) =
            parse_proc_net_dev(&std::fs::read_to_string("/proc/net/dev")?)
                .ok_or_else(|| invalid_data("cannot parse /proc/net/dev"))?;
        let uptime_secs = parse_proc_uptime_secs(&std::fs::read_to_string("/proc/uptime")?)
            .ok_or_else(|| invalid_data("cannot parse /proc/uptime"))?;
        let (disk_total_kb, disk_available_kb) = statvfs_kb(&self.storage_dir)
            .ok_or_else(|| invalid_data("cannot read filesystem capacity"))?;
        Ok(SystemCounters {
            cpu_core_ticks,
            ram_total_kb,
            ram_available_kb,
            net_rx_bytes,
            net_tx_bytes,
            uptime_secs,
            disk_total_kb,
            disk_available_kb,
        })
    }
}

#[cfg(not(target_os = "linux"))]
impl SystemMeter for HostMeter {
    fn read(&mut self) -> io::Result<SystemCounters> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "host resource metering is wired for Linux in this layer",
        ))
    }
}

#[cfg(target_os = "linux")]
fn invalid_data(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

#[cfg(target_os = "linux")]
pub(crate) fn parse_proc_stat_cpu_ticks(stat: &str) -> Option<u128> {
    let line = stat.lines().find(|line| line.starts_with("cpu "))?;
    let mut total = 0u128;
    for field in line.split_whitespace().skip(1) {
        total = total.checked_add(field.parse::<u128>().ok()?)?;
    }
    Some(total)
}

#[cfg(target_os = "linux")]
pub(crate) fn parse_proc_meminfo(meminfo: &str) -> Option<(u64, u64)> {
    let mut ram_total_kb = None;
    let mut ram_available_kb = None;
    for line in meminfo.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            ram_total_kb = Some(rest.trim_end_matches(" kB").trim().parse::<u64>().ok()?);
        } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
            ram_available_kb = Some(rest.trim_end_matches(" kB").trim().parse::<u64>().ok()?);
        }
    }
    Some((ram_total_kb?, ram_available_kb?))
}

#[cfg(target_os = "linux")]
pub(crate) fn parse_proc_net_dev(netdev: &str) -> Option<(u64, u64)> {
    let mut rx_bytes = 0u64;
    let mut tx_bytes = 0u64;
    for line in netdev.lines().skip(2) {
        let (iface, rest) = line.split_once(':')?;
        if iface.trim() == "lo" {
            continue;
        }
        let fields: Vec<&str> = rest.split_whitespace().collect();
        if fields.len() < 9 {
            continue;
        }
        rx_bytes = rx_bytes.saturating_add(fields[0].parse::<u64>().ok()?);
        tx_bytes = tx_bytes.saturating_add(fields[8].parse::<u64>().ok()?);
    }
    Some((rx_bytes, tx_bytes))
}

#[cfg(target_os = "linux")]
pub(crate) fn parse_proc_uptime_secs(uptime: &str) -> Option<u64> {
    let first = uptime.lines().next()?.split_whitespace().next()?;
    let whole = first.split('.').next()?;
    whole.parse::<u64>().ok()
}

/// Reads the total and available capacity of the contributed storage
/// directory from the filesystem statistics, in kilobytes. This is a real
/// measurement of the space the node could contribute; the resource
/// economy's grant limits cap what is paid for.
#[cfg(target_os = "linux")]
fn statvfs_kb(path: &std::path::Path) -> Option<(u64, u64)> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: statvfs writes into a zeroed, correctly sized C struct and
    // only reads the borrowed C path string.
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut stats) } != 0 {
        return None;
    }
    let block_size = stats.f_bsize;
    let total_kb = u128::from(block_size) * u128::from(stats.f_blocks) / 1_024;
    let available_kb = u128::from(block_size) * u128::from(stats.f_bavail) / 1_024;
    Some((
        u64::try_from(total_kb).unwrap_or(u64::MAX),
        u64::try_from(available_kb).unwrap_or(u64::MAX),
    ))
}

/// A meter that replays programmed counter snapshots in order, for
/// deterministic tests and offline replays. Reading past the last sample
/// is an error, never an invented value.
#[derive(Clone, Debug, Default)]
pub struct SequenceMeter {
    samples: Vec<SystemCounters>,
    next: usize,
}

impl SequenceMeter {
    pub fn new(samples: Vec<SystemCounters>) -> Self {
        Self { samples, next: 0 }
    }
}

impl SystemMeter for SequenceMeter {
    fn read(&mut self) -> io::Result<SystemCounters> {
        match self.samples.get(self.next) {
            Some(counters) => {
                self.next = self.next.saturating_add(1);
                Ok(*counters)
            }
            None => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "meter has no more samples",
            )),
        }
    }
}

/// Turns consecutive meter reads into a per-period MeasuredUsage for the
/// resource economy: CPU core ticks, average RAM use integrated over the
/// elapsed time, transferred bytes and uptime hours.
#[derive(Clone, Debug)]
pub struct UsagePeriodMeter<M: SystemMeter> {
    meter: M,
    last: Option<SystemCounters>,
}

impl<M: SystemMeter> UsagePeriodMeter<M> {
    pub fn new(meter: M) -> Self {
        Self { meter, last: None }
    }

    pub fn last_counters(&self) -> Option<SystemCounters> {
        self.last
    }

    pub fn into_meter(self) -> M {
        self.meter
    }

    /// Measures the delta since the previous read. The first read only
    /// establishes the baseline and returns None.
    pub fn sample_period(&mut self) -> io::Result<Option<MeasuredUsage>> {
        let current = self.meter.read()?;
        let Some(previous) = self.last.replace(current) else {
            return Ok(None);
        };
        let delta_ticks = current
            .cpu_core_ticks
            .saturating_sub(previous.cpu_core_ticks);
        let delta_secs = current.uptime_secs.saturating_sub(previous.uptime_secs);
        let used_kb_before = previous
            .ram_total_kb
            .saturating_sub(previous.ram_available_kb);
        let used_kb_now = current
            .ram_total_kb
            .saturating_sub(current.ram_available_kb);
        let average_used_kb = used_kb_before.saturating_add(used_kb_now) / 2;
        let average_used_mb = (average_used_kb as u128) / 1_024;
        let ram_mb_hours = average_used_mb.saturating_mul(delta_secs as u128) / 3_600;
        let transferred_bytes = current
            .net_rx_bytes
            .saturating_sub(previous.net_rx_bytes)
            .saturating_add(current.net_tx_bytes.saturating_sub(previous.net_tx_bytes));
        let bandwidth_gb = (transferred_bytes as u128) / 1_000_000_000;
        let uptime_hours = (delta_secs as u128) / 3_600;
        let storage_available_gb_before = (previous.disk_available_kb as u128) / 1_000_000;
        let storage_available_gb_now = (current.disk_available_kb as u128) / 1_000_000;
        let average_storage_gb =
            storage_available_gb_before.saturating_add(storage_available_gb_now) / 2;
        let storage_gb_hours = average_storage_gb.saturating_mul(delta_secs as u128) / 3_600;
        Ok(Some(MeasuredUsage {
            cpu_core_ticks: u64::try_from(delta_ticks).unwrap_or(u64::MAX),
            gpu_unit_ticks: 0,
            ram_mb_hours,
            storage_gb_hours,
            bandwidth_gb: u64::try_from(bandwidth_gb).unwrap_or(u64::MAX),
            uptime_hours: u64::try_from(uptime_hours).unwrap_or(u64::MAX),
        }))
    }
}

/// One independent verifier's signed measurement of a node's usage in one
/// settlement period.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignedUsageReport {
    pub node: AweId,
    pub period_start_unix: u64,
    pub period_end_unix: u64,
    pub usage: MeasuredUsage,
    pub verifier_public_key: [u8; 32],
    #[serde(with = "crate::serde_bytes_64")]
    pub signature: [u8; 64],
}

impl SignedUsageReport {
    pub fn new(
        verifier: &Identity,
        node: AweId,
        period_start_unix: u64,
        period_end_unix: u64,
        usage: MeasuredUsage,
    ) -> Self {
        let mut report = Self {
            node,
            period_start_unix,
            period_end_unix,
            usage,
            verifier_public_key: verifier.public.public_key,
            signature: [0u8; 64],
        };
        report.signature = verifier.sign(&report.signing_bytes());
        report
    }

    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut unsigned = self.clone();
        unsigned.signature = [0u8; 64];
        serde_json::to_vec(&unsigned).expect("signed usage report serialization")
    }

    pub fn verify(&self) -> bool {
        Identity::verify(
            &self.verifier_public_key,
            &self.signing_bytes(),
            &self.signature,
        )
    }
}

/// Collects independent verifier reports about a node's usage. The
/// consensus is deliberately conservative: at least MIN_INDEPENDENT_VERIFIERS
/// distinct valid verifiers must agree on the same node and period, and the
/// effective usage is the per-resource minimum across their reports, so no
/// single verifier can inflate a reward.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UsageVerification {
    pub reports: Vec<SignedUsageReport>,
}

impl UsageVerification {
    /// Adds a report. Returns false when the report window is full; a
    /// dropped report is never silently treated as agreement.
    pub fn add(&mut self, report: SignedUsageReport) -> bool {
        if self.reports.len() >= MAX_VERIFIER_REPORTS {
            return false;
        }
        self.reports.push(report);
        true
    }

    pub fn distinct_verifiers(&self) -> usize {
        let keys: BTreeSet<[u8; 32]> = self
            .reports
            .iter()
            .filter(|report| report.verify())
            .map(|report| report.verifier_public_key)
            .collect();
        keys.len()
    }

    pub fn consensus_usage(
        &self,
        node: &AweId,
        period_start_unix: u64,
        period_end_unix: u64,
    ) -> Option<MeasuredUsage> {
        if period_start_unix >= period_end_unix {
            return None;
        }
        let valid: Vec<&SignedUsageReport> = self
            .reports
            .iter()
            .filter(|report| {
                report.verify()
                    && report.node == *node
                    && report.period_start_unix == period_start_unix
                    && report.period_end_unix == period_end_unix
            })
            .collect();
        let verifiers: BTreeSet<[u8; 32]> = valid
            .iter()
            .map(|report| report.verifier_public_key)
            .collect();
        if valid.len() < MIN_INDEPENDENT_VERIFIERS || verifiers.len() < MIN_INDEPENDENT_VERIFIERS {
            return None;
        }
        Some(MeasuredUsage {
            cpu_core_ticks: valid
                .iter()
                .map(|r| r.usage.cpu_core_ticks)
                .min()
                .unwrap_or(0),
            gpu_unit_ticks: valid
                .iter()
                .map(|r| r.usage.gpu_unit_ticks)
                .min()
                .unwrap_or(0),
            ram_mb_hours: valid
                .iter()
                .map(|r| r.usage.ram_mb_hours)
                .min()
                .unwrap_or(0),
            storage_gb_hours: valid
                .iter()
                .map(|r| r.usage.storage_gb_hours)
                .min()
                .unwrap_or(0),
            bandwidth_gb: valid
                .iter()
                .map(|r| r.usage.bandwidth_gb)
                .min()
                .unwrap_or(0),
            uptime_hours: valid
                .iter()
                .map(|r| r.usage.uptime_hours)
                .min()
                .unwrap_or(0),
        })
    }

    /// Builds the settlement evidence: the node's own metered report, the
    /// verifier consensus as the network measurement and the delivered
    /// work results from the task runner.
    pub fn evidence(
        &self,
        node_report: MeasuredUsage,
        node: &AweId,
        period_start_unix: u64,
        period_end_unix: u64,
        delivered_results: Option<MeasuredUsage>,
    ) -> Option<UsageEvidence> {
        let network_measurement = self.consensus_usage(node, period_start_unix, period_end_unix)?;
        Some(UsageEvidence {
            node_report: Some(node_report),
            network_measurement: Some(network_measurement),
            delivered_results,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::founder::FounderAuthority;
    use crate::identity::Username;
    use crate::onecoin::OnecoinLedger;
    use crate::resource_economy::{ResourceEconomy, ResourceLimits};

    fn identity(name: &str) -> Identity {
        Identity::generate(Username::new(name).unwrap())
    }

    fn counters(cpu: u128, uptime: u64) -> SystemCounters {
        SystemCounters {
            cpu_core_ticks: cpu,
            ram_total_kb: 1_000_000,
            ram_available_kb: 500_000,
            net_rx_bytes: 0,
            net_tx_bytes: 0,
            uptime_secs: uptime,
            disk_total_kb: 0,
            disk_available_kb: 0,
        }
    }

    #[test]
    fn period_meter_converts_counter_deltas_into_usage() {
        let mut meter = UsagePeriodMeter::new(SequenceMeter::new(vec![
            counters(0, 0),
            counters(100, 7_200),
        ]));
        assert!(meter.last_counters().is_none());
        assert_eq!(meter.sample_period().unwrap(), None);
        assert!(meter.last_counters().is_some());
        let usage = meter.sample_period().unwrap().unwrap();
        assert_eq!(usage.cpu_core_ticks, 100);
        assert_eq!(usage.ram_mb_hours, 976);
        assert_eq!(usage.bandwidth_gb, 0);
        assert_eq!(usage.uptime_hours, 2);
    }

    #[test]
    fn period_meter_counts_transferred_bytes() {
        let before = counters(0, 0);
        let mut after = counters(10, 3_600);
        after.net_rx_bytes = 3_000_000_000;
        after.net_tx_bytes = 2_000_000_000;
        let mut meter = UsagePeriodMeter::new(SequenceMeter::new(vec![before, after]));
        meter.sample_period().unwrap();
        let usage = meter.sample_period().unwrap().unwrap();
        assert_eq!(usage.bandwidth_gb, 5);
        assert_eq!(usage.uptime_hours, 1);
    }

    #[test]
    fn period_meter_measures_contributed_storage_capacity_hours() {
        let mut before = counters(0, 0);
        before.disk_total_kb = 500_000_000;
        before.disk_available_kb = 200_000_000;
        let mut after = counters(10, 3_600);
        after.disk_total_kb = 500_000_000;
        after.disk_available_kb = 100_000_000;
        let mut meter = UsagePeriodMeter::new(SequenceMeter::new(vec![before, after]));
        meter.sample_period().unwrap();
        let usage = meter.sample_period().unwrap().unwrap();
        // average available capacity 150 GB over one hour
        assert_eq!(usage.storage_gb_hours, 150);
    }

    #[test]
    fn sequence_meter_never_invents_past_samples() {
        let mut meter = SequenceMeter::new(vec![counters(1, 1)]);
        assert!(meter.read().is_ok());
        assert!(meter.read().is_err());
    }

    #[test]
    fn consensus_needs_distinct_valid_verifiers() {
        let node = identity("ver-node");
        let verifier_a = identity("ver-a");
        let verifier_b = identity("ver-b");
        let usage = MeasuredUsage {
            cpu_core_ticks: 40,
            gpu_unit_ticks: 0,
            ram_mb_hours: 0,
            storage_gb_hours: 0,
            bandwidth_gb: 0,
            uptime_hours: 1,
        };
        let mut verification = UsageVerification::default();
        let report_a = SignedUsageReport::new(
            &verifier_a,
            node.public.awe_id.clone(),
            0,
            3_600,
            usage.clone(),
        );
        // a single report is not consensus
        verification.add(report_a.clone());
        assert_eq!(verification.distinct_verifiers(), 1);
        assert!(verification
            .consensus_usage(&node.public.awe_id, 0, 3_600)
            .is_none());
        // the same verifier twice is still not consensus
        verification.add(report_a);
        assert_eq!(verification.distinct_verifiers(), 1);
        assert!(verification
            .consensus_usage(&node.public.awe_id, 0, 3_600)
            .is_none());
        // a second distinct verifier completes the consensus
        let report_b = SignedUsageReport::new(
            &verifier_b,
            node.public.awe_id.clone(),
            0,
            3_600,
            usage.clone(),
        );
        verification.add(report_b);
        assert_eq!(verification.distinct_verifiers(), 2);
        let consensus = verification
            .consensus_usage(&node.public.awe_id, 0, 3_600)
            .unwrap();
        assert_eq!(consensus, usage);
        assert!(verification
            .consensus_usage(&node.public.awe_id, 3_600, 3_600)
            .is_none());
        // the consensus ignores reports about a different period
        assert!(verification
            .consensus_usage(&node.public.awe_id, 3_600, 7_200)
            .is_none());
    }

    #[test]
    fn tampered_reports_cannot_drive_consensus() {
        let node = identity("tamper-node");
        let verifier_a = identity("tamper-a");
        let verifier_b = identity("tamper-b");
        let usage = MeasuredUsage {
            cpu_core_ticks: 10,
            gpu_unit_ticks: 0,
            ram_mb_hours: 0,
            storage_gb_hours: 0,
            bandwidth_gb: 0,
            uptime_hours: 1,
        };
        let mut tampered = SignedUsageReport::new(
            &verifier_a,
            node.public.awe_id.clone(),
            0,
            3_600,
            usage.clone(),
        );
        tampered.usage.cpu_core_ticks = 10_000;
        assert!(!tampered.verify());
        let mut verification = UsageVerification::default();
        verification.add(tampered);
        verification.add(SignedUsageReport::new(
            &verifier_b,
            node.public.awe_id.clone(),
            0,
            3_600,
            usage,
        ));
        assert_eq!(verification.distinct_verifiers(), 1);
        assert!(verification
            .consensus_usage(&node.public.awe_id, 0, 3_600)
            .is_none());
    }

    #[test]
    fn metered_evidence_settles_in_the_resource_economy() {
        let founder = identity("meter-founder");
        let node_owner = identity("meter-node");
        let verifier_a = identity("meter-verifier-a");
        let verifier_b = identity("meter-verifier-b");
        let mut ledger = OnecoinLedger::default();
        ledger
            .initialize_genesis_with_verifiers(
                &[
                    founder.public.awe_id.clone(),
                    node_owner.public.awe_id.clone(),
                    verifier_a.public.awe_id.clone(),
                    verifier_b.public.awe_id.clone(),
                ],
                &[founder.public.public_key],
            )
            .unwrap();
        let mut meter = UsagePeriodMeter::new(SequenceMeter::new(vec![
            counters(0, 0),
            counters(100, 7_200),
        ]));
        meter.sample_period().unwrap();
        let node_report = meter.sample_period().unwrap().unwrap();
        let mut verification = UsageVerification::default();
        verification.add(SignedUsageReport::new(
            &verifier_a,
            node_owner.public.awe_id.clone(),
            0,
            7_200,
            node_report.clone(),
        ));
        verification.add(SignedUsageReport::new(
            &verifier_b,
            node_owner.public.awe_id.clone(),
            0,
            7_200,
            node_report.clone(),
        ));
        let evidence = verification
            .evidence(
                node_report.clone(),
                &node_owner.public.awe_id,
                0,
                7_200,
                Some(node_report),
            )
            .unwrap();
        let mut economy = ResourceEconomy::default();
        economy
            .grant(
                &node_owner,
                ResourceLimits {
                    cpu_core_ticks: 100,
                    gpu_unit_ticks: 0,
                    ram_mb_hours: 1_000,
                    storage_gb_hours: 0,
                    bandwidth_gb: 10,
                    uptime_hours: 24,
                },
                1,
            )
            .unwrap();
        let settlement = economy
            .settle(
                &mut ledger,
                &founder,
                &node_owner.public.awe_id,
                &evidence,
                0,
                7_200,
            )
            .unwrap();
        // cpu 100 * 1000 + ram 976 * 10 + uptime 2 * 100 = 109_960 usage atoms
        assert_eq!(settlement.usage_atoms, 109_960);
        assert_eq!(settlement.availability_atoms, 100);
        let _ = FounderAuthority::install(&founder, 0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn proc_parsers_read_real_formats() {
        assert_eq!(
            parse_proc_stat_cpu_ticks("cpu  10 20 30 40 0 0 0 0\ncpu0 1 2 3 4"),
            Some(100)
        );
        assert_eq!(
            parse_proc_meminfo("MemTotal:       1000 kB\nMemAvailable:    400 kB\n"),
            Some((1000, 400))
        );
        let netdev = "Inter-|   Receive                                                |  Transmit\n face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n  lo:       100        1    0    0    0     0          0         0        0        1    0    0    0     0       0          0\n eth0:     500        2    0    0    0     0          0         0      700        2    0    0    0     0       0          0\n";
        assert_eq!(parse_proc_net_dev(netdev), Some((500, 700)));
        assert_eq!(parse_proc_uptime_secs("12345.67 12345.67\n"), Some(12345));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn host_meter_reads_real_counters() {
        let mut meter = HostMeter::default();
        let first = meter.read().expect("host counters");
        let second = meter.read().expect("host counters");
        assert!(second.cpu_core_ticks >= first.cpu_core_ticks);
        assert!(second.uptime_secs >= first.uptime_secs);
        assert!(first.ram_total_kb >= first.ram_available_kb);
        assert!(first.disk_total_kb >= first.disk_available_kb);
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn host_meter_is_honest_off_linux() {
        let mut meter = HostMeter;
        assert!(meter.read().is_err());
    }
}
