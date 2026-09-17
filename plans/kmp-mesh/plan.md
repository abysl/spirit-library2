# KMP mesh controls and presence

## Goal

Add QR enrollment, manual ping, and a node list to the Android and desktop KMP demo. Every Rust node runs a heartbeat every five seconds by default. A peer is green after a valid received message within 60 seconds, red after an error or more than 60 seconds without a message. A new received message restores green. Never-seen members start red.

## Tasks

- [x] Test and implement monotonic, in-memory peer presence and independent five-second heartbeats in `spirit-node`.
- [x] Export node lifecycle, enrollment, QR data, presence, and ping through UniFFI and the Kotlin SDK.
- [x] Add a shared mesh panel, Android camera scanner, desktop QR-image import, ticket fallback, and platform lifecycle ownership.
- [x] Test Rust, generated bindings, Kotlin SDK and shared logic; compile desktop and Android; document platform and manual-test limits.

## Decisions

- Received messages include validated enrollment, membership exchange, ping, and pong; an unauthenticated or malformed message never establishes connectivity.
- Presence is runtime state, never persisted. Outbound failures mark the intended member disconnected; a later authenticated valid message restores connectivity.
- Heartbeats contact members independently so an unreachable device cannot delay checking the others. Work has a deadline and does not accumulate overlapping probes.
- The mobile scanner redeems the existing CLI ticket format. The shared panel can also display a ticket QR so another member can enroll the app.
- The app owns a persistent native node while open. Android keeps it across activity recreation using a ViewModel; desktop closes it on exit. Continuous Android background operation is not included.
- Web and iOS retain the current unavailable-native-SDK behavior, with UI code that compiles for those targets.

## Review

See [the implementation review](review/implementation.md) for validation and manual camera/network checks.
