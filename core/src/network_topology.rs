use std::collections::{HashMap, HashSet, VecDeque};

pub type NodeId = String;
pub type DataCentreId = String;
pub type DataGroupId = String;
pub type CentreGroupId = String;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CentreLinkMode {
    FullMesh,
    Relay { relay_node: NodeId },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PeerMetrics {
    pub rtt_ms: u32,
    pub throughput_kbps: u64,
    pub load_percent: u8,
    pub reliability_percent: u8,
    pub last_seen_unix: u64,
}

impl PeerMetrics {
    pub fn score(&self) -> u64 {
        let rtt = self.rtt_ms.max(1) as u64;
        let load = self.load_percent.min(100) as u64;
        let reliability = self.reliability_percent.min(100) as u64;
        rtt.saturating_mul(4) + load.saturating_mul(10) + (100 - reliability).saturating_mul(20)
    }
    pub fn is_healthy(&self, now_unix: u64, max_age: u64) -> bool {
        self.last_seen_unix > 0
            && now_unix.saturating_sub(self.last_seen_unix) <= max_age
            && self.reliability_percent >= 50
    }
}

#[derive(Clone, Debug)]
pub struct Node {
    pub id: NodeId,
    pub peers: HashSet<NodeId>,
    pub alive: bool,
    pub metrics: PeerMetrics,
}

impl Node {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            peers: HashSet::new(),
            alive: true,
            metrics: PeerMetrics::default(),
        }
    }
    pub fn set_health(&mut self, alive: bool) {
        self.alive = alive;
    }
    pub fn update_metrics(&mut self, metrics: PeerMetrics) {
        self.metrics = metrics;
    }
}

#[derive(Clone, Debug)]
pub struct CentreLink {
    pub mode: CentreLinkMode,
    pub relays: Vec<NodeId>,
    pub healthy: bool,
}

#[derive(Clone, Debug)]
pub struct DataCentre {
    pub id: DataCentreId,
    pub nodes: HashMap<NodeId, Node>,
    pub links: HashSet<DataCentreId>,
    pub link_state: HashMap<DataCentreId, CentreLink>,
}

#[derive(Clone, Debug)]
pub struct DataGroup {
    pub id: DataGroupId,
    pub centres: HashSet<DataCentreId>,
}
#[derive(Clone, Debug)]
pub struct CentreGroup {
    pub id: CentreGroupId,
    pub groups: HashSet<DataGroupId>,
}

#[derive(Default, Debug)]
pub struct AweNet {
    pub centres: HashMap<DataCentreId, DataCentre>,
    pub data_groups: HashMap<DataGroupId, DataGroup>,
    pub centre_groups: HashMap<CentreGroupId, CentreGroup>,
}

impl DataCentre {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            nodes: HashMap::new(),
            links: HashSet::new(),
            link_state: HashMap::new(),
        }
    }

    pub fn add_node(&mut self, n: Node) {
        if n.alive {
            if let Some(peer) = self.nearest(&n.id) {
                if let Some(existing) = self.nodes.get_mut(&peer) {
                    existing.peers.insert(n.id.clone());
                }
            }
        }
        self.nodes.insert(n.id.clone(), n);
    }

    pub fn full_mesh(&mut self) {
        let ids: Vec<_> = self
            .nodes
            .values()
            .filter(|n| n.alive)
            .map(|n| n.id.clone())
            .collect();
        for a in &ids {
            for b in &ids {
                if a != b {
                    self.nodes
                        .get_mut(a)
                        .expect("node exists")
                        .peers
                        .insert(b.clone());
                }
            }
        }
    }

    pub fn nearest(&self, from: &NodeId) -> Option<NodeId> {
        self.nodes
            .values()
            .filter(|n| n.alive && &n.id != from)
            .min_by_key(|n| (n.metrics.score(), n.peers.len() as u64, n.id.clone()))
            .map(|n| n.id.clone())
    }

    pub fn mark_node(&mut self, id: &NodeId, alive: bool) -> Result<(), String> {
        self.nodes
            .get_mut(id)
            .map(|n| n.alive = alive)
            .ok_or_else(|| "node not found".into())
    }

    pub fn healthy_nodes(&self, now_unix: u64, max_age: u64) -> Vec<NodeId> {
        self.nodes
            .values()
            .filter(|n| {
                n.alive
                    && (n.metrics.last_seen_unix == 0 || n.metrics.is_healthy(now_unix, max_age))
            })
            .map(|n| n.id.clone())
            .collect()
    }
}

impl DataGroup {
    pub fn new(
        id: impl Into<String>,
        centres: impl IntoIterator<Item = DataCentreId>,
    ) -> Result<Self, String> {
        let centres: HashSet<DataCentreId> = centres.into_iter().collect();
        if centres.len() < 3 {
            return Err("DataGroup requires at least 3 distinct data centres".into());
        }
        Ok(Self {
            id: id.into(),
            centres,
        })
    }
}

