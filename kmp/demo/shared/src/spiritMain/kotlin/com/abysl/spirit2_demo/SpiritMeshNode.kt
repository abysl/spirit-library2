package com.abysl.spirit2_demo

import blue.rae.spirit.sdk.SpiritNode
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext

actual fun openMeshNode(dir: String, nickname: String, local: Boolean): MeshNode? = SpiritMeshNode(dir, nickname, local)

private class SpiritMeshNode(private val dir: String, private val nickname: String, private val local: Boolean) : MeshNode {
    private val lock = Mutex()
    private var node: SpiritNode? = null
    private var closed = false

    private suspend fun native(): SpiritNode = lock.withLock {
        check(!closed) { "Node is closed" }
        node ?: SpiritNode.open(dir, nickname, local).also { node = it }
    }

    override suspend fun status(): MeshSnapshot {
        val status = native().status()
        return MeshSnapshot(status.name, status.meshName, status.peers.map {
            MeshPeer(it.id, it.name, it.connected, it.lastReceivedAgoMs, it.lastError)
        })
    }

    override suspend fun createMesh(name: String) = native().createMesh(name)

    override suspend fun pair(): MeshInvitation {
        val invitation = native().pair()
        return MeshInvitation(invitation.ticket, invitation.width, invitation.modules, invitation.lifetimeSeconds)
    }

    override suspend fun add(ticket: String): String = native().add(ticket)

    override suspend fun ping(id: String): String {
        val pong = native().ping(id)
        return "Pong from ${pong.name} in ${pong.elapsedMs} ms"
    }

    override suspend fun close() = withContext(Dispatchers.IO) {
        lock.withLock {
            closed = true
            node?.close()
            node = null
        }
    }
}
