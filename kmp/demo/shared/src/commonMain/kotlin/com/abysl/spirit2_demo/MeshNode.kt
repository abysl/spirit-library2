package com.abysl.spirit2_demo

import androidx.compose.runtime.Composable

data class MeshPeer(
    val id: String,
    val name: String,
    val connected: Boolean,
    val lastReceivedAgoMs: Long?,
    val lastError: String?,
)

data class MeshSnapshot(val name: String, val meshName: String?, val peers: List<MeshPeer>)
data class MeshInvitation(val ticket: String, val width: Int, val modules: ByteArray, val lifetimeSeconds: Int)

interface MeshNode {
    suspend fun status(): MeshSnapshot
    suspend fun createMesh(name: String)
    suspend fun pair(): MeshInvitation
    suspend fun add(ticket: String): String
    suspend fun ping(id: String): String
    suspend fun close()
}

expect fun openMeshNode(dir: String, nickname: String, local: Boolean = false): MeshNode?

@Composable
expect fun ScanPairingButton(enabled: Boolean, onTicket: (String) -> Unit, onError: (String) -> Unit)

fun MeshPeer.connectionLabel(): String = when {
    connected -> "Connected"
    lastError != null -> "Disconnected · $lastError"
    lastReceivedAgoMs == null -> "Disconnected · waiting for a message"
    else -> "Disconnected · last message ${lastReceivedAgoMs / 1000}s ago"
}
