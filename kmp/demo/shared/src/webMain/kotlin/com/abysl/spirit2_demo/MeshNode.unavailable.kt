package com.abysl.spirit2_demo

import androidx.compose.material3.Text
import androidx.compose.runtime.Composable

actual fun openMeshNode(dir: String, nickname: String, local: Boolean): MeshNode? = null

@Composable
actual fun ScanPairingButton(enabled: Boolean, onTicket: (String) -> Unit, onError: (String) -> Unit) {
    Text("QR pairing is available on Android and desktop")
}
