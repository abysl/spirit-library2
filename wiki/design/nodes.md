# Private device meshes

## Identity and membership

Each device has an Ed25519 keypair. Its public key is its iroh endpoint ID. A nickname defaults to the hostname at initialization and is stored with the identity. Nicknames are display and lookup names, not authentication credentials. Duplicate nicknames are allowed; a CLI operation targeting an ambiguous nickname requires a full device ID.

A new mesh receives a cryptographically random 32-byte `MeshId`, independent of every device key. Its canonical text form is `mesh1_` followed by unpadded URL-safe base64. The founder is recorded separately and signs its initial admission; every member can sign admissions for additional devices without the founder remaining online. The signed bytes are the Postcard encoding of the tuple `("spirit/mesh/admission/2", mesh_id_bytes, founder_id, mesh_name, member, issuer)` in that order. The signature is Ed25519, encoded with unpadded URL-safe base64. Mesh-level authorization still requires validated membership evidence: possession of a mesh ID is not a credential, and this change does not introduce roles or administrative privileges.

Existing meshes retain their original founder-key IDs and version-1 admission signatures, with no `founder` field. Their signed tuple remains `("spirit/mesh/admission/1", mesh_id, mesh_name, member, issuer)`. They remain readable and can admit more members; they are not silently migrated because changing an ID would invalidate signed membership on other devices. New-format meshes require upgraded clients. Older clients reject their IDs and admissions; the unchanged pairing and sync ALPNs do not negotiate a downgrade. Rebuild native libraries together with their UniFFI bindings when upgrading.

A received membership snapshot is accepted only if every signature verifies and every issuer can be traced to the self-signed founding admission. This graph is verified without relying on the ordering of records. Duplicate admissions for the same `(device, generation)`, duplicate departures, and conflicting nicknames for the same `(device, generation)` are rejected. Existing local records are retained when merging, making independent enrollments and departures converge by `(device, generation)`. Merges must agree on mesh ID, mesh name, and founder; matching display names alone never join independent meshes.

A member proves ownership of its admitted key through the iroh connection. Nicknames, network addresses, and the presence of a node ID in an unsigned list cannot authorize a pong.

## Enrollment

The device being added runs `node pair`. It generates a random 32-byte secret, records an expiry in memory, and issues a `spirit1` ticket. The ticket contains its endpoint address, nickname, secret, and expiry, serialized with Postcard and encoded with unpadded URL-safe base64. The CLI renders those exact ticket bytes as a QR code. `--qr-svg` saves a scalable image; `--no-qr` prints only the ticket.

An existing member runs `mesh add <ticket>`. It connects to the ticket's authenticated endpoint, signs an admission for that device, and sends the secret, membership evidence, and address hints over the encrypted connection. The receiving device verifies the secret, its own recorded expiry, the introducer's membership, its own admission, and compatibility with any existing mesh. It atomically saves membership before acknowledging and consumes the ticket.

A ticket expires after five minutes by default. Its lifetime can be set between 1 and 3600 seconds. Creating another ticket invalidates the previous ticket. Restarting invalidates every outstanding ticket. Failed attempts with an incorrect secret or a different mesh do not consume the valid invitation. Replaying a consumed ticket fails. Re-enrolling the same device with a fresh ticket into the same mesh is harmless.

The ticket is a bearer credential: possessing it authorizes its holder to enroll the issuing device into the holder's mesh. Keep the QR image and text private until enrollment completes. It contains an enrollment secret, never a device's long-term private key.

If the final acknowledgment is lost, the receiver may already have committed its membership. Its subsequent membership exchange repairs the introducer's view. Inspect `mesh members` before generating a fresh ticket and retrying.

## Leaving and rejoining

A member leaves by signing a departure record for its own current admission. The signed bytes are the Postcard encoding of `("spirit/mesh/departure/1", mesh_id, founder, mesh_name, member_id, generation)`, where `mesh_id` is the canonical text ID and `founder` is optional for legacy meshes. Only the departing key can sign its departure; another member cannot remove a device. Departures merge like admissions: every copy of the mesh converges on the union of both record sets, so a stale snapshot cannot return a departed device to the member list.

A device is a current member when its highest-generation admission has no matching departure. Current members are the only devices listed, pinged, sent heartbeats, answered with pongs, or allowed to admit devices. Departed admissions remain in the signed history because later admissions may trace their trust through them. Consequently the founder, or any introducer, may leave without invalidating the devices it admitted. Departure is a cooperative membership change, not key revocation: a departed key that is later compromised can still sign records that other members accept, and removing a device against its will requires a future policy.

