package blue.rae.spirit.sdk

import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.spirit_ffi.SpiritNode as FfiNode

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

class SpiritNode private constructor(
    private val ffi: FfiNode,
    private val dispatcher: CoroutineDispatcher,
) : AutoCloseable {
    private val closed = AtomicBoolean(false)

    companion object {
        suspend fun open(
            nodeDir: String,
            nickname: String,
            local: Boolean = false,
            dispatcher: CoroutineDispatcher = Dispatchers.IO,
        ): SpiritNode {
            var opened: FfiNode? = null
            try {
                return withContext(dispatcher) {
                    val native = FfiNode.open(nodeDir, nickname, local)
                    opened = native
                    SpiritNode(native, dispatcher)
                }
            } catch (error: Throwable) {
                withContext(NonCancellable + dispatcher) {
                    opened?.let { try { it.shutdown() } finally { it.close() } }
                }
                throw error
            }
        }
    }

    private suspend fun <T> io(block: () -> T): T = withContext(dispatcher) {
        check(!closed.get()) { "Node is closed" }
        block()
    }

    suspend fun status(): NodeStatus = io {
        val status = ffi.status()
        NodeStatus(status.id, status.name, status.meshName, status.peers.map {
            NodePeer(it.id, it.name, it.connected, it.lastReceivedAgoMs?.toLong(), it.lastError)
        })
    }

    suspend fun createMesh(name: String) = io { ffi.createMesh(name) }

    suspend fun pair(): PairingInvitation = io {
        val code = ffi.pair()
        PairingInvitation(code.ticket, code.width.toInt(), code.modules, code.lifetimeSeconds.toInt())
    }

    suspend fun add(ticket: String): String = io { ffi.add(ticket.trim()) }

    suspend fun ping(device: String): NodePong = io {
        val pong = ffi.ping(device)
        NodePong(pong.name, pong.elapsedMs.toLong())
    }

    override fun close() {
        if (closed.compareAndSet(false, true)) {
            try { ffi.shutdown() } finally { ffi.close() }
        }
    }
}
