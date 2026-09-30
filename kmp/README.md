# Spirit2 Kotlin Multiplatform

Kotlin Multiplatform bindings and demo applications for the sibling `../rust` workspace. Rust owns node transport, ticket payload validation, ticket expiry, replay protection, mesh membership, and five-second gossip heartbeats. `mesh` owns the portable pairing/session contract. Applications own UI, camera/scanner integration, node-directory selection, and their coroutine lifecycle.

## Gradle workspace

| Project | Role |
|---|---|
| `:mesh` | Pure Kotlin Multiplatform pairing API, `blue.rae.spirit:spirit-mesh`, targeting JVM 25, Android JVM 11, iOS arm64/simulator arm64, JS, and Wasm |
| `:sdk` | JVM/Android JNA SDK, `blue.rae.spirit:spirit-sdk`; exports `:mesh` with `api(project(":mesh"))` |
| `:demo:shared` | Compose Multiplatform UI and platform-neutral demo logic |
| `:demo:androidApp` | Android application |
| `:demo:desktopApp` | Desktop JVM application |
| `:demo:webApp` | JavaScript and Wasm web application |

The JNA-backed `:sdk` cannot be used from shared iOS, JS, or Wasm code. Shared application code should depend on `:mesh`; a platform host supplies a `MeshNode`, commonly by opening a `SpiritNode` on Android or JVM.

## Building

Run commands from this directory:

```text
direnv allow
./gradlew prepareNative --no-configuration-cache
./gradlew :mesh:jvmTest
./gradlew :sdk:jvmTest
./gradlew :demo:shared:jvmTest
./gradlew :demo:desktopApp:run
android-native
./gradlew :demo:androidApp:assembleDebug
```

The root `devenv.nix` supplies the Android SDK and NDK, Rust targets, JDK 25, Node, Yarn, Binaryen, and the Linux runtime libraries needed by Compose Desktop. It provides `jvm-test`, `unit-test`, `desktop`, `apk`, `install`, and `assemble` convenience commands.

Native preparation compiles the sibling Rust workspace before the consuming Gradle invocation. Native output remains under `../rust/target`; the JVM SDK package embeds the release native library as a JNA classpath resource, and Android consumes the JNI library and JNA AAR. `prepareNative` builds the host native artifacts and matching UniFFI bindings; `android-native` separately builds the Android ABIs.

## Device pairing and presence

The portable `:mesh` contract has `MeshNode.status(): NodeStatus` (identity, a list of groups with member IDs, names and generations, and deduplicated peer presence, plus native pending-ticket state), `createMesh(name): String`, `pair(): PairingInvitation`, `add(meshId, ticket): String`, `leaveMesh(meshId): LeftMesh`, `ping(device): NodePong`, and `shutdown()`. The JVM/Android `SpiritNode` implements it over UniFFI; it dispatches native operations off the main thread.

`MeshSession(nodeFactory, nowMillis)` owns one node during `run()`. Its
`files: StateFlow<MeshFiles?>` publishes the node's file contract on every open,
before the first poll, and clears it on close. AFM can observe it to register a
catalog handler and reapply share sets for each opening without retaining a
stale node. Nodes that do not implement `MeshFiles` publish null.

`MeshSession.run()` may be called only once per session. The host creates one session in its owner scope and cancels and joins its run job before closing other resources. Native operations and state publication already in progress finish before shutdown, so cancellation does not roll back enrollment. An open failure becomes state and ends `run()`; it is not an invitation to reopen the directory in the composable. It publishes `StateFlow<MeshState>` with `groups: List<GroupState>`, each containing its current members and their presence, a deduplicated `peers` list, the device's invitation and remaining seconds, `busy`, `error`, typed `failure: MeshFailure?`, and `notice`. Use `createGroup(name): String?` to identify the new group even when names collide; `addDevice(meshId, ticket): AddDeviceResult` distinguishes an admitted device from typed failures; `leaveGroup(meshId): LeftMesh?` returns the remaining/notified counts. Use `refreshTicket()` and `clearMessages()` to manage QR and notice/error/failure. `ping(deviceId)` and `reportError(message)` are available to the host. There is no automatic group creation.

An invitation is offered on initial status even when enrolled. Tickets are five-minute, single-use bearer credentials: anyone holding one can add the device to a group they choose. An invitation is withdrawn when native status says its ticket is no longer pending (even if readmission was into an existing group), when a first group is created locally, or when it expires. First-group creation and leaving withdraw the current ticket and offer one fresh ticket; subsequent group creation preserves the pending invitation. The QR countdown starts before native generation and uses monotonic time. Tickets are trimmed, limited to 8,192 UTF-8 bytes and checked for `spirit1` URL-safe syntax before native calls; the session rejects its own invitation and never includes a ticket in its error text. Rust validates payloads, expiry, self pairing and replay.

