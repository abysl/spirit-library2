package blue.rae.spirit.sdk

import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

expect fun fakeReadFile(path: String): ByteArray
expect fun fakeWriteFile(path: String, bytes: ByteArray)

class FakeMeshFiles(val deviceId: String = "fake-node") : MeshFiles {
    private val mutex = Mutex()
    private val handlers = mutableMapOf<String, (AppCallInfo, ByteArray) -> ByteArray>()
    private val blobs = mutableMapOf<String, ByteArray>()
    private val shared = mutableMapOf<String, MutableSet<String>>()

    suspend fun sharesFor(meshId: String): Set<String> = mutex.withLock { shared[meshId]?.toSet() ?: emptySet() }

    override suspend fun importFile(path: String): String = store(fakeReadFile(path))

    override suspend fun importSource(open: () -> MeshSource): String {
        val source = open()
        val chunks = mutableListOf<ByteArray>()
        try {
            while (true) {
                val chunk = source.read(1024 * 1024)
                if (chunk.isEmpty()) break
                chunks.add(chunk)
            }
        } finally { source.close() }
        return store(chunks.fold(byteArrayOf()) { all, next -> all + next })
    }

    private suspend fun store(bytes: ByteArray): String {
        val hash = digest(bytes)
        mutex.withLock { blobs[hash] = bytes.copyOf() }
        return hash
    }

    private suspend fun blob(hash: String): ByteArray = mutex.withLock {
        blobs[hash]?.copyOf() ?: throw MeshNodeException(MeshFailure.Missing)
    }

    override suspend fun exportFile(hash: String, path: String): Long {
        val bytes = blob(hash)
        fakeWriteFile(path, bytes)
        return bytes.size.toLong()
    }

    override suspend fun exportTo(hash: String, sink: MeshSink): Long {
        val bytes = blob(hash)
        sink.write(bytes)
        sink.finish()
        return bytes.size.toLong()
    }

    override suspend fun hasBlob(hash: String): Boolean = mutex.withLock { hash in blobs }
    override suspend fun blobSize(hash: String): Long = blob(hash).size.toLong()
    override suspend fun setShares(meshId: String, hashes: List<String>) {
        mutex.withLock { shared[meshId] = hashes.toMutableSet() }
    }
    override suspend fun share(meshId: String, hash: String) {
        mutex.withLock { shared.getOrPut(meshId) { mutableSetOf() }.add(hash) }
    }
    override suspend fun unshare(meshId: String, hash: String) {
        mutex.withLock { shared[meshId]?.remove(hash) }
    }
    override suspend fun fetch(meshId: String, provider: String, hash: String, expectedSize: Long?, onProgress: (Long, Long) -> Unit): Long {
        val size = blobSize(hash)
        if (expectedSize != null && expectedSize != size) throw MeshNodeException(MeshFailure.Corrupt)
        onProgress(size, size)
        return size
    }

    override suspend fun registerAppHandler(protocol: String, handler: (AppCallInfo, ByteArray) -> ByteArray) {
        mutex.withLock { handlers[protocol] = handler }
    }
    override suspend fun unregisterAppHandler(protocol: String) {
        mutex.withLock { handlers.remove(protocol) }
    }
    override suspend fun appRequest(meshId: String, peer: String, protocol: String, bytes: ByteArray): ByteArray {
        val handler = mutex.withLock { handlers[protocol] } ?: throw MeshNodeException(MeshFailure.Unavailable)
        return handler(object : AppCallInfo {
            override val meshId = meshId
            override val peer = peer
            override val protocol = protocol
            override val remainingMs get() = 3000L
            override val isCancelled get() = false
        }, bytes.copyOf())
    }
    override suspend fun signApp(domain: String, bytes: ByteArray): String = signature(deviceId, domain, bytes)
    override fun verifyApp(deviceId: String, domain: String, bytes: ByteArray, signature: String): Boolean =
        signature == signature(deviceId, domain, bytes)
    override suspend fun diagnostics(): List<Diagnostic> = emptyList()

    private fun signature(deviceId: String, domain: String, bytes: ByteArray): String =
        "fake-unsigned:$deviceId:$domain:${digest(bytes)}"

    private fun digest(bytes: ByteArray): String {
        val states = longArrayOf(0x57c4b901feab32d1L, 0x428f1d3ae0276cb5L, 0x3c21ef850d7ab493L, 0x76a4c2e93815dbf0L)
        for (byte in bytes) for (index in states.indices) {
            val state = states[index] xor (byte.toLong() and 255L)
            states[index] = ((state shl 5) or (state ushr 59)) * 0x100000001b3L
        }
        return states.joinToString("") { it.toULong().toString(16).padStart(16, '0') }
    }
}
