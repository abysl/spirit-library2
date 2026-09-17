import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    kotlin("multiplatform")
    id("com.android.kotlin.multiplatform.library")
}

group = "blue.rae.spirit"
version = "0.1.0"

val spirit2Root = rootDir.resolve("../rust")
val cargoRelease = spirit2Root.resolve("target/release")
val jnaPlatform = run {
    val os = System.getProperty("os.name").lowercase()
    val arch = when (val a = System.getProperty("os.arch")) {
        "amd64", "x86_64" -> "x86-64"
        "aarch64", "arm64" -> "aarch64"
        else -> a
    }
    when {
        os.contains("linux") -> "linux-$arch"
        os.contains("mac") -> "darwin-$arch"
        os.contains("windows") -> "win32-$arch"
        else -> "$os-$arch"
    }
}
val jnaResources = layout.buildDirectory.dir("generated/jna")

val copyNativeLib by tasks.registering(Copy::class) {
    from(cargoRelease) {
        include("libspirit_ffi.so", "libspirit_ffi.dylib", "spirit_ffi.dll")
    }
    into(jnaResources.map { it.dir(jnaPlatform) })
}

kotlin {
    jvmToolchain(25)
    jvm {
        compilerOptions {
            jvmTarget = JvmTarget.JVM_25
        }
    }

    android {
        namespace = "blue.rae.spirit.sdk"
        compileSdk = 36
        minSdk = 24
        compilerOptions {
            jvmTarget = JvmTarget.JVM_11
        }
    }

    sourceSets {
        commonMain.dependencies {
            implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.11.0")
        }
        jvmMain {
            dependencies {
                implementation("net.java.dev.jna:jna:5.15.0")
            }
            resources.srcDir(copyNativeLib.map { jnaResources })
        }
        androidMain.dependencies {
            implementation("net.java.dev.jna:jna:5.15.0@aar")
        }
        jvmTest.dependencies {
            implementation(kotlin("test"))
        }
    }
}
