package blue.rae.spirit.sdk

import java.nio.file.Paths
import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.withTimeout
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.spirit_ffi.SpiritNode as FfiNode
import uniffi.spirit_ffi.FfiException

internal fun mapError(error: FfiException): MeshNodeException = MeshNodeException(when (error) {
    is FfiException.Invalid -> MeshFailure.Invalid
    is FfiException.NodeClosed -> MeshFailure.NodeClosed
    is FfiException.NodeBusy -> MeshFailure.NodeBusy
    is FfiException.MeshLimit -> MeshFailure.MeshLimit
    is FfiException.NotMember -> MeshFailure.NotMember
    is FfiException.TicketRejected -> MeshFailure.TicketRejected
    is FfiException.Unavailable -> MeshFailure.Unavailable
    else -> MeshFailure.Node
})


private const val NODE_LEASE_TIMEOUT_MILLIS = 15_000L

private object NodeDirectoryLeases {
    class Entry(val mutex: Mutex = Mutex(), var users: Int = 0)
    private val entries = mutableMapOf<String, Entry>()

    class Lease(private val path: String, private val entry: Entry) {
        private val released = AtomicBoolean(false)
        fun release() {
            if (released.compareAndSet(false, true)) {
                synchronized(entries) {
                    entry.mutex.unlock()
                    releaseUser(path, entry)
                }
            }
        }
    }

    private fun releaseUser(path: String, entry: Entry) {
        entry.users--
        if (entry.users == 0) entries.remove(path)
    }

    suspend fun acquire(nodeDir: String, timeoutMillis: Long): Lease {
        val path = Paths.get(nodeDir).toAbsolutePath().normalize().toString()
        val entry = synchronized(entries) { entries.getOrPut(path) { Entry() }.also { it.users++ } }
        try {
            withTimeout(timeoutMillis) { entry.mutex.lock() }
            return Lease(path, entry)
        } catch (error: Throwable) {
            synchronized(entries) { releaseUser(path, entry) }
            if (error is TimeoutCancellationException) throw MeshNodeException(MeshFailure.NodeBusy)
            throw error
        }
    }
}

class SpiritNode private constructor(
    private val ffi: FfiNode,
    private val lease: NodeDirectoryLeases.Lease,
    private val dispatcher: CoroutineDispatcher,
) : MeshNode, AutoCloseable {
    private val closed = AtomicBoolean(false)

    companion object {
        suspend fun open(
            nodeDir: String,
            nickname: String,
            local: Boolean = false,
            dispatcher: CoroutineDispatcher = Dispatchers.IO,
            leaseTimeoutMillis: Long = NODE_LEASE_TIMEOUT_MILLIS,
        ): SpiritNode {
            var opened: FfiNode? = null
            var acquired: NodeDirectoryLeases.Lease? = null
            try {
                return withContext(dispatcher) {
                    val lease = NodeDirectoryLeases.acquire(nodeDir, leaseTimeoutMillis)
                    acquired = lease
                    val native = FfiNode.open(nodeDir, nickname, local)
                    opened = native
                    SpiritNode(native, lease, dispatcher)
                }
            } catch (error: Throwable) {
                withContext(NonCancellable + dispatcher) {
                    try { opened?.let { try { it.shutdown() } finally { it.close() } } }
                    finally { acquired?.release() }
                }
                throw if (error is FfiException) mapError(error) else error
            }
        }
    }

    private suspend fun <T> io(block: () -> T): T = withContext(dispatcher) {
        if (closed.get()) throw MeshNodeException(MeshFailure.NodeClosed)
        try { block() } catch (error: FfiException) { throw mapError(error) }
    }

    override suspend fun status(): NodeStatus = io {
        val status = ffi.status()
        NodeStatus(status.id, status.name, status.meshes.map { mesh ->
            MeshStatus(mesh.id, mesh.name, mesh.members.map { MeshMember(it.id, it.name, it.generation.toLong()) })
        }, status.peers.map {
            NodePeer(it.id, it.name, it.connected, it.lastReceivedAgoMs?.toLong(), it.lastError)
        }, status.ticketPending)
    }

    override suspend fun createMesh(name: String) = io { ffi.createMesh(name) }

    override suspend fun pair(): PairingInvitation = io {
        val code = ffi.pair()
        PairingInvitation(code.ticket, code.width.toInt(), code.modules, code.lifetimeSeconds.toInt())
    }

    override suspend fun add(meshId: String, ticket: String): String = io { ffi.add(meshId, ticket.trim()) }

    override suspend fun ping(device: String): NodePong = io {
        val pong = ffi.ping(device)
        NodePong(pong.name, pong.elapsedMs.toLong())
    }

    override suspend fun leaveMesh(meshId: String): LeftMesh = io {
        val left = ffi.leaveMesh(meshId)
        LeftMesh(left.meshId, left.meshName, left.remainingMembers.toInt(), left.notifiedMembers.toInt())
    }

    override suspend fun shutdown() = withContext(NonCancellable + dispatcher) { close() }

    override fun close() {
        if (closed.compareAndSet(false, true)) {
            try { ffi.shutdown() } finally { try { ffi.close() } finally { lease.release() } }
        }
    }
}
