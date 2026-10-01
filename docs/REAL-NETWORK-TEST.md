# AWEP2P Real Node-to-Node Test

This test is intended to be run by the operator on two machines or two terminals.

## 1. Build

~~~text
cargo build -p awe-node --release
~~~

## 2. Create identities

Terminal A:

~~~text
awe-node init ~/.awep2p/node-a.vault
~~~

Terminal B:

~~~text
awe-node init ~/.awep2p/node-b.vault
~~~

The command asks for a vault password. Keep each password available for the corresponding node.

## 3. Start Node A

~~~text
AWE_USERNAME=node-a awe-node run ~/.awep2p/node-a.vault '<PASSWORD-A>' 0.0.0.0:41000
~~~

On Windows PowerShell:

~~~powershell
$env:AWE_USERNAME="node-a"
awe-node run "$HOME/.awep2p/node-a.vault" "<PASSWORD-A>" "0.0.0.0:41000"
~~~

## 4. Start Node B and bootstrap to Node A

~~~text
AWE_USERNAME=node-b awe-node run ~/.awep2p/node-b.vault '<PASSWORD-B>' 0.0.0.0:41001 127.0.0.1:41000
~~~

Use the real reachable IP address of Node A when the nodes are on different machines.

## 5. Run the authenticated heartbeat probe

From a third terminal on Node B:

~~~text
awe-node probe 127.0.0.1:41000
~~~

Expected result includes:

- an authenticated peer AWE-ID;
- `Authenticated heartbeat: OK`;
- a measured round-trip time.

## 6. LAN test

For two machines on the same LAN, replace `127.0.0.1` with Node A's LAN address, for example `192.168.1.10:41000`.

Make sure the chosen TCP port is reachable through the host firewall.

## 7. What this proves

A successful probe proves a real TCP connection, signed identity handshake, ephemeral X25519 key agreement, authenticated encrypted session, and authenticated Ping/Pong heartbeat.

It does not by itself prove NAT traversal, Internet-wide routing, automatic relay failover, or a multi-Data-Centre deployment. Those require the corresponding multi-machine experiments.

## 8. Recommended multi-node experiment

Use at least three nodes:

- Node A: Data Centre 1
- Node B: Data Centre 1
- Node C: Data Centre 2

Verify:

1. A ↔ B direct connectivity.
2. B ↔ C connectivity.
3. A can discover B/C.
4. A heartbeat remains healthy while B is online.
5. Stop B and verify the expected failure is detected.
6. Repeat with C as the alternate path.

Record the exact addresses, timestamps, RTT values, failures, and recovery times for reproducibility.
