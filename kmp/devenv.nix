{
  pkgs,
  lib,
  ...
}: let
  skikoRuntimeLibs = with pkgs; [
    fontconfig
    freetype
    libglvnd
    libxkbcommon
    libx11
    libxext
    libxi
    libxrender
    libxtst
  ];
in {
  cachix.enable = false;

  android = {
    enable = true;
    platforms.version = ["34" "36" "37"];
    buildTools.version = ["34.0.0" "36.0.0"];
    ndk.enable = true;
    ndk.version = ["26.3.11579264"];
  };

  languages.rust = {
    enable = true;
    channel = "stable";
    targets = [
      "aarch64-linux-android"
      "x86_64-linux-android"
      "aarch64-apple-ios"
      "aarch64-apple-ios-sim"
    ];
  };

  packages = with pkgs;
    [
      binaryen
      cargo-ndk
      jdk25
      nodejs_22
      yarn
    ]
    ++ skikoRuntimeLibs;

  env.LD_LIBRARY_PATH = lib.makeLibraryPath skikoRuntimeLibs;

  enterShell = ''
    export ANDROID_SDK_ROOT="$ANDROID_HOME"
    export JAVA25_HOME="${pkgs.jdk25.home}"
    export JAVA_HOME="$JAVA25_HOME"
    echo "spirit2/kmp dev env — android sdk at $ANDROID_HOME, jdk 25"
  '';

  scripts."generate-bindings".exec = ''
    set -eu
    cd "$DEVENV_ROOT/../rust"
    cargo build -p spirit-ffi --bin uniffi-bindgen --features uniffi/cli
    lib="target/debug/libspirit_ffi.so"
    [ -f "$lib" ] || lib="target/debug/libspirit_ffi.dylib"
    ./target/debug/uniffi-bindgen generate --library "$lib" --language kotlin \
      --out-dir "$DEVENV_ROOT/sdk/src/commonMain/kotlin" --no-format
    echo "generated sdk/src/commonMain/kotlin/uniffi/spirit_ffi/"
  '';

  scripts."jvm-native".exec = ''
    set -eu
    cd "$DEVENV_ROOT/../rust"
    cargo build --release -p spirit-ffi
    echo "built rust/target/release/libspirit_ffi.{so,dylib,dll}"
  '';

  scripts."android-native".exec = ''
    set -eu
    export ANDROID_NDK_HOME=$ANDROID_NDK_ROOT
    cd "$DEVENV_ROOT/../rust"
    cargo ndk -t arm64-v8a -t x86_64 -P 26 \
      -o "$DEVENV_ROOT/sdk/src/androidMain/jniLibs" \
      build --release -p spirit-ffi
  '';

  scripts."ios-native".exec = ''
    set -eu
    cd "$DEVENV_ROOT/../rust"
    cargo build --release --target aarch64-apple-ios -p spirit-ffi
    cargo build --release --target aarch64-apple-ios-sim -p spirit-ffi
    echo "bundle the two static libs into an .xcframework with xcodebuild -create-xcframework"
  '';

  scripts."jvm-test".exec = ''
    set -eu
    generate-bindings
    jvm-native
    "$DEVENV_ROOT"/gradlew -p "$DEVENV_ROOT" :sdk:jvmTest
  '';

  scripts."unit-test".exec = ''
    set -eu
    generate-bindings
    jvm-native
    "$DEVENV_ROOT"/gradlew -p "$DEVENV_ROOT" :demo:shared:jvmTest
  '';

  scripts."desktop".exec = ''
    set -eu
    generate-bindings
    jvm-native
    "$DEVENV_ROOT"/gradlew -p "$DEVENV_ROOT" :demo:desktopApp:run
  '';

  scripts."apk".exec = ''
    set -eu
    generate-bindings
    jvm-native
    android-native
    "$DEVENV_ROOT"/gradlew -p "$DEVENV_ROOT" :demo:androidApp:assembleDebug
    echo "apk at $DEVENV_ROOT/demo/androidApp/build/outputs/apk/debug/androidApp-debug.apk"
  '';

  scripts."install".exec = ''
    set -eu
    apk
    adb install -r "$DEVENV_ROOT"/demo/androidApp/build/outputs/apk/debug/androidApp-debug.apk
  '';

  scripts."assemble".exec = ''
    set -eu
    generate-bindings
    jvm-native
    android-native
    "$DEVENV_ROOT"/gradlew -p "$DEVENV_ROOT" build
  '';

  scripts."ide-gradle-props".exec = ''
    set -eu
    mkdir -p "$HOME/.gradle"
    cat > "$HOME/.gradle/gradle.properties" <<EOF
org.gradle.java.installations.paths=$JAVA25_HOME
android.aapt2FromMavenOverride=$ANDROID_HOME/build-tools/36.0.0/aapt2
EOF
    echo "wrote $HOME/.gradle/gradle.properties"
  '';
}
