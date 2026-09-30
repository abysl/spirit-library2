package blue.rae.spirit.sdk

class FakeMeshFiles : MeshFiles {
    private val blobs = mutableMapOf<String, ByteArray>()
    val shares = mutableMapOf<String, MutableSet<String>>()

    override suspend fun importFile(path: String): String = importSource {
        object : MeshSource {
            private var consumed = false
            override fun read(max: Int): ByteArray = if (consumed) byteArrayOf() else path.encodeToByteArray().also { consumed = true }
            override fun close() = Unit
        }
    }

    override suspend fun importSource(open: () -> MeshSource): String {
        val source = open()
        try {
            val chunks = mutableListOf<ByteArray>()
            while (true) {
                val chunk = source.read(1024 * 1024)
                if (chunk.isEmpty()) break
                chunks.add(chunk)
            }
            val bytes = chunks.fold(byteArrayOf()) { all, next -> all + next }
            val hash = bytes.contentHashCode().toString()
            blobs[hash] = bytes
            return hash
        } finally { source.close() }
    }

    override suspend fun exportFile(hash: String, path: String): Long = blobSize(hash)
    override suspend fun exportTo(hash: String, sink: MeshSink): Long {
        val bytes = blobs.getValue(hash)
        sink.write(bytes)
        sink.finish()
        return bytes.size.toLong()
    }
    override suspend fun hasBlob(hash: String): Boolean = hash in blobs
    override suspend fun blobSize(hash: String): Long = blobs.getValue(hash).size.toLong()
    override suspend fun setShares(meshId: String, hashes: List<String>) { shares[meshId] = hashes.toMutableSet() }
    override suspend fun share(meshId: String, hash: String) { shares.getOrPut(meshId) { mutableSetOf() }.add(hash) }
    override suspend fun unshare(meshId: String, hash: String) { shares[meshId]?.remove(hash) }
}
