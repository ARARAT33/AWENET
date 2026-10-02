# AWEp2P Product Architecture

AWEp2P keeps the full decentralized architecture: identity, secure sessions, discovery, routing, distributed storage, repair, names/sites, Store, Messenger, desktop clients and platform integration.

The simplification is inside the architecture, not a reduction of the product. Each subsystem has one responsibility and the product layer provides one stable model for the end-user node.

## Runtime

Desktop / CLI -> Product orchestration -> Identity/security, discovery, routing, authenticated transport, content-addressed storage, chunking/replication, repair/health, namespace/sites, Store and Messenger -> other AWE nodes.

The public product model is in core/src/product.rs. It gives the existing subsystems shared concepts for resources, peers, placement, transfers, capacity and product snapshots.

## Resource lifecycle
1. Create a content-addressed ResourceId.
2. Create a manifest describing size and chunks.
3. Select distinct healthy peers according to placement policy.
4. Send encrypted chunks over authenticated transport.
5. Verify content before durable placement.
6. Record placement only after verification.
7. Repair missing replicas after failure.
8. Retrieve and verify the resource by content identifier.

## Product rules
- No central database is required for node-to-node data transfer.
- Private data is encrypted before leaving its owner's device.
- Resource identity is derived from content, not a mutable filename.
- Replicas are counted by distinct node IDs.
- Transfers have explicit lifecycle states.
- A local build is not evidence of distributed interoperability.
- A UI screen is not complete until it drives a real product path.

## Release acceptance
Production release requires evidence for clean installation, two independent nodes, authenticated connection/reconnect, discovery, bounded routing, remote upload/download, durable verification, replica repair, namespace/site retrieval, Store verification, Messenger delivery, terminal-free desktop launch, reproducible artifacts and clean-machine installation.