package blue.rae.spirit.sdk

import java.nio.file.Paths
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.ensureActive
import kotlin.coroutines.coroutineContext
import kotlinx.coroutines.launch
import kotlinx.coroutines.channels.Channel
import uniffi.spirit_ffi.FetchListener
import uniffi.spirit_ffi.AppHandler
import uniffi.spirit_ffi.AppCall
import uniffi.spirit_ffi.AppResponseListener
import uniffi.spirit_ffi.verifyApp as nativeVerifyApp
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.withTimeout
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.spirit_ffi.SpiritNode as FfiNode
import uniffi.spirit_ffi.FfiException
import uniffi.spirit_ffi.ByteSource
import uniffi.spirit_ffi.ByteSink

internal fun mapError(error: FfiException): MeshNodeException = MeshNodeException(when (error) {
    is FfiException.Invalid -> MeshFailure.Invalid
    is FfiException.NodeClosed -> MeshFailure.NodeClosed
    is FfiException.NodeBusy -> MeshFailure.NodeBusy
    is FfiException.MeshLimit -> MeshFailure.MeshLimit
    is FfiException.NotMember -> MeshFailure.NotMember
    is FfiException.TicketRejected -> MeshFailure.TicketRejected
    is FfiException.Unavailable -> MeshFailure.Unavailable
    is FfiException.StoreNotConfigured -> MeshFailure.StoreNotConfigured
    is FfiException.Interrupted -> MeshFailure.Interrupted
    is FfiException.Corrupt -> MeshFailure.Corrupt
    is FfiException.Timeout -> MeshFailure.Timeout
    is FfiException.Missing -> MeshFailure.Missing
    is FfiException.Destination -> MeshFailure.Destination
    is FfiException.Io -> MeshFailure.Io
    is FfiException.SourceRead -> MeshFailure.SourceRead
    is FfiException.Cancelled -> MeshFailure.Cancelled
    is FfiException.Node -> MeshFailure.Node
}, when (error) {
    is FfiException.TicketRejected -> "ticket rejected"
    is FfiException.Invalid -> error.v1
    is FfiException.NotMember -> error.v1
    is FfiException.Unavailable -> error.v1
    is FfiException.Io -> error.v1
    is FfiException.SourceRead -> error.v1
    is FfiException.Destination -> error.v1
    is FfiException.Missing -> error.v1
    is FfiException.Node -> error.v1
    is FfiException.Corrupt -> "expected=${error.expected}, actual=${error.actual}"
    else -> ""
}, cause = error,
    expectedHash = (error as? FfiException.Corrupt)?.expected,
    actualHash = (error as? FfiException.Corrupt)?.actual,
)


fun verifyApp(deviceId: String, domain: String, bytes: ByteArray, signature: String): Boolean =
    try { nativeVerifyApp(deviceId, domain, bytes, signature) }
    catch (error: FfiException) { throw mapError(error) }

