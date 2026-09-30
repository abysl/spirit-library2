package blue.rae.spirit.sdk

interface MeshSource {
    fun read(max: Int): ByteArray
    fun close()
}

interface MeshSink {
    fun write(bytes: ByteArray)
    fun finish()
}

interface MeshFiles {
    suspend fun importFile(path: String): String
    suspend fun importSource(open: () -> MeshSource): String
    suspend fun exportFile(hash: String, path: String): Long
    suspend fun exportTo(hash: String, sink: MeshSink): Long
    suspend fun hasBlob(hash: String): Boolean
    suspend fun blobSize(hash: String): Long
    suspend fun setShares(meshId: String, hashes: List<String>)
    suspend fun share(meshId: String, hash: String)
    suspend fun unshare(meshId: String, hash: String)
    suspend fun fetch(meshId: String, provider: String, hash: String, expectedSize: Long?, onProgress: (Long, Long) -> Unit): Long
}
