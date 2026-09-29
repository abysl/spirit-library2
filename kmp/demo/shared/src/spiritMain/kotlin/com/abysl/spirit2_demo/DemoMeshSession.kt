package com.abysl.spirit2_demo

import blue.rae.spirit.sdk.MeshSession
import blue.rae.spirit.sdk.SpiritNode

actual fun openMeshSession(dir: String, nickname: String, local: Boolean): MeshSession? =
    MeshSession({ SpiritNode.open(dir, nickname, local) })
