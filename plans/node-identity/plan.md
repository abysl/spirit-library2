# Private mesh CLI

## Goal

Implement persistent device identities, private mesh enrollment through QR-compatible tickets, automatic membership exchange, and authorized ping/pong. Any member can admit another device. No mobile app or blob transfer in this phase.

## Design

`spirit-node` owns identity, signed membership, tickets, and asynchronous iroh networking. `spirit-sdk` re-exports it behind the optional `node` feature. The CLI controls a running node through an authenticated loopback service.

A mesh is anchored by its founding device's public key. Each admission binds a device key and name to the mesh and is signed by an existing member. Peers validate the complete admission chain. The phone is an enrollment interface, not an online authority. Nodes gossip membership and addressing information to establish direct communication.

Tickets identify a listening device and contain a random single-use secret with an expiry. A member redeems a ticket to enroll that device into its mesh. The receiving device commits its membership before acknowledging admission. A device already in another mesh rejects enrollment. Tickets are invalidated by replacement, redemption, and restart.

The first version is append-only. Device removal, mesh switching, and admission restrictions require future protocol design.

## Tasks

- [x] Write identity, membership, three-node networking, and CLI acceptance tests before implementation.
- [x] Implement atomic persistent identities and signed membership verification.
- [x] Implement ticket enrollment, background membership exchange, authorized ping/pong, and graceful shutdown.
- [x] Implement `node init|id|serve|pair|ping` and `mesh create|add|status|members`, including terminal QR rendering and hostname-default nicknames.
- [x] Document operations, trust boundaries, persistence, limits, and recovery behavior.
- [x] Run workspace tests, formatting, clippy, and a three-process CLI smoke test.

## Acceptance criteria

- Identity and membership survive restarts; concurrent processes cannot overwrite a running node's state.
- New keys and local control credentials have owner-only permissions on Unix.
- Forged admissions, outsiders, wrong-mesh devices, invalid tickets, expired tickets, and consumed tickets fail.
- Any member can enroll another device; membership converges automatically.
- Two enrolled devices communicate with their introducer offline.
- QR codes encode the same usable ticket printed by the CLI.
- Network input sizes and operation durations are bounded.
- Existing blob commands continue to work.

## Review

Implementation and automated validation are complete. See [the review summary](review/implementation.md) for results and remaining manual checks.
