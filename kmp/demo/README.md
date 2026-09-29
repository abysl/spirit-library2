# Spirit2 demo

`spirit-demo` is the Compose Multiplatform consumer of the parent `spirit2/kmp` Gradle workspace.

| Gradle project | Role |
|---|---|
| `:demo:shared` | Shared UI, store integration, and platform source sets |
| `:demo:androidApp` | Android entry point |
| `:demo:desktopApp` | Desktop JVM entry point |
| `:demo:webApp` | JavaScript and Wasm web entry point |

The shared module depends on the parent workspace's `:sdk` project for JVM and Android. Web and iOS builds expose the UI without a native store because the current UniFFI Kotlin bindings use JNA.

Run builds from the parent project:

```text
cd ..
direnv allow
unit-test
./gradlew :demo:shared:jvmTest
desktop
./gradlew :demo:desktopApp:run
apk
./gradlew :demo:androidApp:assembleDebug
```

The parent development environment builds the sibling `../rust` FFI crate, generates Kotlin bindings, supplies Android JNI libraries, and configures the runtime dependencies required by desktop Compose.

The **Devices** tab lists groups and their members, lets you create groups, paste or scan a ticket from within a chosen group, show this device’s QR, leave a group, and ping its members. **Blobs** retains local put/get. Android scans QR codes with the camera; desktop imports PNG/JPEG QR images. The [pairing guide](../README.md#device-pairing-and-presence) describes ticket use and node handover.

Green means an authenticated valid ping or pong was received from that member less than 60 seconds ago; red means no previous heartbeat or at least 60 seconds of silence. The demo does not display per-peer network error diagnostics; transport connection flags and errors do not determine this presence color. Rust heartbeats run every five seconds independently of UI polling.
