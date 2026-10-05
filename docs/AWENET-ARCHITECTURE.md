# AWENET architecture contract

AWENET is the network. Browser, Messenger, Calls, Groups, Files, Sites, Apps and Store are applications of the same network, not separate networks.

## Invariants

- **AWEID is the identity.** Human-readable names are aliases.
- **Resources have stable cryptographic IDs.** Owners may publish newer name bindings without changing the resource ID.
- **Every resource has placement.** Replication, discovery, repair and routing operate on the same resource identity.
- **Secret resources require authorization.** A name alone never grants access.
- **Open protocol.** Alternative desktop, mobile, server and embedded clients may join AWENET/1 if they validate the same wire limits, security rules and capability negotiation.
- **Local control.** Operators explicitly choose storage, bandwidth, relay and compute contributions.
- **Contribution is useful but not ownership.** Accountable contribution receipts can affect scheduling/priority. They are not a central currency and do not transfer ownership.
- **No impossible security promises.** The network uses authentication, encryption, replay protection, admission limits, bounded state and repair, but attacks can never be promised to be impossible.
- **Simple UX.** Everyday users get Browser, Messenger, Groups, Files and Store first; operators/developers get Node, Network, Security and Developer views.

## Layers

Applications -> application protocols -> resource/placement -> identity/access -> discovery/routing -> encrypted data plane -> transport/relay -> node resources.

## Freedom

AWEp2P is a reference implementation, not a lock-in requirement. A compatible implementation can replace its UI, storage engine, scheduler or internal services while remaining an AWENET node. The compatibility boundary is the versioned protocol plus signed identity/resource rules.

## Resource economics

A node can voluntarily offer storage, bandwidth, relay, compute and uptime. The scheduler can prefer healthy contributors and use contribution receipts to provide better priority while retaining a free baseline. Operators may reduce or withdraw their offer; replication and repair move data to other available nodes.


## ONECOIN monetary protocol

ONECOIN is the native economic unit of AWENET. It is not mined.

### Genesis and network-growth distribution

- The initial deterministic genesis set receives exactly **10 ONECOIN per member**.
- When a new member joins, exactly **1 ONECOIN** is allocated as a network-growth dividend and divided equally among all members after the join, including the new member.
- Balances use 18 decimal atomic units; no floating-point arithmetic is permitted.
- Any indivisible remainder is retained by the protocol as join-distribution remainder and is not silently assigned to a user.
- The join dividend stops permanently when the equal share is below one atomic unit; no zero-value distribution event is emitted.

### Contribution rewards

ONECOIN can also be minted as a reward for verified AWENET contribution. A reward must reference a measured ContributionReceipt; a node cannot mint coins merely by claiming capacity. The scheduler is responsible for determining an approved reward amount from verified work.

### Transfers and privacy boundary

Transfers are signed by the sender's AWEID key, use a per-account nonce to prevent replay/double-spend within the ledger state, and carry no human-readable username. AWEIDs are cryptographic identities; privacy-preserving transport and regulated exchange gateways are separate layers.

### Price policy

The reference implementation records a non-decreasing protocol floor. The floor starts at **$1** and rises by **$20** whenever a new **$100** price band is crossed. This is a protocol policy, not a promise that an external exchange will trade at that price. External exchange listing, fiat conversion and regulated services require separate market/liquidity and legal layers.

### AWESTORE and services

ONECOIN is intended to be usable by AWESTORE and future AWENET services as a native payment unit. Applications should consume the signed transaction/ledger primitives instead of inventing independent currencies.
