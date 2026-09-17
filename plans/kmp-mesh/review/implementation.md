# KMP pairing and presence review

## Changes

- Every running Rust node performs independent five-second heartbeats, with no overlapping probe for a member and a four-second heartbeat deadline.
- Presence records valid received messages using a monotonic clock. Errors disconnect immediately when detected; more than 60 seconds of silence or no prior message also means disconnected. A new valid received message restores connectivity.
- Added native node lifecycle, status, pairing QR modules, admission, and ping through UniFFI and a suspending Kotlin SDK wrapper.
- Added Android JNI setup for system DNS, retained application-context lifetime, and non-backed-up identity storage.
- Added the Devices tab with mesh creation, QR display, ticket entry, per-member ping, nickname display, accessible green/red indicators, and one-second presence refresh.
- Android uses a camera scanner with permission handling; desktop imports PNG/JPEG QR images. Both can paste tickets.
- Android retains its node through activity recreation; desktop shuts it down on exit. The existing blob demo remains available in the Blobs tab.
- Aligned the Android app's NDK version with the development environment for native packaging.

## Verification

- Rust workspace tests passed: 33 tests, including monotonic 60-second expiry, immediate errors and recovery, automatic heartbeats through restart, and native binding lifecycle.
- Rust formatting and workspace clippy passed, with the same pre-existing `chunks_exact_to_as_chunks` lint excluded in the blob parser.
- Kotlin SDK tests passed: five tests covering the blob API and native enrollment, ping, presence, close, and reopening persisted membership.
- Shared JVM tests passed: eight tests including connection labels, the shared adapter, QR-image decoding, and a full native QR render/decode/redeem/ping round trip.
- Android and desktop Kotlin compilation, JS and Wasm shared compilation, Android arm64/x86_64 native libraries, and debug APK packaging were exercised.
- A desktop UI ran against a CLI peer in an isolated Xvfb display. Screenshots verified a green Connected row while the peer was running and a red Disconnected row after it stopped. Temporary nodes and the display were shut down afterward.

## Manual checks and limits

- On an Android phone, grant camera permission, scan a CLI ticket, and ping the enrolled device. Also deny permission and verify ticket pasting remains available.
- Rotate the phone and launch/return from the scanner; identity and node ownership should remain intact through the ViewModel.
- Test across separate networks to exercise actual mobile NAT and relay behavior. Automated network tests used loopback.
- No physical camera or Android device was exercised in this environment. The native and Kotlin Android builds validate integration but do not substitute for a device run.
- Android background networking is best effort while its process remains alive. A foreground service is not included.
- iOS and web still lack the native SDK; their shared UI compiles with the existing unavailable-native-feature behavior. An iOS build cannot be run on this Linux host.
- Membership remains append-only. Device removal, admission policy restrictions, nickname changes, and blob transfer are separate work.
