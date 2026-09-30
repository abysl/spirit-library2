package blue.rae.spirit.sdk

import java.io.InputStream
import java.io.OutputStream
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.runBlocking

suspend fun MeshFiles.importStream(open: () -> InputStream): ImportedBlob = importSource {
    val input = open()
    object : MeshSource {
        override fun read(max: Int): ByteArray {
            val bytes = ByteArray(max)
            val count = input.read(bytes)
            return if (count < 0) byteArrayOf() else bytes.copyOf(count)
        }
        override fun close() = input.close()
    }
}

suspend fun MeshFiles.exportToStream(hash: String, open: () -> OutputStream): Long {
    var output: OutputStream? = null
    fun destination(): OutputStream = output ?: runBlocking(Dispatchers.IO) { open() }.also { output = it }
    try {
        return exportTo(hash, object : MeshSink {
            override fun write(bytes: ByteArray) = destination().write(bytes)
            override fun finish() = destination().flush()
        })
    } finally {
        output?.close()
    }
}
