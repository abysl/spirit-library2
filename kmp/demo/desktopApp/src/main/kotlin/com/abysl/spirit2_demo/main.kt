package com.abysl.spirit2_demo

import androidx.compose.runtime.*
import androidx.compose.ui.window.Window
import androidx.compose.ui.window.application
import java.net.InetAddress
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

fun main() = application {
    val home = System.getProperty("user.home")
    val store = remember { openBlobStore("$home/.spirit2/ktdemo") }
    val nickname = remember { System.getenv("SPIRIT_NODE_NAME") ?: runCatching { InetAddress.getLocalHost().hostName }.getOrDefault("desktop") }
    val node = remember { openMeshNode(System.getenv("SPIRIT_NODE_DIR") ?: "$home/.spirit2/ktdemo-node", nickname, System.getenv("SPIRIT_LOCAL") == "1") }
    val scope = rememberCoroutineScope()
    var closing by remember { mutableStateOf(false) }
    Window(
        onCloseRequest = {
            if (!closing) {
                closing = true
                scope.launch {
                    try { node?.close(); withContext(Dispatchers.IO) { store?.close() } }
                    finally { exitApplication() }
                }
            }
        },
        title = "Spirit Devices",
    ) { App(store, node) }
}
