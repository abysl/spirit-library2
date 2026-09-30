package blue.rae.spirit.sdk

import kotlinx.coroutines.async
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import uniffi.spirit_ffi.FfiException
import uniffi.spirit_ffi.SpiritNode as FfiNode
import java.nio.file.Files
import java.io.IOException
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.coroutines.asCoroutineDispatcher
import java.util.concurrent.Executors
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

class SpiritNodeTest {
    @Test
    fun `native nodes pair ping report presence and reopen`() = runBlocking {
        val firstDir = Files.createTempDirectory("spirit-kmp-node-a").toString()
        val secondDir = Files.createTempDirectory("spirit-kmp-node-b").toString()
        val first = SpiritNode.open(firstDir, "desktop", local = true)
        val second = SpiritNode.open(secondDir, "phone", local = true)
        val meshId = try {
            val createdId = first.createMesh("personal")
            assertTrue(createdId.startsWith("mesh1_"))
            assertTrue(createdId != first.status().id)
            val code = second.pair()
            assertTrue(code.ticket.startsWith("spirit1"))
            assertEquals(code.width * code.width, code.modules.size)
            assertEquals("phone", first.add(createdId, code.ticket))
            assertEquals("phone", first.ping("phone").name)
            assertTrue(first.status().peers.single().connected)
            assertTrue(second.status().peers.single().connected)
            assertEquals(createdId, second.status().meshes.single().id)
            createdId
        } finally {
            first.close()
            second.close()
        }
        SpiritNode.open(firstDir, "desktop", local = true).use { reopened ->
            assertEquals("personal", reopened.status().meshes.single().name)
            assertEquals(meshId, reopened.status().meshes.single().id)
            assertEquals("phone", reopened.status().peers.single().name)
        }
    }
}

class SpiritNodeLeaveTest {
    @Test
    fun `a device leaves, members drop it, and a member readmits it`() = runBlocking {
        val memberDir = Files.createTempDirectory("spirit-kmp-node-a").toString()
        val leaverDir = Files.createTempDirectory("spirit-kmp-node-b").toString()
        SpiritNode.open(memberDir, "desktop", local = true).use { member ->
            SpiritNode.open(leaverDir, "phone", local = true).use { leaver ->
                val mesh = member.createMesh("personal")
                assertEquals("phone", member.add(mesh, leaver.pair().ticket))
                val left = leaver.leaveMesh(mesh)
                assertEquals(LeftMesh(mesh, "personal", 1, 1), left)
                assertEquals(null, leaver.status().meshes.firstOrNull()?.name)
                assertTrue(leaver.status().peers.isEmpty())
                assertTrue(member.status().peers.isEmpty())
                assertEquals("phone", member.add(mesh, leaver.pair().ticket))
                assertEquals("personal", leaver.status().meshes.firstOrNull()?.name)
                assertEquals("phone", member.ping("phone").name)
            }
        }
    }
}

class SpiritNodeNoIntroducerTest {
    @Test
    fun `third node enrolls through a member while the original inviter is offline`() = runBlocking {
        val aDir = Files.createTempDirectory("spirit-kmp-node-a").toString()
        val bDir = Files.createTempDirectory("spirit-kmp-node-b").toString()
        val cDir = Files.createTempDirectory("spirit-kmp-node-c").toString()
        val b = SpiritNode.open(bDir, "b", local = true)
        val c = SpiritNode.open(cDir, "c", local = true)
        val a = SpiritNode.open(aDir, "a", local = true)
        try {
            val mesh = b.createMesh("personal")
            assertEquals("c", b.add(mesh, c.pair().ticket))
            b.close()
            assertEquals("a", c.add(mesh, a.pair().ticket))
            assertEquals("a", c.ping("a").name)
            assertTrue(c.status().peers.any { it.name == "a" })
        } finally {
            b.close()
            c.close()
            a.close()
        }
        SpiritNode.open(cDir, "c", local = true).use { reopened ->
            assertEquals("personal", reopened.status().meshes.single().name)
            assertTrue(reopened.status().peers.any { it.name == "a" })
        }
    }
}

