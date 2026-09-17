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
