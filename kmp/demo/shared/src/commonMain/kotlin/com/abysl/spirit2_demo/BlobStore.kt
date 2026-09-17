package com.abysl.spirit2_demo

interface BlobStore : AutoCloseable {
    suspend fun put(bytes: ByteArray): String
    suspend fun get(hash: String): ByteArray
    suspend fun has(hash: String): Boolean
}

expect fun openBlobStore(dir: String): BlobStore?
