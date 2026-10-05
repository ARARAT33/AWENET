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
