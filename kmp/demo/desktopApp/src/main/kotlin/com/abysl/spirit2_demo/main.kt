package com.abysl.spirit2_demo

import androidx.compose.runtime.remember
import androidx.compose.ui.window.Window
import androidx.compose.ui.window.application

fun main() = application {
    val home = System.getProperty("user.home")
    val store = remember { openBlobStore("$home/.spirit2/ktdemo") }
    Window(
        onCloseRequest = ::exitApplication,
        title = "Spirit Demo",
    ) {
        App(store)
    }
}