impl CentreGroup {
    pub fn new(
        id: impl Into<String>,
        groups: impl IntoIterator<Item = DataGroupId>,
    ) -> Result<Self, String> {
        let groups: HashSet<DataGroupId> = groups.into_iter().collect();
        if groups.len() < 2 {
            return Err("CentreGroup requires at least 2 distinct data groups".into());
        }
        Ok(Self {
            id: id.into(),
            groups,
        })
    }
}

impl AweNet {
    pub fn add_centre(&mut self, d: DataCentre) -> Result<(), String> {
        if self.centres.contains_key(&d.id) {
            return Err("data centre ID already exists".into());
        }
        for existing in self.centres.values() {
            if d.nodes.keys().any(|id| existing.nodes.contains_key(id)) {
                return Err("node IDs must be globally unique across data centres".into());
            }
        }
        self.centres.insert(d.id.clone(), d);
        Ok(())
    }

    pub fn add_data_group(&mut self, group: DataGroup) -> Result<(), String> {
        if self.data_groups.contains_key(&group.id) {
            return Err("data group ID already exists".into());
        }
        if !group.centres.iter().all(|id| self.centres.contains_key(id)) {
            return Err("data group references an unknown centre".into());
        }
        self.data_groups.insert(group.id.clone(), group);
        Ok(())
    }

    pub fn add_centre_group(&mut self, group: CentreGroup) -> Result<(), String> {
        if self.centre_groups.contains_key(&group.id) {
            return Err("centre group ID already exists".into());
        }
        if !group
            .groups
            .iter()
            .all(|id| self.data_groups.contains_key(id))
        {
            return Err("centre group references an unknown data group".into());
        }
        self.centre_groups.insert(group.id.clone(), group);
        Ok(())
    }

    pub fn connect_centres(
        &mut self,
        a: &str,
        b: &str,
        mode: CentreLinkMode,
    ) -> Result<(), String> {
        if a == b || !self.centres.contains_key(a) || !self.centres.contains_key(b) {
            return Err("invalid data-centre pair".into());
        }
        let left: Vec<_> = self.centres[a]
            .nodes
            .values()
            .filter(|n| n.alive)
            .map(|n| n.id.clone())
            .collect();
        let right: Vec<_> = self.centres[b]
            .nodes
            .values()
            .filter(|n| n.alive)
            .map(|n| n.id.clone())
            .collect();
        if left.is_empty() || right.is_empty() {
            return Err("both centres must contain live nodes".into());
        }

        let (relays, stored_mode) = match mode {
            CentreLinkMode::FullMesh => {
                for x in &left {
                    for y in &right {
                        self.centres
                            .get_mut(a)
                            .unwrap()
                            .nodes
                            .get_mut(x)
                            .unwrap()
                            .peers
                            .insert(y.clone());
                        self.centres
                            .get_mut(b)
                            .unwrap()
                            .nodes
                            .get_mut(y)
                            .unwrap()
                            .peers
                            .insert(x.clone());
                    }
                }
                (left.clone(), CentreLinkMode::FullMesh)
            }
            CentreLinkMode::Relay { relay_node } => {
                if !left.contains(&relay_node) {
                    return Err("relay node missing or offline".into());
                }
                let target = right
                    .iter()
                    .min_by_key(|id| self.centres[b].nodes[*id].metrics.score())
                    .unwrap()
                    .clone();
                self.centres
                    .get_mut(a)
                    .unwrap()
                    .nodes
                    .get_mut(&relay_node)
                    .unwrap()
                    .peers
                    .insert(target.clone());
                self.centres
                    .get_mut(b)
                    .unwrap()
                    .nodes
                    .get_mut(&target)
                    .unwrap()
                    .peers
                    .insert(relay_node.clone());
                (
                    vec![relay_node.clone()],
                    CentreLinkMode::Relay { relay_node },
                )
            }
        };
        self.centres.get_mut(a).unwrap().links.insert(b.into());
        self.centres.get_mut(b).unwrap().links.insert(a.into());
        self.centres.get_mut(a).unwrap().link_state.insert(
            b.into(),
            CentreLink {
                mode: stored_mode.clone(),
                relays: relays.clone(),
                healthy: true,
            },
        );
        self.centres.get_mut(b).unwrap().link_state.insert(
            a.into(),
            CentreLink {
                mode: stored_mode,
                relays,
                healthy: true,
            },
        );
        Ok(())
    }

    pub fn failover_relay(&mut self, a: &str, b: &str) -> Result<NodeId, String> {
        let state = self
            .centres
            .get(a)
            .and_then(|c| c.link_state.get(b))
            .cloned()
            .ok_or("link not found")?;
        let candidate = self.centres[a]
            .nodes
            .values()
            .filter(|n| n.alive && !state.relays.contains(&n.id))
            .min_by_key(|n| n.metrics.score())
            .map(|n| n.id.clone())
            .ok_or("no healthy relay available")?;
        let target = self.centres[b]
            .nodes
            .values()
            .filter(|n| n.alive)
            .min_by_key(|n| n.metrics.score())
            .map(|n| n.id.clone())
            .ok_or("target centre has no live nodes")?;
        self.centres
            .get_mut(a)
            .unwrap()
            .nodes
            .get_mut(&candidate)
            .unwrap()
            .peers
            .insert(target.clone());
        self.centres
            .get_mut(b)
            .unwrap()
            .nodes
            .get_mut(&target)
            .unwrap()
            .peers
            .insert(candidate.clone());
        self.centres
            .get_mut(a)
            .unwrap()
            .link_state
            .get_mut(b)
            .unwrap()
            .relays
            .push(candidate.clone());
        self.centres
            .get_mut(b)
            .unwrap()
            .link_state
            .get_mut(a)
            .unwrap()
            .relays
            .push(candidate.clone());
        Ok(candidate)
    }