class SpiritNodeMultiMeshTest {
    @Test
    fun twoMeshesEnrollOneDeviceWithFreshTickets() = runBlocking {
        val firstDir = Files.createTempDirectory("spirit-kmp-multi-a").toString()
        val secondDir = Files.createTempDirectory("spirit-kmp-multi-b").toString()
        SpiritNode.open(firstDir, "desktop", local = true).use { first ->
            SpiritNode.open(secondDir, "phone", local = true).use { second ->
                val one = first.createMesh("one")
                val two = first.createMesh("two")
                assertEquals("phone", first.add(two, second.pair().ticket))
                assertEquals("phone", first.add(one, second.pair().ticket))
                assertEquals(setOf(one, two), second.status().meshes.map { it.id }.toSet())
                assertEquals(1, first.status().peers.size)
                assertTrue(first.status().meshes.all { it.members.any { member -> member.name == "phone" && member.generation == 0L } })
                assertEquals(one, second.leaveMesh(one).meshId)
                assertEquals(listOf(two), second.status().meshes.map { it.id })
            }
        }
    }
}

class SpiritNodeLeaseTest {
    @Test
    fun secondOpenWaitsForFirstClose() = runBlocking {
        val dir = Files.createTempDirectory("spirit-lease").toString()
        val first = SpiritNode.open(dir, "device", local = true)
        try {
            val second = async { SpiritNode.open(dir, "device", local = true) }
            delay(100)
            assertFalse(second.isCompleted)
            first.close()
            second.await().use { assertEquals("device", it.status().name) }
        } finally {
            first.close()
        }
    }

    @Test
    fun waitingForHungTeardownTimesOutAsNodeBusy() = runBlocking {
        val dir = Files.createTempDirectory("spirit-lease-timeout").toString()
        SpiritNode.open(dir, "device", local = true).use {
            val error = assertFailsWith<MeshNodeException> {
                SpiritNode.open(dir, "device", local = true, leaseTimeoutMillis = 50)
            }
            assertEquals(MeshFailure.NodeBusy, error.failure)
        }
    }

    @Test
    fun nativeDirectoryLockIsMappedToNodeBusy() = runBlocking {
        val dir = Files.createTempDirectory("spirit-native-lock").toString()
        FfiNode.open(dir, "device", true, null).use { native ->
            try {
                val error = assertFailsWith<MeshNodeException> { SpiritNode.open(dir, "device", local = true) }
                assertEquals(MeshFailure.NodeBusy, error.failure)
            } finally {
                native.shutdown()
            }
        }
    }
}

class SpiritNodeErrorMappingTest {
    @Test
    fun ffiErrorsKeepTheirTypedFailures() {
        val cases = listOf(
            FfiException.Invalid("bad input") to MeshFailure.Invalid,
            FfiException.TicketRejected("cannot enroll this device into itself") to MeshFailure.TicketRejected,
            FfiException.TicketRejected("ticket was already used") to MeshFailure.TicketRejected,
            FfiException.NotMember("not enrolled") to MeshFailure.NotMember,
            FfiException.NodeClosed() to MeshFailure.NodeClosed,
            FfiException.NodeBusy() to MeshFailure.NodeBusy,
            FfiException.Node("generic") to MeshFailure.Node,
            FfiException.Unavailable("offline") to MeshFailure.Unavailable,
            FfiException.MeshLimit() to MeshFailure.MeshLimit,
            FfiException.StoreNotConfigured() to MeshFailure.StoreNotConfigured,
            FfiException.Interrupted() to MeshFailure.Interrupted,
            FfiException.Corrupt("0".repeat(64), "1".repeat(64)) to MeshFailure.Corrupt,
            FfiException.Timeout() to MeshFailure.Timeout,
            FfiException.Missing("missing") to MeshFailure.Missing,
            FfiException.Destination("sink") to MeshFailure.Destination,
            FfiException.SourceRead("source") to MeshFailure.SourceRead,
            FfiException.Io("io") to MeshFailure.Io,
            FfiException.Cancelled() to MeshFailure.Cancelled,
        )
        for ((error, expected) in cases) {
            val mapped = mapError(error)
            assertEquals(expected, mapped.failure)
            assertEquals(error, mapped.cause)
        }
        assertEquals("bad input", mapError(FfiException.Invalid("bad input")).message)
        assertEquals("offline", mapError(FfiException.Unavailable("offline")).message)
        assertEquals("io", mapError(FfiException.Io("io")).message)
    }
}

