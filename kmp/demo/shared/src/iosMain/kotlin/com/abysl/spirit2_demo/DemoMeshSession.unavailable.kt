package com.abysl.spirit2_demo

import androidx.compose.material3.Text
import androidx.compose.runtime.Composable

actual fun openMeshSession(dir: String, nickname: String, local: Boolean, store: BlobStore?): blue.rae.spirit.sdk.MeshSession? = null

@Composable
actual fun ScanPairingButton(enabled: Boolean, onTicket: (String) -> Unit, onError: (String) -> Unit) {
    Text("QR pairing is available on Android and desktop")
}