Leaving atomically clears the local mesh, its routing addresses, presence state, and any outstanding pairing ticket. When other current members remain, it retains a copy of the departed mesh in a map keyed by mesh ID. At most 64 copies are retained; the oldest recorded departure is evicted when a new one exceeds that bound. This limits pull delivery to retained meshes: members of an evicted mesh must learn the departure from a member who received it earlier. The running node announces that copy to every former member over `spirit/depart/1` in parallel, bounded by one request deadline, and reports how many recorded it. One notified member suffices for membership exchange to propagate the departure. `Node::leave_mesh` leaves while stopped and notifies nobody immediately. The retained copy answers later membership exchanges from former members, so they learn of the departure when they next reach the device while it is running.

A departed device can be enrolled again with a fresh ticket. The introducer signs a readmission with the next generation, using `("spirit/mesh/readmission/1", mesh_id, founder, mesh_name, member, issuer, generation)`; a readmission is valid only after a departure from the previous generation. If the introducer has not yet learned of the departure, the receiving device refuses the stale enrollment and returns its departure record without consuming the ticket. The introducer records it and retries once with a readmission. Rejoining the same mesh discards that mesh's retained departure copy; joining a different mesh keeps the old copy for its members. The departed device still receives full membership snapshots from stale members' syncs and stale introducers' enrollment attempts, even though it does not persist those snapshots as current membership or rejoin on those attempts.

Clients from before departures ignore them, so they keep listing a departed device until they upgrade. They reject readmission signatures, so a mixed-version mesh stops exchanging membership with older clients after a device rejoins.

## Networking

`spirit-node` owns the iroh endpoint and router. It serves four versioned protocols:

| ALPN | Request | Response | Authorization |
|---|---|---|---|
| `spirit/pair/1` | Enrollment secret and membership snapshot | Committed snapshot, error, or refusal with departed copy | Active ticket and introducer's valid membership |
| `spirit/mesh/1` | Membership and address snapshot | Merged snapshot or retained departed copy | Peer has verifiable membership in the same mesh |
| `spirit/ping/1` | JSON string `"ping"` | JSON string `"pong"` | Authenticated peer is in the local validated membership set |
| `spirit/depart/1` | Departed mesh copy without addresses | JSON string `"recorded"` | The authenticated peer's own signed departure, for the local mesh |

Each exchange uses one bidirectional QUIC stream, with EOF delimiting the JSON message. Unknown peers may perform a transport handshake and submit pairing or membership evidence; they receive no pong without membership. A stale introducer's pairing refusal includes the departed mesh copy, which the introducer records before retrying. A departed device's sync reply also includes its retained departed copy.

Background heartbeats run every five seconds. Each peer is checked independently, with at most one active heartbeat per member and a four-second overall deadline. An unreachable peer cannot delay probing the others. Each heartbeat exchanges membership and then sends a ping. A member can present a signed admission unknown to its peer; this teaches the peer about the new member before a subsequent ping. The CLI performs a membership exchange before pinging.

Addresses are routing hints. A newly discovered member's address can be learned from another member. Afterward, that member's own authenticated connection updates its address. Forwarded stale snapshots do not overwrite a directly learned address. iroh still authenticates the intended device key regardless of the address used.

Normal operation uses iroh's N0 relay and address lookup preset. Ticket generation waits for relay readiness so the ticket includes a usable relay address. `node serve --local` instead binds loopback with no relay or external discovery, for same-machine tests. This mode does not connect separate machines.

Connection and exchange stages each have a ten-second deadline, or three seconds in local mode. Messages are limited to 256 KiB; ping requests to 16 bytes. A mesh holds at most 256 admission records, including readmissions, with at most 16 transport addresses per member. Leave and rejoin cycles consume the admission cap permanently. Two individually valid snapshots whose union exceeds it fail to merge in either direction with `invalid mesh size`; same-generation readmissions with different signed nicknames permanently fail to merge with `conflicting device nickname`. Neither conflict is resolved automatically. Names are limited to 128 UTF-8 bytes and cannot contain control characters.

Membership propagation is eventual. Peers must have exchanged membership and usable addresses before the introducer goes offline. Once they have, the introducer need not remain online for authentication, pinging, or further enrollment.

## Persistence and local control

The node directory defaults to `~/.spirit2/node`, overridden with `--node-dir` or `SPIRIT_NODE_DIR`. Blob storage continues to use its separate `--store` setting.

| File | Contents |
|---|---|
| `secret.key` | Raw 32-byte secret key |
| `state.json` | Local identity, nickname, signed mesh admissions and departures, learned addresses, and up to 64 departed mesh copies with their recording order |
| `node.lock` | Process ownership lock |
| `control.json` | Running node's loopback control address and random credential |

Writes use temporary files and atomic replacement. New secret, state, control, and exported QR files are owner-only on Unix. Malformed or missing existing identities fail rather than silently generating a replacement. A process lock prevents a second service or offline mutation from overwriting the running node's state.

