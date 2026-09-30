package blue.rae.spirit.sdk

interface MeshSource {
    fun read(max: Int): ByteArray
    fun close()
}

interface MeshSink {
    fun write(bytes: ByteArray)
    fun finish()
}

interface AppCallInfo {
    val meshId: String
    val peer: String
    val protocol: String
    val remainingMs: Long
    val isCancelled: Boolean
}
data class Diagnostic(val channel: String, val mesh: String, val peer: String, val protocol: String, val cause: String)

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
    suspend fun fetch(meshId: String, provider: String, hash: String, expectedSize: Long?, onQueued: () -> Unit = {}, onProgress: (Long, Long) -> Unit): Long
    suspend fun registerAppHandler(protocol: String, handler: (AppCallInfo, ByteArray) -> ByteArray)
    suspend fun unregisterAppHandler(protocol: String)
    suspend fun appRequest(meshId: String, peer: String, protocol: String, bytes: ByteArray): ByteArray
    suspend fun signApp(domain: String, bytes: ByteArray): String
    fun verifyApp(deviceId: String, domain: String, bytes: ByteArray, signature: String): Boolean
    suspend fun diagnostics(): List<Diagnostic>
}
