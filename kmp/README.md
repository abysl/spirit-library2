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

## Pairing API

`MeshNode` is the reusable native-node contract. It exposes suspend `status`, `createMesh`, `pair`, `add`, `ping`, and `shutdown` operations using `NodeStatus`, `NodePeer`, `PairingInvitation`, and `NodePong`. `SpiritNode` implements `MeshNode`, retains `AutoCloseable.close`, and provides noncancellable off-main `shutdown` for session cleanup.

`PairingSession(nodeFactory, meshName, nowMillis)` owns exactly one node while `run()` is active. `run()` may be called once per session instance. It publishes `StateFlow<PairingState>`, automatically creates a ticket after its first successful status read, polls status and ages displayed peer samples once per second, serializes node operations and shutdown, and closes a node even when opening or an operation is cancelled. Call `refreshTicket()` for a manual QR refresh, `pair(value)` for a scanned or pasted ticket, and `reportError(message)` for host failures such as camera errors.

A fresh receiver stays enrollable until it scans a ticket. Its first successful scan creates the requested mesh immediately before enrollment; later scans reuse that mesh. Independent meshes cannot merge.

## Pairing and presence limits

Tickets are trimmed before use, must have `spirit1` URL-safe syntax, and are limited to 8,192 UTF-8 bytes. The session rejects its currently offered ticket before native calls; Rust validates the full payload, expiry, self pairing, replay, and mesh membership. Ticket strings and private payload material are not included in session-generated errors.

The QR countdown uses monotonic elapsed time starting before native ticket generation, so generation latency cannot extend the displayed lifetime. Expired and consumed receiver tickets are removed from `PairingState`.

`DeviceStatus.online` is true only when a peer has a received-message sample younger than 60,000 milliseconds. It never uses transport `connected` or `lastError`. Samples age monotonically on the one-second session timer even if polling stalls or fails; never-seen peers and peers after restart are offline. State filters the local node and deduplicates complete peer IDs, while preserving duplicate nicknames.

## IntelliJ IDEA

Start IntelliJ from the project devenv so Gradle-run desktop applications inherit the Nix OpenGL runtime libraries:

```text
devenv shell -- idea .
```

Quit existing IntelliJ processes before using this command. The project-local IntelliJ settings use the Gradle wrapper and the devenv-provided Gradle JVM.

Android integrations must call `AndroidNodeContext.initialize(applicationContext)` before opening a node. This installs a process-lifetime JNI reference used by iroh to read system DNS configuration. The SDK requires internet and network-state permissions; camera permission belongs to the application that implements scanning.
