# Trust lifecycle — 2026-11-01

AWENET now exposes a deterministic, bounded trust lifecycle in
`core/src/trust.rs`. Trust is an admission and routing signal only; it never
creates administrative authority and never replaces cryptographic identity or
transport authentication.

## States and transitions

- `unknown`: the initial neutral state, starting at score 500.
- `trusted`: score is at least 700.
- `probation`: score is below 300 but above the quarantine threshold.
- `quarantined`: score is at or below 100, or abuse evidence is observed.
- `banned`: an explicit operator action; normal observations cannot modify it.

A successful observation adds 10 points, a failed observation removes 50
points, and an abuse observation removes 200 points and places the peer in
quarantine. Scores are clamped to 0–1000 and all counters are saturating.

Bans are sticky. Reinstatement is explicit and requires a quarantined peer to
have a score of at least 700. The operator action clears the recorded abuse
evidence and promotes the peer to `trusted`; automated scoring can never
silently undo a quarantine.

## Persistence and bounds

The lifecycle stores at most 4,096 peers. State serializes to JSON using a
stable hexadecimal peer-id key and includes the current state, score, evidence
counters, and last tick. Time is supplied by the caller as an explicit tick;
the module does not spawn threads or read a wall clock.

The module is currently a core library contract. It is not yet wired to a new
HTTP configuration endpoint or a UI control surface; such wiring should define
an authenticated operator capability and an audit/event persistence contract
before exposure.
