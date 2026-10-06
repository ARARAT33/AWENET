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

ONECOIN can also be minted as a reward for verified AWENET contribution. A reward must reference a measured ContributionReceipt signed by an explicitly authorized reward verifier; a node cannot mint coins merely by claiming capacity. The reward amount is deterministic from the network ContributionRewardPolicy and is capped per accounting period. The same signed receipt cannot be rewarded twice. Reward verifier authority is initialized explicitly at genesis and can be delegated only by an existing authorized verifier.

### Transfers and privacy boundary

Transfers are signed by the sender's AWEID key, use a per-account nonce to prevent replay within an ordered ledger state, and carry no human-readable username. AWEIDs are cryptographic identities; this is pseudonymous addressing, not absolute anonymity. Stronger privacy and regulated exchange gateways are separate layers.

### Price policy

The reference implementation records a non-decreasing protocol floor. The floor starts at **$1** and rises by **$20** whenever a new **$100** price band is crossed. This is a protocol policy, not a promise that an external exchange will trade at that price. External exchange listing, fiat conversion and regulated services require separate market/liquidity and legal layers.

### AWESTORE and services

ONECOIN is intended to be usable by AWESTORE and future AWENET services as a native payment unit. Applications should consume the signed transaction/ledger primitives instead of inventing independent currencies.


## Security and failure containment

The reference node now applies defense-in-depth at several independent boundaries:

- authenticated encrypted sessions with replay windows and sequence-bound AEAD;
- bounded frame, peer, routing, inbox, connection and discovery state to prevent memory amplification;
- pre-authentication IP admission plus authenticated-peer token buckets;
- per-peer quarantine/strike escalation with a bounded defense table, ending in a local ban after repeated protocol abuse;
- circuit-breaker primitives for expensive subsystems so repeated failures can temporarily isolate a workload;
- content-addressed storage integrity checks and atomic manifest writes so interrupted writes do not replace valid state with partial files;
- signed application manifests bind the developer AWEID to the developer public key and reject unsafe application IDs/path traversal;
- persistent ONECOIN state is written atomically and validated on reload, including member/AWEID consistency and supply invariants.

These controls are failure-containment mechanisms, not a claim of absolute immunity. A compatible implementation must preserve the same protocol invariants and must not silently weaken authentication, bounds, replay protection or resource accounting.

## Durable ONECOIN boundary

The ledger is no longer only an in-memory protocol object: PersistentOnecoinLedger provides an atomic on-disk state boundary and reload validation. Transfers are persisted only after the signed transaction passes membership, nonce, signature and balance checks. Distributed ordering/consensus is still a separate network layer; local persistence alone cannot solve Byzantine double-spending across independent nodes.


## Native payments and monetary governance

AWESTORE and AWENET services can use `OnecoinPaymentRequest` instead of inventing application-specific payment formats. A signed invoice binds seller AWEID, buyer AWEID, resource ID, amount and expiry; the buyer's normal nonce-protected ONECOIN transaction must match that invoice before settlement.

A price-floor reduction is not unilateral. `PriceFloorGovernance` requires a verified supermajority of eligible network members (at least 66.67% by protocol basis points) for an explicit reduction proposal. Normal price-band increases remain monotonic and do not require a vote.


## ONECOIN distributed finality

A local ONECOIN ledger is not treated as network truth. Finalized transfers are grouped into canonical blocks ordered by `(sender, nonce, transaction-id)`, chained by the previous finalized block hash, signed by a validator proposer, and accepted only with a verified validator quorum of at least 66.67%. Nodes execute a candidate block against a cloned ledger before committing it, so a failed transaction cannot partially mutate finalized state.

`OnecoinFinalizedState` is the deterministic state machine boundary. `PersistentOnecoinState` stores that finalized state atomically so a restart does not silently reset the consensus height or tip. This still requires the live network to agree on the validator set and transport proposals/votes; those are network-control-plane responsibilities, not a reason to fall back to unsafe local acceptance.
