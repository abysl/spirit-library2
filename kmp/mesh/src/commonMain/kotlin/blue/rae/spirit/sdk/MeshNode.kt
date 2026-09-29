package blue.rae.spirit.sdk

data class NodePeer(
    val id: String,
    val name: String,
    val connected: Boolean,
    val lastReceivedAgoMs: Long?,
    val lastError: String?,
)

data class MeshMember(val id: String, val name: String, val generation: Long)
data class MeshStatus(val id: String, val name: String, val members: List<MeshMember>)
data class NodeStatus(val id: String, val name: String, val meshes: List<MeshStatus>, val peers: List<NodePeer>, val ticketPending: Boolean)
data class PairingInvitation(val ticket: String, val width: Int, val modules: ByteArray, val lifetimeSeconds: Int)
data class NodePong(val name: String, val elapsedMs: Long)
data class LeftMesh(val meshId: String, val meshName: String, val remainingMembers: Int, val notifiedMembers: Int)

enum class MeshFailure { Invalid, NodeClosed, NodeBusy, MeshLimit, NotMember, TicketRejected, Unavailable, StoreNotConfigured, Interrupted, Corrupt, Timeout, Missing, Destination, SourceRead, Io, Cancelled, Node }

class MeshNodeException(
    val failure: MeshFailure,
    message: String = failure.name,
    cause: Throwable? = null,
    val expectedHash: String? = null,
    val actualHash: String? = null,
) : Exception(message, cause)

interface MeshNode {
    suspend fun status(): NodeStatus
    suspend fun createMesh(name: String): String
    suspend fun pair(): PairingInvitation
    suspend fun add(meshId: String, ticket: String): String
    suspend fun ping(device: String): NodePong
    suspend fun leaveMesh(meshId: String): LeftMesh
    suspend fun shutdown()
}