    pub fn validate(&self) -> Result<(), String> {
        for (id, c) in &self.centres {
            for n in c.nodes.values() {
                for peer in &n.peers {
                    if !self.centres.values().any(|dc| dc.nodes.contains_key(peer)) {
                        return Err(format!("node {id} references unknown peer {peer}"));
                    }
                }
            }
        }
        for g in self.data_groups.values() {
            if g.centres.len() < 3 || !g.centres.iter().all(|id| self.centres.contains_key(id)) {
                return Err("invalid data group".into());
            }
        }
        for g in self.centre_groups.values() {
            if g.groups.len() < 2 || !g.groups.iter().all(|id| self.data_groups.contains_key(id)) {
                return Err("invalid centre group".into());
            }
        }
        Ok(())
    }

    pub fn route(&self, src: &NodeId, dst: &NodeId) -> Option<Vec<NodeId>> {
        if src == dst {
            return Some(vec![src.clone()]);
        }
        let mut adjacency: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for dc in self.centres.values() {
            for n in dc.nodes.values().filter(|n| n.alive) {
                adjacency.entry(n.id.clone()).or_default().extend(
                    n.peers
                        .iter()
                        .filter(|p| {
                            self.centres
                                .values()
                                .any(|other| other.nodes.get(*p).map(|x| x.alive).unwrap_or(false))
                        })
                        .cloned(),
                );
            }
        }
        let mut q = VecDeque::from([src.clone()]);
        let mut prev: HashMap<NodeId, Option<NodeId>> = HashMap::from([(src.clone(), None)]);
        while let Some(x) = q.pop_front() {
            if &x == dst {
                break;
            }
            for p in adjacency.get(&x).into_iter().flatten() {
                if !prev.contains_key(p) {
                    prev.insert(p.clone(), Some(x.clone()));
                    q.push_back(p.clone());
                }
            }
        }
        if !prev.contains_key(dst) {
            return None;
        }
        let mut out = Vec::new();
        let mut x = dst.clone();
        loop {
            out.push(x.clone());
            match prev[&x].clone() {
                Some(p) => x = p,
                None => break,
            }
        }
        out.reverse();
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mesh_ignores_dead_nodes() {
        let mut c = DataCentre::new("dc");
        c.add_node(Node::new("a"));
        let mut b = Node::new("b");
        b.alive = false;
        c.add_node(b);
        c.full_mesh();
        assert!(c.nodes["a"].peers.is_empty());
    }
    #[test]
    fn duplicate_node_ids_are_rejected() {
        let mut n = AweNet::default();
        let mut a = DataCentre::new("a");
        a.add_node(Node::new("same"));
        let mut b = DataCentre::new("b");
        b.add_node(Node::new("same"));
        assert!(n.add_centre(a).is_ok());
        assert!(n.add_centre(b).is_err());
    }
    #[test]
    fn duplicate_data_centre_ids_are_rejected() {
        let mut network = AweNet::default();
        assert!(network.add_centre(DataCentre::new("same")).is_ok());
        assert!(network.add_centre(DataCentre::new("same")).is_err());
        assert_eq!(network.centres.len(), 1);
    }

    #[test]
    fn groups_require_distinct_members_and_ids_cannot_overwrite() {
        assert!(DataGroup::new("g", ["a".to_string(), "a".to_string(), "b".to_string()]).is_err());
        assert!(CentreGroup::new("cg", ["g".to_string(), "g".to_string()]).is_err());

        let mut network = AweNet::default();
        for id in ["a", "b", "c"] {
            network.add_centre(DataCentre::new(id)).unwrap();
        }
        let group = DataGroup::new("group", ["a".into(), "b".into(), "c".into()]).unwrap();
        assert!(network.add_data_group(group.clone()).is_ok());
        assert!(network.add_data_group(group).is_err());
        assert_eq!(network.data_groups.len(), 1);
    }

    #[test]
    fn relay_failover_adds_backup() {
        let mut n = AweNet::default();
        let mut a = DataCentre::new("a");
        a.add_node(Node::new("a1"));
        a.add_node(Node::new("a2"));
        let mut b = DataCentre::new("b");
        b.add_node(Node::new("b1"));
        n.add_centre(a).unwrap();
        n.add_centre(b).unwrap();
        n.connect_centres(
            "a",
            "b",
            CentreLinkMode::Relay {
                relay_node: "a1".into(),
            },
        )
        .unwrap();
        assert!(n.failover_relay("a", "b").is_ok());
    }
}