Actions are serialized rather than dropped when the UI is busy. A successful create or leave publishes its membership and notice before a separate cancellable ticket request; ticket failure preserves the notice and reports a ticket error. A remote enrollment observed by either an action or polling consumes the old invitation and requests at most one replacement per event. The FFI does not identify who admitted this device, so its notice reads `Joined <group>`. An action waiting for the lock can be cancelled; a native operation that already started completes and publishes its result before shutdown. The session polls status each second and ages authenticated ping/pong presence using the monotonic clock and the strict 60-second threshold, even when polling stalls. Never-seen peers remain offline. `connected` and `lastError` do not make a member online.

UniFFI errors map into `MeshNodeException.failure`: `Invalid`, `NodeClosed`, `NodeBusy`, `MeshLimit`, `NotMember`, `TicketRejected`, `Unavailable`, and fallback `Node`. `TicketRejected` includes remote refusal, expiry, replay, and self-enrollment; prompt for a fresh QR. `Unavailable` covers dial, relay, and request failures. The session gives the 64-group limit a specific message and never surfaces native error text containing tickets. A failed leave keeps its membership. Leaving reports fully, partially or unsuccessfully notified members; notified members relay the departure, and others can learn it when they next reach the departed device while it retains the copy. Post-leave ticket creation is cancellable when the node suspends; native `SpiritNode.pair()` runs on an IO dispatcher and offline teardown may still wait for its bounded request deadline (about 10 seconds). See [leaving and rejoining](../wiki/design/nodes.md#leaving-and-rejoining).

If the receiver is already in 64 groups, the introducer gets `TicketRejected("could not join this mesh")`; the receiver must leave a group before joining another.

### Demo and node directory lifecycle

Android keeps one `MeshSession` in its `AndroidViewModel` across configuration changes. The Kotlin SDK leases each node directory process-wide: `SpiritNode.open` waits for the previous owner to finish shutdown before opening the native lock, even when Android calls the finishing ViewModel’s `onCleared` after a new activity starts. A hung teardown fails the waiting open with typed `NodeBusy` after 15 seconds; an external holder of the native directory lock also reports `NodeBusy`. Desktop cancels and joins its node job before exiting its window. AFM can use `SpiritNode.open` directly without copying a demo handover.

The demo uses `MeshSession` for a group list, group creation, group-scoped ticket paste or QR scan, member presence, leave confirmation, and this device's QR on Android and desktop. Web and iOS have no native node. Desktop uses `~/.spirit2/ktdemo-node` with `SPIRIT_NODE_DIR` and `SPIRIT_NODE_NAME` overrides; `SPIRIT_LOCAL=1` enables loopback. Android calls `AndroidNodeContext.initialize(applicationContext)` before opening a node and stores its identity in non-backed-up app storage. The SDK needs internet and network-state permissions; camera permission belongs to the scanner app. There is no foreground service, so Android can suspend background networking.

## IntelliJ IDEA

Start IntelliJ from the project devenv so Gradle-run desktop applications inherit the Nix OpenGL runtime libraries:

```text
devenv shell -- idea .
```

Quit existing IntelliJ processes before using this command. The project-local IntelliJ settings use the Gradle wrapper and the devenv-provided Gradle JVM.

## File sharing and app-channel bindings

`MeshFiles` supplies suspend imports from paths or `MeshSource`, verified exports to paths or
`MeshSink`, per-mesh share-set replacement, cancellable fetch with progress, signed app operations,
app requests and synchronous app handlers. Android can pass content streams using `importStream`
and `exportToStream` in `:sdk`; neither requires a filesystem path. Node store operations block
the caller, so the SDK dispatches them to `Dispatchers.IO`, not Spirit's Tokio runtime workers.
App handlers run on a blocking native worker and must observe live `AppCallInfo.remainingMs` and
`isCancelled` and return promptly. Native fetches each use one OS thread for listener forwarding,
not Tokio workers; do not call `SpiritNode` synchronously from a listener. The SDK moves progress
into a child coroutine and joins it before returning. Slow consumers may miss progress updates,
but the completion result is always delivered. Closing the node during a fetch reports `Cancelled`,
not `NodeClosed`; calls starting after close report `NodeClosed`. Coroutine cancellation cancels and awaits
the native transfer, including cancellation while its start call is in flight. Adapters catch
`Exception`; a JVM `Error` instead becomes an opaque `Node` failure. `onQueued`
reports permit waiting separately from `onProgress`; a local blob hit reports one final
progress update without entering the transfer queue.
`verifyApp` also exists as a top-level SDK function and needs no open node; false means the
signature does not verify. `FakeMeshFiles` ships only in `:mesh-testing`; AFM must depend on this module in `commonTest`, never production. It uses a non-cryptographic 64-hex content digest (not BLAKE3) and deterministic `fake-unsigned:` tokens, **not cryptographic signatures**. Do not use the fake for authentication or production file persistence.
