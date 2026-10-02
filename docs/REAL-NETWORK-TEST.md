# AWEP2P Real Node-to-Node Test

This procedure validates the direct authenticated TCP path only. It does not prove DHT, multi-hop routing, NAT traversal, or distributed storage.

## Prerequisites
- Compatible Rust toolchain and two independent node processes (two machines preferred).
- Reachable TCP port and appropriate firewall configuration.
- Use test identities/passwords; never publish credentials or private vaults.

## Build
```sh
cargo build -p awe-node --release
```

## Create identities
Run once per node, using distinct vault paths:
```sh
awe-node init <path-to-node-vault>
```

## Start node A
```sh
AWE_USERNAME=node-a awe-node run <node-a-vault> <password-a> 0.0.0.0:41000
```

PowerShell:
```powershell
$env:AWE_USERNAME="node-a"
awe-node run "$HOME/.awep2p/node-a.vault" "<PASSWORD-A>" "0.0.0.0:41000"
```

## Start node B and connect to A
```sh
AWE_USERNAME=node-b awe-node run <node-b-vault> <password-b> 0.0.0.0:41001 <node-a-reachable-address>:41000
```
Use `127.0.0.1:41000` for a same-machine test; use A's reachable address across machines.

## Probe
```sh
awe-node probe <node-a-reachable-address>:41000
```
Record revision, OS, addresses, time, and output with secrets removed. Confirm the current command implementation before interpreting exactly what the probe proves.

## Scope and failure cases
A successful probe demonstrates only the direct connection and handshake/heartbeat operations actually performed. It does not prove Internet-wide discovery, multi-hop forwarding, relay failover, NAT traversal, remote shard persistence/retrieval/repair, anonymity, or production release quality.

Also test abrupt shutdown, restart, unreachable peers, malformed frames, duplicate connections, and timeouts. Record expected versus actual behavior and attach reproducible evidence.