class SpiritNodeStoreTest {
    @Test
    fun importAndExportThroughNodeStoreAndStreams() = runBlocking {
        val dir = Files.createTempDirectory("spirit-node-store")
        val store = dir.resolve("store").toString()
        SpiritNode.open(dir.resolve("node").toString(), "a", local = true, storeDir = store).use { node ->
            val bytes = ByteArray(1024 * 1024 + 3) { (it % 251).toByte() }
            val hash = node.importStream { bytes.inputStream() }
            assertTrue(node.hasBlob(hash))
            assertEquals(bytes.size.toLong(), node.blobSize(hash))
            val output = java.io.ByteArrayOutputStream()
            assertEquals(bytes.size.toLong(), node.exportToStream(hash) { output })
            assertTrue(bytes.contentEquals(output.toByteArray()))
            val mesh = node.createMesh("group")
            node.setShares(mesh, listOf(hash))
            node.unshare(mesh, hash)
            node.share(mesh, hash)
            val path = dir.resolve("copy")
            assertEquals(bytes.size.toLong(), node.exportFile(hash, path.toString()))
            assertTrue(bytes.contentEquals(Files.readAllBytes(path)))
        }
    }
}

class SpiritNodeCallbackTest {
    @Test
    fun callbacksKeepOriginalCauses() = runBlocking {
        val dir = Files.createTempDirectory("spirit-callback")
        SpiritNode.open(dir.resolve("node").toString(), "a", local = true,
            storeDir = dir.resolve("store").toString()).use { node ->
            val read = IOException("reader failed")
            val failure = assertFailsWith<MeshNodeException> {
                node.importSource { object : MeshSource {
                    override fun read(max: Int): ByteArray = throw read
                    override fun close() = Unit
                } }
            }
            assertEquals(MeshFailure.SourceRead, failure.failure)
            assertEquals(read, failure.cause)
            val hash = node.importSource { object : MeshSource {
                private var done = false
                override fun read(max: Int): ByteArray = if (done) byteArrayOf() else "a".encodeToByteArray().also { done = true }
                override fun close() = Unit
            } }
            for (finishFails in listOf(false, true)) {
                val cause = IOException("sink failed")
                val error = assertFailsWith<MeshNodeException> {
                    node.exportTo(hash, object : MeshSink {
                        override fun write(bytes: ByteArray) { if (!finishFails) throw cause }
                        override fun finish() { if (finishFails) throw cause }
                    })
                }
                assertEquals(MeshFailure.Destination, error.failure)
                assertEquals(cause, error.cause)
            }
        }
    }

    @Test
    fun closingWaitsForSlowImportBeforeReopening() = runBlocking {
        val dir = Files.createTempDirectory("spirit-close-import")
        val started = CountDownLatch(1)
        val resume = CountDownLatch(1)
        val node = SpiritNode.open(dir.resolve("node").toString(), "a", local = true,
            storeDir = dir.resolve("store").toString())
        val importing = async(Dispatchers.IO) {
            assertFailsWith<MeshNodeException> {
                node.importSource { object : MeshSource {
                    override fun read(max: Int): ByteArray {
                        started.countDown()
                        resume.await(5, TimeUnit.SECONDS)
                        return byteArrayOf(1)
                    }
                    override fun close() = Unit
                } }
            }
        }
        try {
            assertTrue(withContext(Dispatchers.IO) { started.await(5, TimeUnit.SECONDS) })
            val closing = async(Dispatchers.IO) { node.close() }
            delay(50)
            resume.countDown()
            closing.await()
            importing.await()
            SpiritNode.open(dir.resolve("node").toString(), "a", local = true,
                storeDir = dir.resolve("store").toString()).use { assertEquals("a", it.status().name) }
        } finally { resume.countDown(); node.close() }
    }
}

