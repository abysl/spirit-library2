package com.abysl.spirit2_demo

import androidx.compose.runtime.Composable
import blue.rae.spirit.sdk.MeshSession

expect fun openMeshSession(dir: String, nickname: String, local: Boolean = false, store: BlobStore? = null): MeshSession?

@Composable
expect fun ScanPairingButton(enabled: Boolean, onTicket: (String) -> Unit, onError: (String) -> Unit)