The CLI contacts the service through a loopback TCP listener authenticated by a random 32-byte credential. Control requests use length-prefixed JSON with a 256 KiB limit and a 30-second deadline. At most 32 control connections are handled concurrently. Public iroh connections cannot use this interface.

`node serve` handles Ctrl-C and SIGTERM, cancels control tasks, removes the control file, and shuts down iroh. A process crash can leave a stale control file; restarting replaces it after acquiring the node lock. Initialization, mesh creation, and leaving work offline. Status and membership can be read while stopped. Enrollment, pairing, and ping require a running service.

## Rust API

Consumers enable the `node` feature on `spirit-sdk`. Blob-only Rust SDK consumers do not enable the networking dependency. The FFI crate enables it to provide KMP node bindings. Public types include `Node`, `NodeConfig`, `NodeId`, `MeshId`, `NodeInfo`, `Member`, `LeftMesh`, and `Pong`. `NodeInfo.mesh_id` is an optional `MeshId`, not a `NodeId`; `mesh status` prints it separately from the mesh name.

- `Node::init(directory, nickname)` creates or reopens an identity.
- `Node::create_mesh(directory, mesh_name)` creates a mesh while stopped.
- `Node::bind(directory, config).await` starts networking and background exchange.
- `Node::leave_mesh(directory)` leaves while stopped; former members learn of it from the running node later.
- `node.new_mesh(mesh_name)` creates a mesh while running.
- `node.pair(lifetime).await` opens an enrollment window and returns a ticket.
- `node.add(ticket).await` admits the ticket's device.
- `node.leave().await` leaves the mesh, notifies reachable former members, and returns a `LeftMesh` with the remaining and notified member counts.
- `node.info().resolve(nickname_or_id)` resolves a member without guessing on duplicates.
- `node.peers()` returns each other member's connectivity, time since last received message, and last error.
- `node.ping(member.id).await` returns an authenticated pong and elapsed milliseconds.
- `node.shutdown().await` stops networking. Drop the node to release ownership of its directory.

`Node::read_info(directory)` reads committed state without starting networking. `NodeConfig::default()` uses normal iroh services; `NodeConfig::local()` uses loopback. The SDK also exports the restricted-permission atomic file writer used by the CLI for control and QR files.

## Deferred operations

This version supports one mesh at a time per device. Nicknames are fixed at initialization. Removing another device, key revocation, renaming, merging meshes, and restricting admission require new signed update and policy rules. Blob transfer remains outside this phase. Kotlin node bindings and Android/desktop pairing controls are available through the KMP app.

The protocol is an initial version intended for small personal meshes. Its automatic exchanges favor straightforward convergence over large-network efficiency.

## References

- [iroh endpoint configuration](https://docs.rs/iroh/1.2.0/iroh/endpoint/struct.Builder.html)
- [iroh router lifecycle](https://docs.rs/iroh/1.2.0/iroh/protocol/struct.Router.html)
- [QR rendering](https://docs.rs/qrcode/0.14.1/qrcode/render/index.html)

## Connectivity and KMP integration

Connectivity is in-memory state measured using a monotonic clock. Only a validated heartbeat message marks a member as received. A green peer has received an authenticated valid ping or pong from that member less than 60 seconds ago. A transport, protocol, or heartbeat error is retained as a diagnostic without changing green status; a subsequent valid ping or pong clears it. Never-seen peers start red. Restarting clears presence without removing memberships.

Membership snapshots and enrollment messages do not count. Unauthenticated traffic, malformed messages, and sent requests do not make a device connected. Heartbeat timeouts are recorded as diagnostics, and pending probes are canceled before the next interval. The regular request deadlines still apply to manual operations.

`spirit-ffi` runs nodes on a shared Tokio runtime and exposes synchronous operations that the Kotlin SDK dispatches to IO threads. Its `SpiritNode` object supports explicit shutdown; dropping an unclosed handle also schedules shutdown. Status maps into records with string IDs so the KMP UI does not need iroh types. `MeshStatus.mesh_id`, `NodeStatus.meshId`, and `PairingState.meshId` expose mesh identity separately from device identity. Pairing returns the original ticket plus QR modules generated from those exact bytes. The FFI pairing window is fixed at 300 seconds; the CLI alone offers a configurable ticket lifetime, keeping the embedded contract minimal.

The Android JNI initialization holds the application context in a global reference for the process lifetime and installs it once before constructing an iroh endpoint. This lifetime is required by iroh's DNS integration; a temporary activity reference must never be substituted. Device identity is stored in Android's non-backup directory so OS backup restoration does not duplicate an endpoint identity onto another phone.

The KMP list polls status every second. It displays both a colored dot and textual state, keeps disconnected members visible, and shows a short ID when nicknames collide. Manual ping targets the member's full ID even though the interface displays its nickname. Camera permission denial leaves ticket pasting available.
