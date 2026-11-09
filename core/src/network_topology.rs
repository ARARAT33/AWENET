use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};

pub type NodeId = String;
pub type DataCentreId = String;
pub type DataGroupId = String;
pub type CentreGroupId = String;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CentreLinkMode {
    FullMesh,
    Relay { relay_node: NodeId },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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
            && self.last_seen_unix <= now_unix
            && now_unix - self.last_seen_unix <= max_age
            && self.reliability_percent >= 50
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CentreLink {
    pub mode: CentreLinkMode,
    pub relays: Vec<NodeId>,
    pub healthy: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataCentre {
    pub id: DataCentreId,
    pub nodes: HashMap<NodeId, Node>,
    pub links: HashSet<DataCentreId>,
    pub link_state: HashMap<DataCentreId, CentreLink>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataGroup {
    pub id: DataGroupId,
    pub centres: HashSet<DataCentreId>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CentreGroup {
    pub id: CentreGroupId,
    pub groups: HashSet<DataGroupId>,
}

#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

    /// Adds a node without replacing an existing identity. When the node has
    /// no explicit peers, it connects to the nearest live node in both
    /// directions so routing works regardless of which endpoint is the source.
    pub fn add_node(&mut self, mut node: Node) -> Result<(), String> {
        let id = node.id.clone();
        if id.trim().is_empty() {
            return Err("node ID must not be empty".into());
        }
        if self.nodes.contains_key(&id) {
            return Err("node already exists in data centre".into());
        }
        if node.peers.iter().any(|peer| !self.nodes.contains_key(peer)) {
            return Err("node references an unknown peer in its data centre".into());
        }
        if node.alive && node.peers.is_empty() {
            if let Some(peer) = self.nearest(&id) {
                node.peers.insert(peer);
            }
        }
        for peer in &node.peers {
            if let Some(existing) = self.nodes.get_mut(peer) {
                existing.peers.insert(id.clone());
            }
        }
        self.nodes.insert(id, node);
        Ok(())
    }

    pub fn remove_node(&mut self, id: &str) -> Result<Node, String> {
        let removed = self
            .nodes
            .remove(id)
            .ok_or_else(|| "node not found".to_string())?;
        for node in self.nodes.values_mut() {
            node.peers.remove(id);
        }
        Ok(removed)
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
        let id = id.into();
        let centres: HashSet<DataCentreId> = centres.into_iter().collect();
        if id.trim().is_empty() {
            return Err("data group ID must not be empty".into());
        }
        if centres.len() < 3 {
            return Err("DataGroup requires at least 3 distinct data centres".into());
        }
        Ok(Self { id, centres })
    }
}

impl CentreGroup {
    pub fn new(
        id: impl Into<String>,
        groups: impl IntoIterator<Item = DataGroupId>,
    ) -> Result<Self, String> {
        let id = id.into();
        let groups: HashSet<DataGroupId> = groups.into_iter().collect();
        if id.trim().is_empty() {
            return Err("centre group ID must not be empty".into());
        }
        if groups.len() < 2 {
            return Err("CentreGroup requires at least 2 distinct data groups".into());
        }
        Ok(Self { id, groups })
    }
}

impl AweNet {
    pub fn add_centre(&mut self, centre: DataCentre) -> Result<(), String> {
        if centre.id.trim().is_empty() {
            return Err("data centre ID must not be empty".into());
        }
        if self.centres.contains_key(&centre.id) {
            return Err("data centre already exists".into());
        }
        for (node_id, node) in &centre.nodes {
            if node_id.trim().is_empty() || node_id != &node.id {
                return Err("node map key must match a non-empty node ID".into());
            }
            if node
                .peers
                .iter()
                .any(|peer| !centre.nodes.contains_key(peer))
            {
                return Err("new data centre contains a peer outside its own membership".into());
            }
            if self
                .centres
                .values()
                .any(|existing| existing.nodes.contains_key(node_id))
            {
                return Err("node IDs must be globally unique across data centres".into());
            }
        }
        self.centres.insert(centre.id.clone(), centre);
        Ok(())
    }

    pub fn add_data_group(&mut self, group: DataGroup) -> Result<(), String> {
        if group.id.trim().is_empty() {
            return Err("data group ID must not be empty".into());
        }
        if self.data_groups.contains_key(&group.id) {
            return Err("data group already exists".into());
        }
        if group.centres.len() < 3 {
            return Err("data group requires at least 3 distinct data centres".into());
        }
        if !group.centres.iter().all(|id| self.centres.contains_key(id)) {
            return Err("data group references an unknown centre".into());
        }
        self.data_groups.insert(group.id.clone(), group);
        Ok(())
    }

    pub fn add_centre_group(&mut self, group: CentreGroup) -> Result<(), String> {
        if group.id.trim().is_empty() {
            return Err("centre group ID must not be empty".into());
        }
        if self.centre_groups.contains_key(&group.id) {
            return Err("centre group already exists".into());
        }
        if group.groups.len() < 2 {
            return Err("centre group requires at least 2 distinct data groups".into());
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
        if self.centres[a].links.contains(b) || self.centres[b].links.contains(a) {
            return Err("data centres are already connected".into());
        }
        let left: Vec<_> = self.centres[a]
            .nodes
            .values()
            .filter(|node| node.alive)
            .map(|node| node.id.clone())
            .collect();
        let right: Vec<_> = self.centres[b]
            .nodes
            .values()
            .filter(|node| node.alive)
            .map(|node| node.id.clone())
            .collect();
        if left.is_empty() || right.is_empty() {
            return Err("both centres must contain live nodes".into());
        }

        let (left_relays, right_relays, left_mode, right_mode) = match mode {
            CentreLinkMode::FullMesh => {
                for left_id in &left {
                    for right_id in &right {
                        self.centres
                            .get_mut(a)
                            .and_then(|centre| centre.nodes.get_mut(left_id))
                            .ok_or("left node disappeared during link creation")?
                            .peers
                            .insert(right_id.clone());
                        self.centres
                            .get_mut(b)
                            .and_then(|centre| centre.nodes.get_mut(right_id))
                            .ok_or("right node disappeared during link creation")?
                            .peers
                            .insert(left_id.clone());
                    }
                }
                (
                    left.clone(),
                    right.clone(),
                    CentreLinkMode::FullMesh,
                    CentreLinkMode::FullMesh,
                )
            }
            CentreLinkMode::Relay { relay_node } => {
                if !left.contains(&relay_node) {
                    return Err("relay node missing or offline".into());
                }
                let target = right
                    .iter()
                    .min_by_key(|id| self.centres[b].nodes[*id].metrics.score())
                    .ok_or("target centre has no live node")?
                    .clone();
                self.centres
                    .get_mut(a)
                    .and_then(|centre| centre.nodes.get_mut(&relay_node))
                    .ok_or("relay node disappeared during link creation")?
                    .peers
                    .insert(target.clone());
                self.centres
                    .get_mut(b)
                    .and_then(|centre| centre.nodes.get_mut(&target))
                    .ok_or("target node disappeared during link creation")?
                    .peers
                    .insert(relay_node.clone());
                (
                    vec![relay_node.clone()],
                    vec![target.clone()],
                    CentreLinkMode::Relay { relay_node },
                    CentreLinkMode::Relay { relay_node: target },
                )
            }
        };

        self.centres.get_mut(a).unwrap().links.insert(b.into());
        self.centres.get_mut(b).unwrap().links.insert(a.into());
        self.centres.get_mut(a).unwrap().link_state.insert(
            b.into(),
            CentreLink {
                mode: left_mode,
                relays: left_relays,
                healthy: true,
            },
        );
        self.centres.get_mut(b).unwrap().link_state.insert(
            a.into(),
            CentreLink {
                mode: right_mode,
                relays: right_relays,
                healthy: true,
            },
        );
        Ok(())
    }

    pub fn failover_relay(&mut self, a: &str, b: &str) -> Result<NodeId, String> {
        let left_state = self
            .centres
            .get(a)
            .and_then(|centre| centre.link_state.get(b))
            .cloned()
            .ok_or("link not found")?;
        let right_state = self
            .centres
            .get(b)
            .and_then(|centre| centre.link_state.get(a))
            .cloned()
            .ok_or("reverse link state not found")?;
        let old_left = match &left_state.mode {
            CentreLinkMode::Relay { relay_node } => relay_node.clone(),
            CentreLinkMode::FullMesh => {
                return Err("full-mesh links do not use relay failover".into())
            }
        };
        let old_right = match &right_state.mode {
            CentreLinkMode::Relay { relay_node } => relay_node.clone(),
            CentreLinkMode::FullMesh => return Err("reverse link is not a relay link".into()),
        };

        let candidate = self.centres[a]
            .nodes
            .values()
            .filter(|node| node.alive && node.id != old_left)
            .min_by_key(|node| node.metrics.score())
            .map(|node| node.id.clone())
            .ok_or("no healthy relay available")?;
        let target = self.centres[b]
            .nodes
            .values()
            .filter(|node| node.alive)
            .min_by_key(|node| node.metrics.score())
            .map(|node| node.id.clone())
            .ok_or("target centre has no live nodes")?;

        if let Some(node) = self
            .centres
            .get_mut(a)
            .and_then(|centre| centre.nodes.get_mut(&old_left))
        {
            node.peers.remove(&old_right);
        }
        if let Some(node) = self
            .centres
            .get_mut(b)
            .and_then(|centre| centre.nodes.get_mut(&old_right))
        {
            node.peers.remove(&old_left);
        }
        self.centres
            .get_mut(a)
            .and_then(|centre| centre.nodes.get_mut(&candidate))
            .ok_or("new relay disappeared")?
            .peers
            .insert(target.clone());
        self.centres
            .get_mut(b)
            .and_then(|centre| centre.nodes.get_mut(&target))
            .ok_or("new target disappeared")?
            .peers
            .insert(candidate.clone());

        let left_link = self
            .centres
            .get_mut(a)
            .and_then(|centre| centre.link_state.get_mut(b))
            .ok_or("left link state disappeared")?;
        left_link.mode = CentreLinkMode::Relay {
            relay_node: candidate.clone(),
        };
        left_link.relays = vec![candidate.clone()];
        left_link.healthy = true;
        let right_link = self
            .centres
            .get_mut(b)
            .and_then(|centre| centre.link_state.get_mut(a))
            .ok_or("right link state disappeared")?;
        right_link.mode = CentreLinkMode::Relay {
            relay_node: target.clone(),
        };
        right_link.relays = vec![target];
        right_link.healthy = true;
        Ok(candidate)
    }

    pub fn disconnect_centres(&mut self, a: &str, b: &str) -> Result<(), String> {
        if a == b || !self.centres.contains_key(a) || !self.centres.contains_key(b) {
            return Err("invalid data-centre pair".into());
        }
        if !self.centres[a].links.contains(b) || !self.centres[b].links.contains(a) {
            return Err("data-centre link not found".into());
        }
        let left_ids = self.centres[a].nodes.keys().cloned().collect::<Vec<_>>();
        let right_ids = self.centres[b].nodes.keys().cloned().collect::<Vec<_>>();
        for id in &left_ids {
            if let Some(node) = self
                .centres
                .get_mut(a)
                .and_then(|centre| centre.nodes.get_mut(id))
            {
                for peer in &right_ids {
                    node.peers.remove(peer);
                }
            }
        }
        for id in &right_ids {
            if let Some(node) = self
                .centres
                .get_mut(b)
                .and_then(|centre| centre.nodes.get_mut(id))
            {
                for peer in &left_ids {
                    node.peers.remove(peer);
                }
            }
        }
        let left = self.centres.get_mut(a).unwrap();
        left.links.remove(b);
        left.link_state.remove(b);
        let right = self.centres.get_mut(b).unwrap();
        right.links.remove(a);
        right.link_state.remove(a);
        Ok(())
    }

    /// Updates the liveness flag for a node found anywhere in the topology.
    pub fn set_node_health(&mut self, node_id: &str, alive: bool) -> Result<(), String> {
        for centre in self.centres.values_mut() {
            if let Some(node) = centre.nodes.get_mut(node_id) {
                node.set_health(alive);
                return Ok(());
            }
        }
        Err("node not found".into())
    }

    /// Replaces the latest metrics for a known node.
    pub fn update_node_metrics(
        &mut self,
        node_id: &str,
        metrics: PeerMetrics,
    ) -> Result<(), String> {
        for centre in self.centres.values_mut() {
            if let Some(node) = centre.nodes.get_mut(node_id) {
                node.update_metrics(metrics);
                return Ok(());
            }
        }
        Err("node not found".into())
    }

    pub fn remove_node(&mut self, node_id: &str) -> Result<Node, String> {
        let centre_id = self
            .centres
            .iter()
            .find(|(_, centre)| centre.nodes.contains_key(node_id))
            .map(|(id, _)| id.clone())
            .ok_or_else(|| "node not found".to_string())?;
        for centre in self.centres.values() {
            for link in centre.link_state.values() {
                if matches!(&link.mode, CentreLinkMode::Relay { relay_node } if relay_node == node_id)
                {
                    return Err(
                        "cannot remove an active relay; fail over or disconnect its link first"
                            .into(),
                    );
                }
            }
        }
        let removed = self
            .centres
            .get_mut(&centre_id)
            .unwrap()
            .remove_node(node_id)?;
        for centre in self.centres.values_mut() {
            for node in centre.nodes.values_mut() {
                node.peers.remove(node_id);
            }
            for link in centre.link_state.values_mut() {
                link.relays.retain(|relay| relay != node_id);
            }
        }
        Ok(removed)
    }

    pub fn remove_centre(&mut self, id: &str) -> Result<DataCentre, String> {
        if self
            .data_groups
            .values()
            .any(|group| group.centres.contains(id))
        {
            return Err("cannot remove a data centre referenced by a data group".into());
        }
        let links = self
            .centres
            .get(id)
            .ok_or_else(|| "data centre not found".to_string())?
            .links
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        for peer in links {
            self.disconnect_centres(id, &peer)?;
        }
        let node_ids = self.centres[id].nodes.keys().cloned().collect::<Vec<_>>();
        for centre in self.centres.values_mut() {
            for node in centre.nodes.values_mut() {
                for removed in &node_ids {
                    node.peers.remove(removed);
                }
            }
        }
        self.centres
            .remove(id)
            .ok_or_else(|| "data centre not found".into())
    }

    pub fn remove_data_group(&mut self, id: &str) -> Result<DataGroup, String> {
        if self
            .centre_groups
            .values()
            .any(|group| group.groups.contains(id))
        {
            return Err("cannot remove a data group referenced by a centre group".into());
        }
        self.data_groups
            .remove(id)
            .ok_or_else(|| "data group not found".into())
    }

    pub fn remove_centre_group(&mut self, id: &str) -> Result<CentreGroup, String> {
        self.centre_groups
            .remove(id)
            .ok_or_else(|| "centre group not found".into())
    }

    pub fn validate(&self) -> Result<(), String> {
        let mut global_node_ids = HashSet::new();
        for (centre_id, centre) in &self.centres {
            if centre_id.is_empty() || centre.id != *centre_id {
                return Err("data-centre map key does not match its ID".into());
            }
            for (node_id, node) in &centre.nodes {
                if node_id.is_empty() || node.id != *node_id {
                    return Err("node map key does not match its ID".into());
                }
                if !global_node_ids.insert(node_id.clone()) {
                    return Err(format!(
                        "node ID {node_id} is duplicated across data centres"
                    ));
                }
                for peer_id in &node.peers {
                    let peer = self
                        .centres
                        .iter()
                        .find_map(|(peer_centre_id, peer_centre)| {
                            peer_centre
                                .nodes
                                .get(peer_id)
                                .map(|peer| (peer_centre_id, peer))
                        })
                        .ok_or_else(|| {
                            format!("node {node_id} references unknown peer {peer_id}")
                        })?;
                    if !peer.1.peers.contains(node_id) {
                        return Err(format!(
                            "peer link {node_id} <-> {peer_id} is not symmetric"
                        ));
                    }
                    if peer.0 != centre_id && !centre.links.contains(peer.0) {
                        return Err(format!(
                            "cross-centre peer link {node_id} -> {peer_id} has no centre link"
                        ));
                    }
                }
            }
            for peer_centre_id in &centre.links {
                let peer_centre = self.centres.get(peer_centre_id).ok_or_else(|| {
                    format!("centre {centre_id} references unknown centre {peer_centre_id}")
                })?;
                if !peer_centre.links.contains(centre_id) {
                    return Err(format!(
                        "centre link {centre_id} <-> {peer_centre_id} is not symmetric"
                    ));
                }
                let state = centre.link_state.get(peer_centre_id).ok_or_else(|| {
                    format!("centre link {centre_id} -> {peer_centre_id} has no state")
                })?;
                if !peer_centre.link_state.contains_key(centre_id) {
                    return Err(format!(
                        "reverse centre link {peer_centre_id} -> {centre_id} has no state"
                    ));
                }
                if let CentreLinkMode::Relay { relay_node } = &state.mode {
                    if !centre.nodes.contains_key(relay_node) || !state.relays.contains(relay_node)
                    {
                        return Err(format!(
                            "centre link {centre_id} -> {peer_centre_id} has an invalid relay"
                        ));
                    }
                }
                if state
                    .relays
                    .iter()
                    .any(|relay| !centre.nodes.contains_key(relay))
                {
                    return Err(format!(
                        "centre link {centre_id} -> {peer_centre_id} references an unknown relay"
                    ));
                }
            }
            if centre
                .link_state
                .keys()
                .any(|peer| !centre.links.contains(peer))
            {
                return Err(format!("centre {centre_id} has link state without a link"));
            }
        }
        for (group_id, group) in &self.data_groups {
            if group_id.is_empty()
                || group.id != *group_id
                || group.centres.len() < 3
                || !group.centres.iter().all(|id| self.centres.contains_key(id))
            {
                return Err("invalid data group".into());
            }
        }
        for (group_id, group) in &self.centre_groups {
            if group_id.is_empty()
                || group.id != *group_id
                || group.groups.len() < 2
                || !group
                    .groups
                    .iter()
                    .all(|id| self.data_groups.contains_key(id))
            {
                return Err("invalid centre group".into());
            }
        }
        Ok(())
    }

    pub fn route(&self, src: &NodeId, dst: &NodeId) -> Option<Vec<NodeId>> {
        let node_exists = |id: &str| {
            self.centres
                .values()
                .any(|centre| centre.nodes.contains_key(id))
        };
        if !node_exists(src) || !node_exists(dst) {
            return None;
        }
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
        c.add_node(Node::new("a")).unwrap();
        let mut b = Node::new("b");
        b.alive = false;
        c.add_node(b).unwrap();
        c.full_mesh();
        assert!(c.nodes["a"].peers.is_empty());
    }
    #[test]
    fn duplicate_node_ids_are_rejected() {
        let mut n = AweNet::default();
        let mut a = DataCentre::new("a");
        a.add_node(Node::new("same")).unwrap();
        let mut b = DataCentre::new("b");
        b.add_node(Node::new("same")).unwrap();
        assert!(n.add_centre(a).is_ok());
        assert!(n.add_centre(b).is_err());
    }
    #[test]
    fn relay_failover_adds_backup() {
        let mut n = AweNet::default();
        let mut a = DataCentre::new("a");
        a.add_node(Node::new("a1")).unwrap();
        a.add_node(Node::new("a2")).unwrap();
        let mut b = DataCentre::new("b");
        b.add_node(Node::new("b1")).unwrap();
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
    #[test]
    fn adding_nodes_creates_bidirectional_edges_and_rejects_duplicates() {
        let mut centre = DataCentre::new("dc");
        centre.add_node(Node::new("a")).unwrap();
        centre.add_node(Node::new("b")).unwrap();
        assert!(centre.nodes["a"].peers.contains("b"));
        assert!(centre.nodes["b"].peers.contains("a"));
        assert!(centre.add_node(Node::new("b")).is_err());
    }

    #[test]
    fn duplicate_centre_and_group_ids_never_replace_existing_records() {
        let mut network = AweNet::default();
        network.add_centre(DataCentre::new("dc")).unwrap();
        assert!(network.add_centre(DataCentre::new("dc")).is_err());
        assert_eq!(network.centres.len(), 1);

        for id in ["dc-1", "dc-2", "dc-3"] {
            network.add_centre(DataCentre::new(id)).unwrap();
        }
        let group = DataGroup::new("dg", ["dc-1", "dc-2", "dc-3"]).unwrap();
        network.add_data_group(group.clone()).unwrap();
        assert!(network.add_data_group(group).is_err());
        assert_eq!(network.data_groups.len(), 1);
    }

    #[test]
    fn data_group_and_centre_group_require_distinct_members() {
        assert!(DataGroup::new("dg", ["a", "a", "a"]).is_err());
        assert!(CentreGroup::new("cg", ["x", "x"]).is_err());
    }

    #[test]
    fn relay_failover_updates_both_directions_and_remains_valid() {
        let mut network = AweNet::default();
        let mut left = DataCentre::new("left");
        left.add_node(Node::new("left-1")).unwrap();
        left.add_node(Node::new("left-2")).unwrap();
        let mut right = DataCentre::new("right");
        right.add_node(Node::new("right-1")).unwrap();
        right.add_node(Node::new("right-2")).unwrap();
        network.add_centre(left).unwrap();
        network.add_centre(right).unwrap();
        network
            .connect_centres(
                "left",
                "right",
                CentreLinkMode::Relay {
                    relay_node: "left-1".into(),
                },
            )
            .unwrap();
        assert!(network.validate().is_ok());
        assert_eq!(
            network.centres["right"].link_state["left"].mode,
            CentreLinkMode::Relay {
                relay_node: "right-1".into()
            }
        );
        let new_relay = network.failover_relay("left", "right").unwrap();
        assert_eq!(new_relay, "left-2");
        assert_eq!(
            network.centres["left"].link_state["right"].mode,
            CentreLinkMode::Relay {
                relay_node: "left-2".into()
            }
        );
        assert_eq!(
            network.centres["right"].link_state["left"].mode,
            CentreLinkMode::Relay {
                relay_node: "right-1".into()
            }
        );
        assert!(network.validate().is_ok());
    }

    #[test]
    fn disconnect_and_remove_centre_clean_cross_centre_edges() {
        let mut network = AweNet::default();
        let mut left = DataCentre::new("left");
        left.add_node(Node::new("left-1")).unwrap();
        let mut right = DataCentre::new("right");
        right.add_node(Node::new("right-1")).unwrap();
        network.add_centre(left).unwrap();
        network.add_centre(right).unwrap();
        network
            .connect_centres("left", "right", CentreLinkMode::FullMesh)
            .unwrap();
        assert!(network.validate().is_ok());
        network.disconnect_centres("left", "right").unwrap();
        assert!(network.validate().is_ok());
        assert!(network.centres["left"].nodes["left-1"].peers.is_empty());
        network.remove_centre("right").unwrap();
        assert!(!network.centres.contains_key("right"));
        assert!(network.validate().is_ok());
    }

    #[test]
    fn data_group_removal_is_blocked_while_referenced_by_centre_group() {
        let mut network = AweNet::default();
        for id in ["dc-1", "dc-2", "dc-3", "dc-4", "dc-5", "dc-6"] {
            network.add_centre(DataCentre::new(id)).unwrap();
        }
        network
            .add_data_group(DataGroup::new("dg-1", ["dc-1", "dc-2", "dc-3"]).unwrap())
            .unwrap();
        network
            .add_data_group(DataGroup::new("dg-2", ["dc-4", "dc-5", "dc-6"]).unwrap())
            .unwrap();
        network
            .add_centre_group(CentreGroup::new("cg", ["dg-1", "dg-2"]).unwrap())
            .unwrap();
        assert!(network.remove_data_group("dg-1").is_err());
        network.remove_centre_group("cg").unwrap();
        network.remove_data_group("dg-1").unwrap();
        assert!(network.validate().is_ok());
    }

    #[test]
    fn topology_serializes_and_roundtrips() {
        let mut network = AweNet::default();
        let mut centre = DataCentre::new("dc");
        centre.add_node(Node::new("node")).unwrap();
        network.add_centre(centre).unwrap();
        let bytes = serde_json::to_vec(&network).unwrap();
        let restored: AweNet = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(restored, network);
        assert!(restored.validate().is_ok());
    }

    #[test]
    fn removing_a_full_mesh_node_cleans_membership_without_breaking_link() {
        let mut network = AweNet::default();
        let mut left = DataCentre::new("left");
        left.add_node(Node::new("left-1")).unwrap();
        left.add_node(Node::new("left-2")).unwrap();
        let mut right = DataCentre::new("right");
        right.add_node(Node::new("right-1")).unwrap();
        network.add_centre(left).unwrap();
        network.add_centre(right).unwrap();
        network
            .connect_centres("left", "right", CentreLinkMode::FullMesh)
            .unwrap();
        network.remove_node("left-1").unwrap();
        assert!(!network.centres["right"].nodes["right-1"]
            .peers
            .contains("left-1"));
        assert!(!network.centres["left"].link_state["right"]
            .relays
            .contains(&"left-1".to_string()));
        assert!(network.validate().is_ok());
    }

    #[test]
    fn route_rejects_unknown_nodes_and_works_in_both_directions() {
        let mut centre = DataCentre::new("dc");
        centre.add_node(Node::new("a")).unwrap();
        centre.add_node(Node::new("b")).unwrap();
        let mut network = AweNet::default();
        network.add_centre(centre).unwrap();
        assert!(network
            .route(&"missing".into(), &"missing".into())
            .is_none());
        assert_eq!(
            network.route(&"a".into(), &"b".into()),
            Some(vec!["a".into(), "b".into()])
        );
        assert_eq!(
            network.route(&"b".into(), &"a".into()),
            Some(vec!["b".into(), "a".into()])
        );
    }

    #[test]
    fn validate_rejects_asymmetric_peer_edges() {
        let mut centre = DataCentre::new("dc");
        centre.add_node(Node::new("a")).unwrap();
        centre.add_node(Node::new("b")).unwrap();
        centre.nodes.get_mut("b").unwrap().peers.remove("a");
        let mut network = AweNet::default();
        network.centres.insert("dc".into(), centre);
        assert!(network.validate().is_err());
    }
    #[test]
    fn future_peer_timestamps_are_not_healthy() {
        let metrics = PeerMetrics {
            reliability_percent: 100,
            last_seen_unix: 101,
            ..PeerMetrics::default()
        };
        assert!(!metrics.is_healthy(100, 60));
        assert!(metrics.is_healthy(101, 60));
        assert!(!metrics.is_healthy(200, 60));
    }

    #[test]
    fn network_management_updates_only_known_nodes() {
        let mut network = AweNet::default();
        let mut centre = DataCentre::new("dc");
        centre.add_node(Node::new("node")).unwrap();
        network.add_centre(centre).unwrap();

        network.set_node_health("node", false).unwrap();
        assert!(!network.centres["dc"].nodes["node"].alive);
        network.update_node_metrics("node", PeerMetrics {
            rtt_ms: 12,
            reliability_percent: 90,
            last_seen_unix: 100,
            ..PeerMetrics::default()
        }).unwrap();
        assert_eq!(network.centres["dc"].nodes["node"].metrics.rtt_ms, 12);
        assert!(network.set_node_health("missing", true).is_err());
        assert!(network.update_node_metrics("missing", PeerMetrics::default()).is_err());
    }

}
