//! Fair resource balancing for AWE hosted workloads.
//!
//! The balancer does not ban workloads by type. It divides a node's declared
//! capacity into bounded leases so one consumer cannot monopolize the node.
//! Capacity is local and privacy-preserving: consumers are tracked by an
//! opaque connection key, not by username or AWE-ID.

use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct ResourceCapacity {
    pub cpu_slots: u32,
    pub memory_bytes: u64,
    pub storage_bytes: u64,
    pub bandwidth_bytes_per_sec: u64,
}

impl ResourceCapacity {
    pub fn normalized(self) -> Self {
        Self {
            cpu_slots: self.cpu_slots.max(1),
            memory_bytes: self.memory_bytes,
            storage_bytes: self.storage_bytes,
            bandwidth_bytes_per_sec: self.bandwidth_bytes_per_sec,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResourceRequest {
    pub cpu_slots: u32,
    pub memory_bytes: u64,
    pub storage_bytes: u64,
    pub bandwidth_bytes: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct ResourceUsage {
    pub cpu_slots: u32,
    pub memory_bytes: u64,
    pub storage_bytes: u64,
    pub bandwidth_bytes: u64,
}

impl ResourceUsage {
    fn sub(&mut self, request: ResourceRequest) {
        self.cpu_slots = self.cpu_slots.saturating_sub(request.cpu_slots);
        self.memory_bytes = self.memory_bytes.saturating_sub(request.memory_bytes);
        self.storage_bytes = self.storage_bytes.saturating_sub(request.storage_bytes);
        self.bandwidth_bytes = self.bandwidth_bytes.saturating_sub(request.bandwidth_bytes);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceLease {
    pub key: String,
    pub request: ResourceRequest,
}

#[derive(Clone, Debug)]
pub struct ResourceBalancer {
    capacity: ResourceCapacity,
    max_share_percent: u8,
    active: BTreeMap<String, ResourceUsage>,
}

impl ResourceBalancer {
    pub fn new(capacity: ResourceCapacity, max_share_percent: u8) -> Self {
        Self {
            capacity: capacity.normalized(),
            max_share_percent: max_share_percent.clamp(1, 100),
            active: BTreeMap::new(),
        }
    }

    pub fn capacity(&self) -> ResourceCapacity {
        self.capacity
    }

    pub fn usage(&self) -> ResourceUsage {
        self.active
            .values()
            .fold(ResourceUsage::default(), |mut total, usage| {
                total.cpu_slots = total.cpu_slots.saturating_add(usage.cpu_slots);
                total.memory_bytes = total.memory_bytes.saturating_add(usage.memory_bytes);
                total.storage_bytes = total.storage_bytes.saturating_add(usage.storage_bytes);
                total.bandwidth_bytes = total.bandwidth_bytes.saturating_add(usage.bandwidth_bytes);
                total
            })
    }

    pub fn active_consumers(&self) -> usize {
        self.active.len()
    }

    pub fn per_consumer_cpu_limit(&self) -> u32 {
        ((self.capacity.cpu_slots as u64 * self.max_share_percent as u64) / 100).max(1) as u32
    }

    pub fn per_consumer_memory_limit(&self) -> u64 {
        self.capacity
            .memory_bytes
            .saturating_mul(self.max_share_percent as u64)
            / 100
    }

    pub fn per_consumer_storage_limit(&self) -> u64 {
        self.capacity
            .storage_bytes
            .saturating_mul(self.max_share_percent as u64)
            / 100
    }

    pub fn try_acquire(
        &mut self,
        key: impl Into<String>,
        request: ResourceRequest,
    ) -> Option<ResourceLease> {
        if request.cpu_slots == 0
            || request.cpu_slots > self.per_consumer_cpu_limit()
            || (self.capacity.memory_bytes != 0
                && request.memory_bytes > self.per_consumer_memory_limit())
            || (self.capacity.storage_bytes != 0
                && request.storage_bytes > self.per_consumer_storage_limit())
            || (self.capacity.bandwidth_bytes_per_sec != 0
                && request.bandwidth_bytes > self.capacity.bandwidth_bytes_per_sec)
        {
            return None;
        }

        let key = key.into();
        let current = self.active.get(&key).copied().unwrap_or_default();
        let next = ResourceUsage {
            cpu_slots: current.cpu_slots.saturating_add(request.cpu_slots),
            memory_bytes: current.memory_bytes.saturating_add(request.memory_bytes),
            storage_bytes: current.storage_bytes.saturating_add(request.storage_bytes),
            bandwidth_bytes: current
                .bandwidth_bytes
                .saturating_add(request.bandwidth_bytes),
        };
        if next.cpu_slots > self.per_consumer_cpu_limit()
            || (self.capacity.memory_bytes != 0
                && next.memory_bytes > self.per_consumer_memory_limit())
            || (self.capacity.storage_bytes != 0
                && next.storage_bytes > self.per_consumer_storage_limit())
        {
            return None;
        }

        let total = self.usage();
        let capacity_ok = total.cpu_slots.saturating_add(request.cpu_slots)
            <= self.capacity.cpu_slots
            && (self.capacity.memory_bytes == 0
                || total.memory_bytes.saturating_add(request.memory_bytes)
                    <= self.capacity.memory_bytes)
            && (self.capacity.storage_bytes == 0
                || total.storage_bytes.saturating_add(request.storage_bytes)
                    <= self.capacity.storage_bytes);
        if !capacity_ok {
            return None;
        }

        self.active.insert(key.clone(), next);
        Some(ResourceLease { key, request })
    }

    pub fn release(&mut self, lease: &ResourceLease) {
        if let Some(usage) = self.active.get_mut(&lease.key) {
            usage.sub(lease.request);
            if *usage == ResourceUsage::default() {
                self.active.remove(&lease.key);
            }
        }
    }

    /// Returns whether a hosted feature is allowed to consume local resources.
    /// A declared contribution is required when policy demands reciprocity.
    pub fn allows_hosted_feature(&self, declared_return_resources: bool) -> bool {
        declared_return_resources
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_consumer_cannot_monopolize_capacity() {
        let mut b = ResourceBalancer::new(
            ResourceCapacity {
                cpu_slots: 8,
                memory_bytes: 1_000,
                storage_bytes: 10_000,
                bandwidth_bytes_per_sec: 10_000,
            },
            25,
        );
        let first = b.try_acquire(
            "peer-a",
            ResourceRequest {
                cpu_slots: 2,
                memory_bytes: 200,
                storage_bytes: 2_000,
                bandwidth_bytes: 1_000,
            },
        );
        assert!(first.is_some());
        assert!(b
            .try_acquire(
                "peer-a",
                ResourceRequest {
                    cpu_slots: 1,
                    memory_bytes: 100,
                    storage_bytes: 1_000,
                    bandwidth_bytes: 500,
                },
            )
            .is_none());
    }

    #[test]
    fn different_consumers_share_capacity() {
        let mut b = ResourceBalancer::new(
            ResourceCapacity {
                cpu_slots: 8,
                memory_bytes: 1_000,
                storage_bytes: 10_000,
                bandwidth_bytes_per_sec: 10_000,
            },
            25,
        );
        assert!(b
            .try_acquire(
                "a",
                ResourceRequest {
                    cpu_slots: 2,
                    memory_bytes: 100,
                    storage_bytes: 1_000,
                    bandwidth_bytes: 1_000
                }
            )
            .is_some());
        assert!(b
            .try_acquire(
                "b",
                ResourceRequest {
                    cpu_slots: 2,
                    memory_bytes: 100,
                    storage_bytes: 1_000,
                    bandwidth_bytes: 1_000
                }
            )
            .is_some());
        assert_eq!(b.active_consumers(), 2);
    }

    #[test]
    fn release_returns_capacity() {
        let mut b = ResourceBalancer::new(
            ResourceCapacity {
                cpu_slots: 4,
                memory_bytes: 1_000,
                storage_bytes: 10_000,
                bandwidth_bytes_per_sec: 10_000,
            },
            50,
        );
        let lease = b
            .try_acquire(
                "a",
                ResourceRequest {
                    cpu_slots: 2,
                    memory_bytes: 100,
                    storage_bytes: 1_000,
                    bandwidth_bytes: 1_000,
                },
            )
            .unwrap();
        b.release(&lease);
        assert_eq!(b.usage(), ResourceUsage::default());
    }
}
