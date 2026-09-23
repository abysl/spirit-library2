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
generate-bindings
jvm-native
./gradlew :mesh:jvmTest
./gradlew :sdk:jvmTest
./gradlew :demo:shared:jvmTest
./gradlew :demo:desktopApp:run
./gradlew :demo:androidApp:assembleDebug
```

The root `devenv.nix` supplies the Android SDK and NDK, Rust targets, JDK 25, Node, Yarn, Binaryen, and the Linux runtime libraries needed by Compose Desktop. It provides `jvm-test`, `unit-test`, `desktop`, `apk`, `install`, and `assemble` convenience commands.

`generate-bindings` and the native build commands compile the sibling Rust workspace. Native output remains under `../rust/target`; the JVM SDK package embeds the release native library as a JNA classpath resource, and Android consumes the JNI library and JNA AAR.

## Device pairing and presence

### Pairing API

`MeshNode` is the reusable native-node contract. It exposes suspend `status`, `createMesh`, `pair`, `add`, `ping`, and `shutdown` operations using `NodeStatus`, `NodePeer`, `PairingInvitation`, and `NodePong`. `SpiritNode` implements `MeshNode`, retains `AutoCloseable.close`, and provides noncancellable off-main `shutdown` for session cleanup.

`PairingSession(nodeFactory, meshName, nowMillis)` owns exactly one node while `run()` is active. `run()` may be called once per session instance. It publishes `StateFlow<PairingState>`, automatically creates a ticket after its first successful status read, polls status and ages displayed peer samples once per second, serializes node operations and shutdown, and closes a node even when opening or an operation is cancelled. Call `refreshTicket()` for a manual QR refresh, `pair(value)` for a scanned or pasted ticket, and `reportError(message)` for host failures such as camera errors.

A fresh receiver stays enrollable until it scans a ticket. Its first eligible enrollment attempt creates the requested mesh immediately before enrollment; later scans reuse that mesh. A failed remote enrollment can leave a founder-only mesh in place; the receiver QR is withdrawn as soon as this node enters a mesh. Independent meshes cannot merge. To join an existing mesh, have an existing member scan the fresh receiver's ticket, not the other way around.

`NodeStatus.meshId` and `PairingState.meshId` identify the mesh separately from `id`/`nodeId`. New meshes have random `mesh1_...` IDs; legacy meshes keep their old signed identity. See [mesh identity and compatibility](../wiki/design/nodes.md#identity-and-membership).

Launch `run()` in the owning ViewModel/window scope and cancel that scope on teardown. Native operations and their state publication complete before shutdown, so cancellation is not a rollback of enrollment. A second action received while `busy` is true is ignored rather than queued; hosts should disable action controls during that interval. Opening failures are reported in state and can end `run()` without ending the host UI; unrelated resources such as blob storage need their own owner-lifetime cleanup.

### Pairing and presence limits

Tickets are trimmed before use, must have `spirit1` URL-safe syntax, and are limited to 8,192 UTF-8 bytes. The session rejects its currently offered ticket before native calls; Rust validates the full payload, expiry, self pairing, replay, and mesh membership. Ticket strings and private payload material are not included in session-generated errors.

The QR countdown uses monotonic elapsed time starting before native ticket generation, so generation latency cannot extend the displayed lifetime. Expired and consumed receiver tickets are removed from `PairingState`.

`DeviceStatus.online` is true only when a peer has an authenticated ping or pong sample younger than 60,000 milliseconds. It never uses transport `connected` or `lastError`. Samples age monotonically on the one-second session timer even if polling stalls or fails; never-seen peers and peers after restart are offline. State filters the local node and deduplicates complete peer IDs, while preserving duplicate nicknames.

### Demo and native lifecycle

The demo retains its own UI adapter rather than using `PairingSession`. Its Devices tab supports explicit mesh creation, **Show pairing QR**, ticket pasting, and manual ping. Android **Scan QR code** opens a camera scanner; desktop **Open QR image** imports a PNG/JPEG. To enroll into an existing mesh, display the new device's QR and redeem it from a member, for example with `spirit mesh add '<ticket>'`.

Desktop uses `~/.spirit2/ktdemo-node` with `SPIRIT_NODE_DIR` and `SPIRIT_NODE_NAME` overrides. `SPIRIT_LOCAL=1` is only for loopback testing. Android uses an app-private, non-backed-up node directory owned by its ViewModel; camera launches and configuration changes preserve that owner. No foreground service is provided, so Android can suspend background networking. Web and iOS have no native node implementation.

Android integrations must call `AndroidNodeContext.initialize(applicationContext)` before opening a node. This installs a process-lifetime JNI reference used by iroh to read system DNS configuration. The SDK requires internet and network-state permissions; camera permission belongs to the application that implements scanning. `SpiritNode.open` and other FFI operations run on the SDK's IO dispatcher. The standard native ticket lifetime is 300 seconds; the session uses the lifetime returned by the node rather than choosing one itself.

## IntelliJ IDEA

Start IntelliJ from the project devenv so Gradle-run desktop applications inherit the Nix OpenGL runtime libraries:

```text
devenv shell -- idea .
```

Quit existing IntelliJ processes before using this command. The project-local IntelliJ settings use the Gradle wrapper and the devenv-provided Gradle JVM.

