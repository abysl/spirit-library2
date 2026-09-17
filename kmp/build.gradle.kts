@file:OptIn(org.jetbrains.kotlin.gradle.ExperimentalWasmDsl::class)

import org.jetbrains.kotlin.gradle.targets.js.nodejs.NodeJsEnvSpec
import org.jetbrains.kotlin.gradle.targets.js.nodejs.NodeJsPlugin
import org.jetbrains.kotlin.gradle.targets.js.nodejs.NodeJsRootPlugin
import org.jetbrains.kotlin.gradle.targets.js.yarn.YarnPlugin
import org.jetbrains.kotlin.gradle.targets.js.yarn.YarnRootEnvSpec
import org.jetbrains.kotlin.gradle.targets.wasm.binaryen.BinaryenEnvSpec
import org.jetbrains.kotlin.gradle.targets.wasm.binaryen.BinaryenPlugin
import org.jetbrains.kotlin.gradle.targets.wasm.nodejs.WasmNodeJsEnvSpec
import org.jetbrains.kotlin.gradle.targets.wasm.nodejs.WasmNodeJsPlugin
import org.jetbrains.kotlin.gradle.targets.wasm.nodejs.WasmNodeJsRootPlugin
import org.jetbrains.kotlin.gradle.targets.wasm.yarn.WasmYarnPlugin
import org.jetbrains.kotlin.gradle.targets.wasm.yarn.WasmYarnRootEnvSpec

plugins {
    alias(libs.plugins.androidApplication) apply false
    alias(libs.plugins.androidMultiplatformLibrary) apply false
    alias(libs.plugins.composeMultiplatform) apply false
    alias(libs.plugins.composeCompiler) apply false
    alias(libs.plugins.kotlinJvm) apply false
    alias(libs.plugins.kotlinMultiplatform) apply false
}

val downloadWebTools = !providers.environmentVariable("DEVENV_ROOT").isPresent

allprojects {
    plugins.withType<NodeJsRootPlugin> { the<NodeJsEnvSpec>().download.set(downloadWebTools) }
    plugins.withType<NodeJsPlugin> { the<NodeJsEnvSpec>().download.set(downloadWebTools) }
    plugins.withType<YarnPlugin> { the<YarnRootEnvSpec>().download.set(downloadWebTools) }
    plugins.withType<WasmNodeJsRootPlugin> { the<WasmNodeJsEnvSpec>().download.set(downloadWebTools) }
    plugins.withType<WasmNodeJsPlugin> { the<WasmNodeJsEnvSpec>().download.set(downloadWebTools) }
    plugins.withType<WasmYarnPlugin> { the<WasmYarnRootEnvSpec>().download.set(downloadWebTools) }
    plugins.withType<BinaryenPlugin> { the<BinaryenEnvSpec>().download.set(downloadWebTools) }
}
