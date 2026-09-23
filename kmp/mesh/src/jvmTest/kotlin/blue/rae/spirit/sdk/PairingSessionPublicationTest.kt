package blue.rae.spirit.sdk

import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.async
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

@OptIn(ExperimentalCoroutinesApi::class)
class PairingSessionPublicationTest {
    @Test
    fun shutdownWaitsForTicketPublication() = runTest {
        val publishing = CountDownLatch(1)
        val releasePublication = CountDownLatch(1)
        val clockCallsUntilPublication = AtomicInteger(0)
        val shutdowns = AtomicInteger(0)
        val node = object : MeshNode {
            override suspend fun status() = NodeStatus("self", "self", null, emptyList())
            override suspend fun createMesh(name: String) = Unit
            override suspend fun pair() = PairingInvitation("spirit1own", 1, byteArrayOf(1), 300)
            override suspend fun add(ticket: String) = "peer"
            override suspend fun ping(device: String) = NodePong(device, 0)
            override suspend fun shutdown() {
                shutdowns.incrementAndGet()
            }
        }
        val session = PairingSession({ node }, "personal") {
            if (clockCallsUntilPublication.getAndDecrement() == 1) {
                publishing.countDown()
                check(releasePublication.await(10, TimeUnit.SECONDS))
            }
            0L
        }
        val running = backgroundScope.launch { session.run() }
        runCurrent()
        clockCallsUntilPublication.set(2)
        val refresh = backgroundScope.async(Dispatchers.Default) { session.refreshTicket() }
        try {
            assertTrue(publishing.await(10, TimeUnit.SECONDS))
            running.cancel()
            runCurrent()
            assertEquals(0, shutdowns.get())
        } finally {
            releasePublication.countDown()
        }
        refresh.await()
        running.join()
        assertEquals(1, shutdowns.get())
        assertFalse(session.state.value.loading)
        assertFalse(session.state.value.busy)
        assertNull(session.state.value.invitation)
        assertNull(session.state.value.notice)
    }
}
