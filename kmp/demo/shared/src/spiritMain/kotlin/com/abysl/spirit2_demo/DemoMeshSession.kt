package com.abysl.spirit2_demo

import blue.rae.spirit.sdk.MeshSession
import blue.rae.spirit.sdk.SpiritNode

actual fun openMeshSession(dir: String, nickname: String, local: Boolean, store: BlobStore?): MeshSession? =
    MeshSession({
        try { SpiritNode.open(dir, nickname, local, store?.dir) }
        catch (error: Throwable) { store?.close(); throw error }
    }).also { store?.bind(it.files) }
