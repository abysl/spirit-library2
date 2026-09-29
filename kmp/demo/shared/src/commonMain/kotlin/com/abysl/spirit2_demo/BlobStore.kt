package com.abysl.spirit2_demo

import blue.rae.spirit.sdk.MeshFiles
import kotlinx.coroutines.flow.StateFlow

interface BlobStore : AutoCloseable {
    val dir: String
    fun bind(files: StateFlow<MeshFiles?>)
    suspend fun put(bytes: ByteArray): String
    suspend fun get(hash: String): ByteArray
    suspend fun has(hash: String): Boolean
}

expect fun openBlobStore(dir: String): BlobStore?
