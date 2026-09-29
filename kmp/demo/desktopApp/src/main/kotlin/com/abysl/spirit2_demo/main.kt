package com.abysl.spirit2_demo

import androidx.compose.runtime.*
import androidx.compose.ui.window.Window
import androidx.compose.ui.window.application
import java.net.InetAddress
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

fun main() = application {
    val home = System.getProperty("user.home")
    val store = remember { openBlobStore("$home/.spirit2/ktdemo") }
    val nickname = remember { System.getenv("SPIRIT_NODE_NAME") ?: runCatching { InetAddress.getLocalHost().hostName }.getOrDefault("desktop") }
    val session = remember { openMeshSession(System.getenv("SPIRIT_NODE_DIR") ?: "$home/.spirit2/ktdemo-node", nickname, System.getenv("SPIRIT_LOCAL") == "1") }
    val scope = rememberCoroutineScope()
    val running = remember(session) { scope.launch { session?.run() } }
    var closing by remember { mutableStateOf(false) }
    Window(
        onCloseRequest = {
            if (!closing) {
                closing = true
                scope.launch {
                    try { running.cancelAndJoin(); withContext(Dispatchers.IO) { store?.close() } }
                    finally { exitApplication() }
                }
            }
        },
        title = "Spirit Devices",
    ) { App(store, session) }
}
