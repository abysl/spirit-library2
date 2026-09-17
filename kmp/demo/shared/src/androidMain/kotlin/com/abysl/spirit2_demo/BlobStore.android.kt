package com.abysl.spirit2_demo

import blue.rae.spirit.sdk.BlobHash
import blue.rae.spirit.sdk.SpiritStore
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

actual fun openBlobStore(dir: String): BlobStore? = SpiritBlobStore(dir)

private class SpiritBlobStore(private val dir: String) : BlobStore {
    private val lock = Mutex()
    private var store: SpiritStore? = null

    private suspend fun store(): SpiritStore = lock.withLock {
        store ?: SpiritStore.open(dir).also { store = it }
    }

    override suspend fun put(bytes: ByteArray): String = store().put(bytes).hex

    override suspend fun get(hash: String): ByteArray = store().get(BlobHash(hash))

    override suspend fun has(hash: String): Boolean = store().has(BlobHash(hash))

    override fun close() {
        store?.close()
        store = null
    }
}
