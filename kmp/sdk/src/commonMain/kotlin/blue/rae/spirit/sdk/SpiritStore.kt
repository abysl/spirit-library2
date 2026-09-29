package blue.rae.spirit.sdk

import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.spirit_ffi.SpiritStore as FfiStore
import uniffi.spirit_ffi.FfiException

@JvmInline
value class BlobHash(val hex: String) {
    override fun toString(): String = hex
}

class SpiritStore private constructor(
    private val ffi: FfiStore,
    private val dispatcher: CoroutineDispatcher,
) : AutoCloseable {
    companion object {
        suspend fun open(
            storeDir: String,
            dispatcher: CoroutineDispatcher = Dispatchers.IO,
        ): SpiritStore = withContext(dispatcher) {
            try { SpiritStore(FfiStore.open(storeDir), dispatcher) }
            catch (error: FfiException) { throw mapError(error) }
        }
    }

    private suspend fun <T> io(block: () -> T): T = withContext(dispatcher) {
        try { block() } catch (error: FfiException) { throw mapError(error) }
    }

    suspend fun put(bytes: ByteArray): BlobHash = io { BlobHash(ffi.putBlob(bytes)) }

    suspend fun get(hash: BlobHash): ByteArray = io { ffi.getBlob(hash.hex) }

    suspend fun has(hash: BlobHash): Boolean = io { ffi.hasBlob(hash.hex) }

    override fun close() = ffi.close()
}