class SpiritNodeFetchTest {
    @Test
    fun fetchSharesProgressAndWaitsForCancellationCleanup() = runBlocking {
        val firstDir = Files.createTempDirectory("spirit-fetch-a")
        val secondDir = Files.createTempDirectory("spirit-fetch-b")
        SpiritNode.open(firstDir.resolve("node").toString(), "a", local = true, storeDir = firstDir.resolve("store").toString()).use { first ->
            SpiritNode.open(secondDir.resolve("node").toString(), "b", local = true, storeDir = secondDir.resolve("store").toString()).use { second ->
                val mesh = first.createMesh("group")
                first.add(mesh, second.pair().ticket)
                val provider = first.status().id
                val bytes = ByteArray(5 * 1024 * 1024) { 7 }
                val source = first.importSource { byteSource(bytes) }
                first.share(mesh, source)
                var updates = 0
                assertEquals(bytes.size.toLong(), second.fetch(mesh, provider, source, bytes.size.toLong()) { received, total ->
                    assertTrue(received <= total)
                    updates++
                })
                assertTrue(updates > 0)
                val exported = java.io.ByteArrayOutputStream()
                second.exportTo(source, object : MeshSink {
                    override fun write(bytes: ByteArray) { exported.write(bytes) }
                    override fun finish() = Unit
                })
                assertTrue(bytes.contentEquals(exported.toByteArray()))
                val progressFailure = IOException("progress failed")
                assertEquals(progressFailure.message, assertFailsWith<IOException> {
                    second.fetch(mesh, provider, source, null) { _, _ -> throw progressFailure }
                }.message)
                val big = first.importSource { byteSource(ByteArray(64 * 1024 * 1024) { 8 }) }
                first.share(mesh, big)
                val started = kotlinx.coroutines.CompletableDeferred<Unit>()
                val transfer = async {
                    second.fetch(mesh, provider, big, null) { _, _ -> started.complete(Unit) }
                }
                kotlinx.coroutines.withTimeout(20_000) { started.await() }
                transfer.cancel()
                kotlinx.coroutines.withTimeout(20_000) { transfer.join() }
                assertFalse(second.hasBlob(big))
                assertEquals(0L, Files.list(secondDir.resolve("store/tmp")).use { it.count() })
            }
        }
    }

    @Test
    fun cancellationWhileStartIsQueuedDoesNotLeakTransfer() = runBlocking {
        val dir = Files.createTempDirectory("spirit-fetch-cancel-start")
        val providerDir = Files.createTempDirectory("spirit-fetch-cancel-provider")
        SpiritNode.open(providerDir.resolve("node").toString(), "provider", local = true,
            storeDir = providerDir.resolve("store").toString()).use { provider ->
            Executors.newSingleThreadExecutor().asCoroutineDispatcher().use { dispatcher ->
                SpiritNode.open(dir.resolve("node").toString(), "receiver", local = true,
                    storeDir = dir.resolve("store").toString(), dispatcher = dispatcher).use { node ->
                    val mesh = provider.createMesh("group")
                    provider.add(mesh, node.pair().ticket)
                    val hash = provider.importSource { byteSource(ByteArray(4 * 1024 * 1024) { 8 }) }
                    provider.share(mesh, hash)
                    val entered = CountDownLatch(1)
                    val resume = CountDownLatch(1)
                    dispatcher.executor.execute { entered.countDown(); resume.await(5, TimeUnit.SECONDS) }
                    try {
                        assertTrue(withContext(Dispatchers.IO) { entered.await(5, TimeUnit.SECONDS) })
                        val transfer = async { node.fetch(mesh, provider.status().id, hash, null) { _, _ -> } }
                        delay(50)
                        transfer.cancel()
                        resume.countDown()
                        withContext(Dispatchers.IO) { kotlinx.coroutines.withTimeout(5_000) { transfer.join() } }
                        assertFalse(node.hasBlob(hash))
                        assertEquals(0L, Files.list(dir.resolve("store/tmp")).use { it.count() })
                    } finally { resume.countDown() }
                }
            }
        }
    }

    private fun byteSource(bytes: ByteArray): MeshSource = object : MeshSource {
        private var offset = 0
        override fun read(max: Int): ByteArray {
            val end = (offset + max).coerceAtMost(bytes.size)
            return bytes.copyOfRange(offset, end).also { offset = end }
        }
        override fun close() = Unit
    }
}