private fun ffiError(error: MeshNodeException): FfiException = when (error.failure) {
    MeshFailure.Invalid -> FfiException.Invalid(error.message ?: "invalid")
    MeshFailure.NodeClosed -> FfiException.NodeClosed()
    MeshFailure.NodeBusy -> FfiException.NodeBusy()
    MeshFailure.MeshLimit -> FfiException.MeshLimit()
    MeshFailure.NotMember -> FfiException.NotMember(error.message ?: "not member")
    MeshFailure.TicketRejected -> FfiException.TicketRejected("ticket rejected")
    MeshFailure.Unavailable -> FfiException.Unavailable(error.message ?: "unavailable")
    MeshFailure.StoreNotConfigured -> FfiException.StoreNotConfigured()
    MeshFailure.Interrupted -> FfiException.Interrupted()
    MeshFailure.Corrupt -> FfiException.Corrupt(error.expectedHash ?: "", error.actualHash ?: "")
    MeshFailure.Timeout -> FfiException.Timeout()
    MeshFailure.Missing -> FfiException.Missing(error.message ?: "missing")
    MeshFailure.Destination -> FfiException.Destination(error.message ?: "destination")
    MeshFailure.SourceRead -> FfiException.SourceRead(error.message ?: "source read")
    MeshFailure.Io -> FfiException.Io(error.message ?: "io")
    MeshFailure.Cancelled -> FfiException.Cancelled()
    MeshFailure.Node -> FfiException.Node(error.message ?: "node")
}

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
) : MeshNode, MeshFiles, AutoCloseable {
    private val closed = AtomicBoolean(false)
    private val activeCalls = Object()
    private var callCount = 0
    private var teardownClaimed = false

    companion object {
        suspend fun open(
            nodeDir: String,
            nickname: String,
            local: Boolean = false,
            storeDir: String? = null,
            dispatcher: CoroutineDispatcher = Dispatchers.IO,
            leaseTimeoutMillis: Long = NODE_LEASE_TIMEOUT_MILLIS,
        ): SpiritNode {
            var opened: FfiNode? = null
            var acquired: NodeDirectoryLeases.Lease? = null
            try {
                return withContext(dispatcher) {
                    val lease = NodeDirectoryLeases.acquire(nodeDir, leaseTimeoutMillis)
                    acquired = lease
                    val native = FfiNode.open(nodeDir, nickname, local, storeDir)
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

    private fun beginCall() = synchronized(activeCalls) {
        if (closed.get()) throw MeshNodeException(MeshFailure.NodeClosed)
        callCount++
    }

    private fun endCall() {
        val finish = synchronized(activeCalls) {
            callCount--
            activeCalls.notifyAll()
            if (closed.get() && callCount == 0 && !teardownClaimed) {
                teardownClaimed = true
                true
            } else false
        }
        if (finish) finishClose()
    }

    private suspend fun <T> io(block: () -> T): T = withContext(dispatcher) {
        beginCall()
        try {
            try { block() } catch (error: FfiException) { throw mapError(error) }
        } finally { endCall() }
    }

    private fun finishClose() {
        try { ffi.shutdown() } finally { try { ffi.close() } finally { lease.release() } }
    }

    private fun callbackFailure(failure: MeshFailure, error: Exception): MeshNodeException =
        MeshNodeException(failure, error.message ?: failure.name, error)


    override suspend fun importFile(path: String): String = io { ffi.importFile(path) }

    override suspend fun importSource(open: () -> MeshSource): String = io {
        val source = open()
        val original = AtomicReference<Exception?>()
        try {
            try {
                ffi.importSource(object : ByteSource {
                    override fun read(max: UInt): ByteArray = try {
                        if (closed.get()) throw MeshNodeException(MeshFailure.NodeClosed)
                        source.read(max.toInt())
                    } catch (error: Exception) {
                        original.compareAndSet(null, error)
                        if (error is MeshNodeException && error.failure == MeshFailure.NodeClosed) throw FfiException.NodeClosed()
                        throw FfiException.SourceRead(error.message ?: "source read failed")
                    }
                })
            } catch (error: FfiException) {
                original.get()?.let {
                    if (it is MeshNodeException && it.failure == MeshFailure.NodeClosed) throw it
                    throw callbackFailure(MeshFailure.SourceRead, it)
                }
                throw error
            }
        } finally {
            source.close()
        }
    }

    override suspend fun exportFile(hash: String, path: String): Long = io { ffi.exportFile(hash, path).toLong() }

    override suspend fun exportTo(hash: String, sink: MeshSink): Long = io {
        val original = AtomicReference<Exception?>()
        try {
            ffi.exportTo(hash, object : ByteSink {
                override fun write(bytes: ByteArray) = try {
                    if (closed.get()) throw MeshNodeException(MeshFailure.NodeClosed)
                    sink.write(bytes)
                } catch (error: Exception) {
                    original.compareAndSet(null, error)
                    if (error is MeshNodeException && error.failure == MeshFailure.NodeClosed) throw FfiException.NodeClosed()
                    throw FfiException.Destination(error.message ?: "destination write failed")
                }
                override fun finish() = try {
                    if (closed.get()) throw MeshNodeException(MeshFailure.NodeClosed)
                    sink.finish()
                } catch (error: Exception) {
                    original.compareAndSet(null, error)
                    if (error is MeshNodeException && error.failure == MeshFailure.NodeClosed) throw FfiException.NodeClosed()
                    throw FfiException.Destination(error.message ?: "destination finish failed")
                }
            }).toLong()
        } catch (error: FfiException) {
            original.get()?.let {
                if (it is MeshNodeException && it.failure == MeshFailure.NodeClosed) throw it
                throw callbackFailure(MeshFailure.Destination, it)
            }
            throw error
        }
    }

    override suspend fun hasBlob(hash: String): Boolean = io { ffi.hasBlob(hash) }
    override suspend fun blobSize(hash: String): Long = io { ffi.blobSize(hash).toLong() }
    override suspend fun setShares(meshId: String, hashes: List<String>) = io { ffi.setShares(meshId, hashes) }
    override suspend fun share(meshId: String, hash: String) = io { ffi.share(meshId, hash) }
    override suspend fun unshare(meshId: String, hash: String) = io { ffi.unshare(meshId, hash) }

    override suspend fun fetch(meshId: String, provider: String, hash: String, expectedSize: Long?, onQueued: () -> Unit, onProgress: (Long, Long) -> Unit): Long = coroutineScope {
        if (expectedSize != null && expectedSize < 0) throw MeshNodeException(MeshFailure.Invalid)
        beginCall()
        val completion = CompletableDeferred<Long>()
        val progress = Channel<Pair<Long?, Long?>>(Channel.BUFFERED)
        val handle = AtomicReference<uniffi.spirit_ffi.FetchHandle?>()
        val worker = launch(dispatcher) {
            for ((received, total) in progress) {
                if (received == null) onQueued() else onProgress(received, checkNotNull(total))
            }
        }
        try {
            try {
                withContext(NonCancellable + dispatcher) {
                    if (closed.get()) throw MeshNodeException(MeshFailure.NodeClosed)
                    try {
                        handle.set(ffi.startFetch(meshId, provider, hash, expectedSize?.toULong(), object : FetchListener {
                            override fun onQueued() { progress.trySend(null to null) }
                            override fun onProgress(received: ULong, total: ULong) {
                                progress.trySend(received.toLong() to total.toLong())
                            }
                            override fun onComplete(size: ULong?, error: FfiException?) {
                                if (error != null) completion.completeExceptionally(mapError(error))
                                else if (size != null) completion.complete(size.toLong())
                                else completion.completeExceptionally(MeshNodeException(MeshFailure.Node))
                            }
                        }))
                    } catch (error: FfiException) { throw mapError(error) }
                }
                coroutineContext.ensureActive()
                completion.await()
            } catch (error: CancellationException) {
                handle.get()?.cancel()
                withContext(NonCancellable) { try { completion.await() } catch (_: Exception) { } }
                throw error
            }
        } finally {
            progress.close()
            withContext(NonCancellable) { worker.join() }
            handle.get()?.close()
            endCall()
        }
    }

    override suspend fun registerAppHandler(protocol: String, handler: (AppCallInfo, ByteArray) -> ByteArray) = io {
        ffi.registerAppHandler(protocol, object : AppHandler {
            override fun handle(call: AppCall, payload: ByteArray): ByteArray = try {
                handler(object : AppCallInfo {
                    override val meshId get() = call.meshId()
                    override val peer get() = call.peer()
                    override val protocol get() = call.protocol()
                    override val remainingMs get() = call.remainingMs().toLong()
                    override val isCancelled get() = call.isCancelled()
                }, payload)
            } catch (error: Exception) {
                if (error is MeshNodeException) throw ffiError(error)
                throw FfiException.Node(error.message ?: "app handler failed")
            } finally { call.close() }
        })
    }

    override suspend fun unregisterAppHandler(protocol: String) = io { ffi.unregisterAppHandler(protocol) }
    override suspend fun appRequest(meshId: String, peer: String, protocol: String, bytes: ByteArray): ByteArray {
        beginCall()
        val completion = CompletableDeferred<ByteArray>()
        val handle = AtomicReference<uniffi.spirit_ffi.FetchHandle?>()
        try {
            try {
                withContext(NonCancellable + dispatcher) {
                    if (closed.get()) throw MeshNodeException(MeshFailure.NodeClosed)
                    try {
                        handle.set(ffi.startAppRequest(meshId, peer, protocol, bytes, object : AppResponseListener {
                            override fun onComplete(bytes: ByteArray?, error: FfiException?) {
                                if (error != null) completion.completeExceptionally(mapError(error))
                                else if (bytes != null) completion.complete(bytes)
                                else completion.completeExceptionally(MeshNodeException(MeshFailure.Node))
                            }
                        }))
                    } catch (error: FfiException) { throw mapError(error) }
                }
                coroutineContext.ensureActive()
                return completion.await()
            } catch (error: CancellationException) {
                handle.get()?.cancel()
                withContext(NonCancellable) { try { completion.await() } catch (_: Exception) { } }
                throw error
            }
        } finally {
            handle.get()?.close()
            endCall()
        }
    }
    override suspend fun signApp(domain: String, bytes: ByteArray): String = io { ffi.signApp(domain, bytes) }
    override fun verifyApp(deviceId: String, domain: String, bytes: ByteArray, signature: String): Boolean =
        blue.rae.spirit.sdk.verifyApp(deviceId, domain, bytes, signature)
    override suspend fun diagnostics(): List<Diagnostic> = io {
        ffi.diagnostics().map { Diagnostic(it.channel, it.mesh, it.peer, it.protocol, it.cause) }
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
            val deadline = System.nanoTime() + 15_000_000_000L
            val finish = synchronized(activeCalls) {
                while (callCount != 0) {
                    val remaining = (deadline - System.nanoTime()) / 1_000_000
                    if (remaining <= 0) break
                    try { activeCalls.wait(remaining) } catch (_: InterruptedException) {
                        Thread.currentThread().interrupt()
                        break
                    }
                }
                if (callCount == 0 && !teardownClaimed) {
                    teardownClaimed = true
                    true
                } else false
            }
            if (finish) finishClose()
        }
    }
}
