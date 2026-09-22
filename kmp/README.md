# Spirit2 Kotlin Multiplatform

Kotlin Multiplatform bindings and demo applications for the sibling `../rust` workspace. The Rust `spirit-ffi` crate generates the low-level UniFFI bindings; `sdk` exposes the Kotlin API used by the demo.

## Gradle workspace

| Project | Role |
|---|---|
| `:sdk` | Kotlin SDK, `blue.rae.spirit:spirit-sdk`, targeting JVM and Android |
| `:demo:shared` | Compose Multiplatform UI and platform-neutral demo logic |
| `:demo:androidApp` | Android application |
| `:demo:desktopApp` | Desktop JVM application |
| `:demo:webApp` | JavaScript and Wasm web application |

`demo:shared` depends directly on `:sdk`. The SDK wraps generated JNA bindings with `suspend` store methods, a `BlobHash` value class, and `AutoCloseable` lifecycle management.

## Building

Run commands from this directory:

```text
direnv allow
generate-bindings
jvm-native
./gradlew :sdk:jvmTest
./gradlew :demo:shared:jvmTest
./gradlew :demo:desktopApp:run
./gradlew :demo:androidApp:assembleDebug
```

The root `devenv.nix` supplies the Android SDK and NDK, Rust targets, JDK 25, Node, Yarn, Binaryen, and the Linux runtime libraries needed by Compose Desktop. It provides `jvm-test`, `unit-test`, `desktop`, `apk`, `install`, and `assemble` convenience commands.

## IntelliJ IDEA

Start IntelliJ from the project devenv so Gradle-run desktop applications inherit the Nix OpenGL runtime libraries:

```text
devenv shell -- idea .
```

Quit existing IntelliJ processes before using this command. The project-local IntelliJ settings use the Gradle wrapper and the devenv-provided Gradle JVM.

`generate-bindings` and the native build commands compile the sibling Rust workspace and write generated bindings or Android JNI libraries into ignored SDK directories. The JVM SDK package embeds the release native library as a JNA classpath resource; Android consumes the JNI library and JNA AAR.

## Device pairing and presence

The Android and desktop demo now opens a persistent native Spirit node alongside the blob store. The Devices tab supports creating a mesh, displaying a single-use pairing QR, enrolling another device, listing members, and sending manual pings.

On Android, **Scan QR code** opens a camera scanner and requests camera permission. On desktop, **Open QR image** reads a PNG or JPEG containing a pairing code. Both accept a pasted ticket. **Show pairing QR** lets an existing member enroll this app; its text ticket can also be redeemed with `spirit mesh add '<ticket>'`.

To enroll a CLI node from the app:

1. Create a mesh in the app if it is the first device.
2. Run `spirit node init`, `spirit node serve`, and, in another terminal, `spirit node pair` on the other device.
3. Scan that code in the app, or paste its ticket and select **Add device**.
4. The new member appears in Devices. Select **Ping** to request a pong.

A green dot means an authenticated valid ping or pong was received from that member less than 60 seconds ago. A red dot means never heard from or 60 seconds of silence. Connection and request failures remain available as diagnostics without changing a green dot; a later valid ping or pong clears the diagnostic. Enrollment and membership messages do not count. The list refreshes once per second; Rust sends a heartbeat to each member every five seconds, including CLI nodes. Membership and presence are separate: disconnected members stay listed.

### State and lifecycle

Desktop uses `~/.spirit2/ktdemo-node` and defaults the nickname to the hostname. `SPIRIT_NODE_DIR` and `SPIRIT_NODE_NAME` override those values. Set `SPIRIT_LOCAL=1` only for loopback testing with CLI nodes running `--local`.

Android uses an app-private, non-backed-up node directory and initially uses the device model as its nickname. The node survives activity recreation and camera scanner launches. It is closed when its owning ViewModel is cleared. Heartbeats run while the app process is alive; an Android background service is not included, so the OS may suspend or terminate background networking. Keep the app open for enrollment and live connectivity checks.

The native SDK is currently available on Android and desktop. Web and iOS continue to display the native-feature unavailable state.

### Kotlin API

`SpiritNode.open(directory, nickname, local = false)` starts networking. Its suspend methods are `status`, `createMesh`, `pair`, `add`, and `ping`; `close` shuts down the native node and releases its directory. `status().peers` includes `connected`, `lastReceivedAgoMs`, and `lastError`. `pair()` returns the ticket, QR width, dark-module bytes, and lifetime. The wrapper moves blocking FFI calls to its IO dispatcher.

`pair()` always uses the standard five-minute window. Only the CLI can configure a different ticket lifetime with `node pair --ttl-seconds` (1 to 3600); the Kotlin contract deliberately stays minimal until storage work revisits the SDK surface.

Android integrations must call `AndroidNodeContext.initialize(applicationContext)` before opening any node. This installs a process-lifetime JNI reference used by iroh to read the system DNS configuration. The demo does this before starting the native runtime. The SDK requires internet and network-state permissions; the demo additionally requests camera permission for scanning.
