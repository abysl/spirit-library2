package com.abysl.spirit2_demo

import blue.rae.spirit.sdk.MeshFiles
import blue.rae.spirit.sdk.MeshSink
import blue.rae.spirit.sdk.MeshSource
import blue.rae.spirit.sdk.MeshNodeException
import blue.rae.spirit.sdk.MeshFailure
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.first
import java.io.ByteArrayOutputStream
import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.CompletableDeferred

internal class NodeBlobStore(override val dir: String) : BlobStore {
    private val owner = CompletableDeferred<StateFlow<MeshFiles?>>()
    private val closed = AtomicBoolean(false)
    private val stopped = MutableStateFlow(false)

    private suspend fun node(): MeshFiles {
        if (closed.get()) throw MeshNodeException(MeshFailure.NodeClosed)
        val files = owner.await()
        val (node, closed) = combine(files, stopped) { current, stopped -> current to stopped }
            .first { (current, stopped) -> current != null || stopped }
        if (closed) throw MeshNodeException(MeshFailure.NodeClosed)
        return checkNotNull(node)
    }

    override fun bind(files: StateFlow<MeshFiles?>) {
        check(!closed.get() && owner.complete(files)) { "Blob store already bound" }
    }

    override suspend fun put(bytes: ByteArray): String = node().importSource {
        object : MeshSource {
            private var offset = 0
            override fun read(max: Int): ByteArray {
                val end = (offset + max).coerceAtMost(bytes.size)
                return bytes.copyOfRange(offset, end).also { offset = end }
            }
            override fun close() = Unit
        }
    }.hash

    override suspend fun get(hash: String): ByteArray {
        val output = ByteArrayOutputStream()
        node().exportTo(hash, object : MeshSink {
            override fun write(bytes: ByteArray) = output.write(bytes)
            override fun finish() = Unit
        })
        return output.toByteArray()
    }

    override suspend fun has(hash: String): Boolean = node().hasBlob(hash)

    override fun close() {
        closed.set(true)
        stopped.value = true
        owner.completeExceptionally(MeshNodeException(MeshFailure.NodeClosed))
    }
}
