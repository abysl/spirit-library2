package blue.rae.spirit.sdk

data class NodePeer(
    val id: String,
    val name: String,
    val connected: Boolean,
    val lastReceivedAgoMs: Long?,
    val lastError: String?,
)

data class NodeStatus(val id: String, val name: String, val meshName: String?, val peers: List<NodePeer>)
data class PairingInvitation(val ticket: String, val width: Int, val modules: ByteArray, val lifetimeSeconds: Int)
data class NodePong(val name: String, val elapsedMs: Long)

interface MeshNode {
    suspend fun status(): NodeStatus
    suspend fun createMesh(name: String)
    suspend fun pair(): PairingInvitation
    suspend fun add(ticket: String): String
    suspend fun ping(device: String): NodePong
    suspend fun shutdown()
}
