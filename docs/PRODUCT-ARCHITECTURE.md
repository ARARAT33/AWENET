# AWEp2P Product Architecture

AWEp2P keeps the full decentralized architecture: identity, secure sessions, discovery, routing, distributed storage, repair, names/sites, Store, Messenger, desktop clients and platform integration.

The simplification is inside the architecture, not a reduction of the product. Each subsystem has one responsibility and the product layer provides one stable model for the end-user node.

## Runtime

Desktop / CLI -> Product orchestration -> Identity/security, discovery, routing, authenticated transport, content-addressed storage, chunking/replication, repair/health, namespace/sites, Store and Messenger -> other AWE nodes.

The public product model is in core/src/product.rs. It gives the existing subsystems shared concepts for resources, peers, placement, transfers, capacity and product snapshots.

## Founder control plane

The Founder Authority is the network control plane and is governed by the canonical [Founder Authority specification](FOUNDER-AUTHORITY.md). It is separate from node ownership and resource contribution. AWENET's one founder identity, `AWENeter`, is established by signed Genesis and acts only through approved founder devices and device-bound sessions.

Founder-only operations must pass shared cryptographic authorization and audit enforcement; hiding a button or checking a UI role is not authorization. Neters may perform only founder-delegated bounded tasks. Members and Messenger groups can advise but cannot vote away the founder. Founder authority does not override user privacy, secret-resource access maps, or local node operators' right to stop resource contribution.

The product runtime must expose public read-only status separately from authenticated founder administration. The founder can govern without hosting an always-online server.

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