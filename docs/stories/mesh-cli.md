# Private mesh CLI user story

As a user, I create a private mesh on one device, enroll my other devices by exchanging pairing tickets, and contact them by nickname. Any member can enroll another device. An introducer can go offline after membership has propagated.

## Build

From `rust/`:

```bash
cargo build --release -p spirit-cli
```

The binary is `target/release/spirit`. The commands below assume it is on your PATH.

## First device

```bash
spirit node init
spirit mesh create --name personal
spirit node serve
```

The nickname defaults to the hostname. Use `spirit node init --name desktop` to choose one when initializing. Re-running initialization preserves the existing identity and nickname.

The service runs in the foreground. Keep it running and execute other commands in another terminal. Stop it with Ctrl-C.

## Enroll another device

On the new device:

```bash
spirit node init --name laptop
spirit node serve
```

In another terminal on that device:

```bash
spirit node pair
```

The CLI displays a terminal QR code and its text ticket. It expires in five minutes and can be used once. For a scalable QR image:

```bash
spirit node pair --qr-svg pairing.svg
```

This generates a new ticket and invalidates the previous one. Keep the QR image private while the ticket is active.

On an existing mesh member, copy the printed ticket into:

```bash
spirit mesh add '<ticket>'
```

Expected output:

```text
Added laptop to personal.
```

The new device joins the existing device's mesh. It does not create a separate mesh. The Android app can scan the same QR payload; this walkthrough uses the CLI to redeem it.

For scripts, print only the ticket:

```bash
spirit node pair --no-qr --ttl-seconds 60
```

## Inspect and communicate

```bash
spirit mesh status
spirit mesh members
spirit node ping laptop
```

Example output:

```text
Device: desktop
Node: running
Mesh: personal
Members: 2
```

```text
NICKNAME
desktop (this device)
laptop
```

```text
pong from laptop in 12 ms
```

Names are exact and case-sensitive. With duplicate nicknames, list full device IDs and use the intended ID:

```bash
spirit mesh members --ids
spirit node id
spirit node ping '<device-id>'
```

`node id` always prints the local public ID. Membership listings show known members, not live presence; use `node ping` to check reachability. Restarting preserves both identity and membership.

## Leave a mesh

```bash
spirit mesh leave
```

Expected output while the service is running and every other member is reachable:

```text
Left personal and notified its 2 remaining members.
```

The device keeps its identity and nickname but is no longer a member: it stops heartbeats, other members stop listing and pinging it, and outstanding pairing tickets are invalidated. Leaving also works while the service is stopped. Unreached members learn of the departure from any notified member, or from this device when they next reach it while it is serving and still retains the departed mesh copy. The device retains at most 64 departed meshes, evicting the oldest departure after 64 later departures. After eviction, this device can no longer deliver that departure by pull or refuse a stale introducer's enrollment. Without another member relaying the departure, the stale introducer's generation-0 enrollment rejoins the device on its side only, while informed members still reject it. To recover, leave again, then be readmitted.

To return, generate a fresh ticket on the device and redeem it from a remaining member with `mesh add`, as for a new device. It rejoins the same mesh with its original device ID. The device can instead create or join a different mesh. Leaving does not revoke its key, and there is no command to remove a different device.

## Try multiple devices on one computer

Each simulated device needs its own directory. These example paths are relative to the current working directory; keep them outside a source checkout because they contain private keys.

```bash
spirit --node-dir ./desktop node init --name desktop
spirit --node-dir ./desktop mesh create --name personal
spirit --node-dir ./desktop node serve --local
```

In another terminal:

```bash
spirit --node-dir ./laptop node init --name laptop
spirit --node-dir ./laptop node serve --local
```

In a third terminal:

```bash
ticket=$(spirit --node-dir ./laptop node pair --no-qr)
spirit --node-dir ./desktop mesh add "$ticket"
spirit --node-dir ./desktop node ping laptop
```

For separate machines, omit `--local` so iroh can use its relay and discovery services. The default node directory is `~/.spirit2/node`; `SPIRIT_NODE_DIR` is another way to override it.

## Expected failures

- A nonmember receives no pong, including when it bypasses the CLI and connects directly.
- Expired, replaced, malformed, and consumed tickets fail.
- A device already belonging to another mesh rejects enrollment.
- A fresh ticket for an existing member of the same mesh is safe to redeem again.
- An ambiguous nickname requires a device ID.
- Pairing, enrollment, and ping report that the node must be running when it is stopped.
- A second service using the same node directory fails without disturbing the first.
- Leaving fails when the device is not a mesh member. A device that left rejects pings, and members no longer ping it.

If enrollment loses its final acknowledgment, check `mesh members`: the admitted device may already have joined and will exchange its membership with the introducer. Restarting invalidates unconsumed tickets, so generate a fresh ticket if needed.

## Acceptance tests

`rust/crates/cli/tests/node.rs` runs separate CLI services, exercises hostname defaults, terminal/SVG QR generation, nickname resolution, duplicate names, crash recovery, and communication with the introducer offline. `rust/crates/node/tests/mesh.rs` covers persistent identities, enrollment, transitive admission, restart, and ticket behavior. Library tests also send unauthorized requests directly over iroh and verify signed admission rejection.

Blob storage commands continue to use their existing store. The CLI suite also leaves and rejoins a mesh while serving and leaves while stopped. Blob transfer, removing another device, and admission restrictions are future stories.

See [the node design](../../wiki/design/nodes.md) for the trust model and protocol.

## Pair with the KMP app

The Android app can scan the QR emitted by `node pair`, and the desktop app can import a PNG/JPEG screenshot of it. Create a mesh in the app, then scan or paste the CLI device's ticket to enroll it. An existing CLI mesh can instead redeem the app's displayed ticket with `mesh add`.

Every running node now sends heartbeats every five seconds. The app shows members as green after receiving an authenticated valid ping or pong from that member less than 60 seconds ago, and red when no heartbeat has been received or the heartbeat is at least 60 seconds old. Errors remain diagnostics and do not turn a green member red. See the [KMP pairing guide](../../kmp/README.md#device-pairing-and-presence).
