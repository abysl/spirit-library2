package com.abysl.spirit2_demo

import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.async
import kotlinx.coroutines.launch
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.flow.first
import java.nio.file.Files
import kotlin.io.path.absolutePathString
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertTrue
import kotlin.test.assertFailsWith
import blue.rae.spirit.sdk.MeshNodeException
import blue.rae.spirit.sdk.MeshFailure

class BlobStoreDesktopTest {
    @Test
    fun closeCompletesPendingRequestsExceptionally() = runBlocking {
        kotlinx.coroutines.supervisorScope {
        val dir = Files.createTempDirectory("spirit-demo-pending").toString()
        val store = assertNotNull(openBlobStore(dir))
        val waiter = async(start = kotlinx.coroutines.CoroutineStart.UNDISPATCHED) {
            store.put(byteArrayOf(1))
        }
        store.close()
        assertEquals(MeshFailure.NodeClosed, assertFailsWith<MeshNodeException> { waiter.await() }.failure)
        }
    }

    @Test
    fun theDesktopStoreIsSpirit() {
        val dir = Files.createTempDirectory("spirit-demo-store").absolutePathString()
        val store = assertNotNull(openBlobStore(dir))
        val nodeDir = Files.createTempDirectory("spirit-demo-node").absolutePathString()
        val session = assertNotNull(openMeshSession(nodeDir, "desktop", true, store))
        var hash = ""
        runBlocking {
            val running = launch(kotlinx.coroutines.Dispatchers.IO) { session.run() }
            try {
                kotlinx.coroutines.withTimeout(10_000) { session.state.first { !it.loading } }
                store.use {
                    hash = it.put("from the demo".encodeToByteArray())
                    assertEquals(64, hash.length)
                    assertTrue(it.has(hash))
                    assertContentEquals("from the demo".encodeToByteArray(), it.get(hash))
                    assertTrue(java.io.File(dir, hash).isFile)
                }
            } finally { running.cancelAndJoin() }
            val reopenedStore = assertNotNull(openBlobStore(dir))
            val reopened = assertNotNull(openMeshSession(nodeDir, "desktop", true, reopenedStore))
            val next = launch(kotlinx.coroutines.Dispatchers.IO) { reopened.run() }
            try {
                kotlinx.coroutines.withTimeout(10_000) { reopened.state.first { !it.loading } }
                reopenedStore.use { assertContentEquals("from the demo".encodeToByteArray(), it.get(hash)) }
            } finally { next.cancelAndJoin() }
        }
    }
}
