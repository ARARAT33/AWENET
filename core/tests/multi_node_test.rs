use awep2p_core::identity::{Identity, Username};
use awep2p_core::messenger::format_uid;
use awep2p_core::network::Node;
use awep2p_core::repair::execute_repairs;
use awep2p_core::replication::{assess, build_plan};
use awep2p_core::routing::{MultiHopRoutePlanner, PeerRoute};
use std::collections::BTreeSet;

#[tokio::test]
async fn test_multi_node_cluster_bootstrap_and_routing() {
    let id1 = Identity::generate(Username::new("node-1").unwrap());
    let id2 = Identity::generate(Username::new("node-2").unwrap());
    let id3 = Identity::generate(Username::new("node-3").unwrap());

    let node1 = Node::new(id1, "127.0.0.1:0".parse().unwrap());
    let node2 = Node::new(id2, "127.0.0.1:0".parse().unwrap());
    let node3 = Node::new(id3, "127.0.0.1:0".parse().unwrap());

    let n1_uid = format_uid(node1.identity.public.awe_id.as_bytes());
    let n2_uid = format_uid(node2.identity.public.awe_id.as_bytes());
    let n3_uid = format_uid(node3.identity.public.awe_id.as_bytes());

    let nodes_list = vec![n1_uid.clone(), n2_uid.clone(), n3_uid.clone()];
    let plan = build_plan([7u8; 32], &nodes_list).unwrap();
    assert_eq!(plan.placements.len(), 1000);

    let mut online_nodes = BTreeSet::new();
    online_nodes.insert(n1_uid.clone());
    online_nodes.insert(n2_uid.clone());

    let health = assess(&plan, &online_nodes);
    let repair_res = execute_repairs([7u8; 32], &plan, &health, &online_nodes);
    assert_eq!(repair_res.file_id, [7u8; 32]);

    let planner = MultiHopRoutePlanner::new(3);
    let peers = vec![
        PeerRoute {
            node_id: hex::encode(node1.identity.public.awe_id.as_bytes()),
            address: "127.0.0.1:41001".into(),
            distance: [0; 32],
            latency_ms: Some(5),
            healthy: true,
        },
        PeerRoute {
            node_id: hex::encode(node2.identity.public.awe_id.as_bytes()),
            address: "127.0.0.1:41002".into(),
            distance: [0; 32],
            latency_ms: Some(10),
            healthy: true,
        },
        PeerRoute {
            node_id: hex::encode(node3.identity.public.awe_id.as_bytes()),
            address: "127.0.0.1:41003".into(),
            distance: [0; 32],
            latency_ms: Some(15),
            healthy: true,
        },
    ];

    let route = planner
        .plan_route(node3.identity.public.awe_id.as_bytes(), &peers)
        .unwrap();
    assert!(!route.is_empty());
}

#[tokio::test]
async fn test_nat_stun_hole_punching_and_repair_worker() {
    use std::net::SocketAddr;
    let id_a = Identity::generate(Username::new("nat-a").unwrap());
    let id_b = Identity::generate(Username::new("nat-b").unwrap());
    let id_c = Identity::generate(Username::new("nat-c").unwrap());

    let node_a = Node::new(id_a, "127.0.0.1:0".parse().unwrap());
    let node_b = Node::new(id_b, "127.0.0.1:0".parse().unwrap());
    let node_c = Node::new(id_c, "127.0.0.1:0".parse().unwrap());

    let addr_a: SocketAddr = "127.0.0.1:41010".parse().unwrap();
    let stun_pkt = awep2p_core::network::StunHolePunchPacket::new(
        *node_a.identity.public.awe_id.as_bytes(),
        *node_b.identity.public.awe_id.as_bytes(),
        addr_a,
        awep2p_core::network::NatType::FullCone,
    );

    assert_eq!(stun_pkt.observed_addr, addr_a);
    assert_eq!(stun_pkt.nat_type, awep2p_core::network::NatType::FullCone);

    let worker = awep2p_core::repair::RepairWorker::new(10);
    let nodes_list = vec![
        format_uid(node_a.identity.public.awe_id.as_bytes()),
        format_uid(node_b.identity.public.awe_id.as_bytes()),
        format_uid(node_c.identity.public.awe_id.as_bytes()),
    ];
    let plan = build_plan([9u8; 32], &nodes_list).unwrap();
    let mut online = BTreeSet::new();
    online.insert(format_uid(node_a.identity.public.awe_id.as_bytes()));

    let health = assess(&plan, &online);
    let res = worker.process_manifest_repair([9u8; 32], &plan, &health, &online);
    assert_eq!(res.file_id, [9u8; 32]);
}
