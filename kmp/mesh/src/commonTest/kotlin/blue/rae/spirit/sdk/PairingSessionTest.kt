package blue.rae.spirit.sdk

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.async
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

@OptIn(ExperimentalCoroutinesApi::class)
class PairingSessionTest {
    @Test
    fun `presence ages through poll failures at the sixty second boundary`() = runTest {
        var now = 0L
        val node = FakeNode(peerAge = 0)
        val session = PairingSession({ node }, "personal") { now }
        val running = start(session)

        node.statusFailure = IllegalStateException()
        now = 59_999
        advanceTimeBy(1_000)
        runCurrent()
        assertTrue(session.state.value.peers.single().online)

        now = 60_000
        advanceTimeBy(1_000)
        runCurrent()
        assertFalse(session.state.value.peers.single().online)
        assertEquals("Could not read node status", session.state.value.error)

        running.cancelAndJoin()
        assertEquals(1, node.shutdowns)
    }

    @Test
    fun `presence ages while a native poll is stalled`() = runTest {
        var now = 0L
        val node = FakeNode(peerAge = 0)
        val session = PairingSession({ node }, "personal") { now }
        val running = start(session)

        node.statusGate = CompletableDeferred()
        val gate = node.statusGate!!
        now = 60_000
        advanceTimeBy(1_000)
        runCurrent()
        assertFalse(session.state.value.peers.single().online)

        gate.complete(Unit)
        running.cancelAndJoin()
        assertEquals(1, node.shutdowns)
    }

    @Test
    fun `ticket expires conservatively and rejects malformed and own values`() = runTest {
        var now = 0L
        val node = FakeNode(ticketLifetimeSeconds = 5)
        val session = PairingSession({ node }, "personal") { now }
        val running = start(session)
        val offered = session.state.value.invitation

        assertTrue(offered != null)
        assertEquals(5, session.state.value.invitationSecondsRemaining)
        session.pair("not-a-ticket")
        assertEquals("Enter a valid pairing ticket", session.state.value.error)
        assertEquals(0, node.adds.size)
        session.pair(offered.ticket)
        assertEquals("This pairing ticket belongs to this device", session.state.value.error)
        assertEquals(0, node.adds.size)

        now = 5_000
        advanceTimeBy(1_000)
        runCurrent()
        assertNull(session.state.value.invitation)
        assertEquals(0, session.state.value.invitationSecondsRemaining)

        running.cancelAndJoin()
    }

    @Test
    fun `first scan creates a mesh and later scans reuse it`() = runTest {
        val node = FakeNode(meshName = null)
        val session = PairingSession({ node }, "personal") { 0L }
        val running = start(session)

        session.pair(" spirit1first ")
        session.pair("spirit1second")

        assertEquals(listOf("personal"), node.createdMeshes)
        assertEquals(listOf("spirit1first", "spirit1second"), node.adds)
        assertNull(session.state.value.invitation)
        assertEquals("personal", session.state.value.meshName)

        running.cancelAndJoin()
    }

    @Test
    fun `failed first scan withdraws the receiver ticket once the mesh exists`() = runTest {
        var now = 0L
        val node = FakeNode(meshName = null)
        node.addFailure = IllegalStateException("pairing ticket has expired")
        val session = PairingSession({ node }, "personal") { now }
        val running = start(session)
        assertTrue(session.state.value.invitation != null)

        session.pair("spirit1expired")

        assertEquals(listOf("personal"), node.createdMeshes)
        assertEquals("personal", session.state.value.meshName)
        assertNull(session.state.value.invitation)
        assertEquals(0, session.state.value.invitationSecondsRemaining)
        assertEquals("Could not add device", session.state.value.error)

        now = 1_000
        advanceTimeBy(1_000)
        runCurrent()
        assertNull(session.state.value.invitation)
        assertEquals(0, session.state.value.invitationSecondsRemaining)

        running.cancelAndJoin()
    }

    @Test
    fun `a concurrent action is rejected while another is busy and cancellation still closes the node`() = runTest {
        val node = FakeNode(meshName = "personal")
        val session = PairingSession({ node }, "personal") { 0L }
        val running = start(session)
        val gate = CompletableDeferred<Unit>()
        node.addGate = gate

        val first = backgroundScope.async { session.pair("spirit1first") }
        val second = backgroundScope.async { session.pair("spirit1second") }
        runCurrent()
        assertEquals(listOf("spirit1first"), node.adds)

        first.cancel()
        gate.complete(Unit)
        runCurrent()
        assertEquals(listOf("spirit1first"), node.adds)

        running.cancelAndJoin()
        assertTrue(session.state.value.peers.isEmpty())
        assertNull(session.state.value.invitation)
        assertEquals(1, node.shutdowns)
    }

    @Test
    fun `run cancellation clears a ticket completed before shutdown`() = runTest {
        val node = FakeNode(meshName = "personal")
        val session = PairingSession({ node }, "personal") { 0L }
        val running = start(session)
        val pairGate = CompletableDeferred<Unit>()
        node.pairGate = pairGate
        val refresh = backgroundScope.async { session.refreshTicket() }
        runCurrent()

        running.cancel()
        runCurrent()
        pairGate.complete(Unit)
        refresh.join()
        running.join()

        assertFalse(session.state.value.busy)
        assertNull(session.state.value.invitation)
        assertNull(session.state.value.notice)
        assertEquals(1, node.shutdowns)
    }

    @Test
    fun `caller cancellation still reconciles a completed native enrollment`() = runTest {
        val node = FakeNode(meshName = "personal")
        val session = PairingSession({ node }, "personal") { 0L }
        val running = start(session)
        val addGate = CompletableDeferred<Unit>()
        node.addGate = addGate
        val enrollment = backgroundScope.async { session.pair("spirit1member") }
        runCurrent()

        enrollment.cancel()
        addGate.complete(Unit)
        enrollment.join()

        assertFalse(session.state.value.busy)
        assertNull(session.state.value.invitation)
        assertEquals("Added spirit1member", session.state.value.notice)
        running.cancelAndJoin()
    }

    @Test
    fun `cancelled opening is cleaned up after the factory returns`() = runTest {
        val opened = CompletableDeferred<MeshNode>()
        val node = FakeNode()
        val session = PairingSession({ opened.await() }, "personal") { 0L }
        val running = backgroundScope.launch { session.run() }
        runCurrent()

        running.cancel()
        opened.complete(node)
        running.join()

        assertEquals(1, node.shutdowns)
    }

    @Test
    fun `open failure becomes reusable state without leaking a node`() = runTest {
        val session = PairingSession({ throw IllegalStateException() }, "personal") { 0L }

        session.run()
        assertFalse(session.state.value.loading)
        assertEquals("Could not open node", session.state.value.error)
    }

    @Test
    fun `scanner errors survive successful polls until the next action`() = runTest {
        val node = FakeNode(meshName = "personal")
        val session = PairingSession({ node }, "personal") { 0L }
        val running = start(session)

        session.reportError("Camera permission denied")
        advanceTimeBy(1_000)
        runCurrent()

        assertEquals("Camera permission denied", session.state.value.error)
        running.cancelAndJoin()
    }

    @Test
    fun `pair reads mesh membership while holding the native operation lock`() = runTest {
        val node = FakeNode(meshName = null)
        val session = PairingSession({ node }, "personal") { 0L }
        val running = start(session)
        node.snapshot = node.snapshot.copy(meshName = "joined")

        session.pair("spirit1member")

        assertTrue(node.createdMeshes.isEmpty())
        assertEquals(listOf("spirit1member"), node.adds)
        running.cancelAndJoin()
    }

    @Test
    fun `cancelled queued action does not reach the native node`() = runTest {
        val node = FakeNode(meshName = "personal")
        val session = PairingSession({ node }, "personal") { 0L }
        val running = start(session)
        val pollGate = CompletableDeferred<Unit>()
        node.statusGate = pollGate
        advanceTimeBy(1_000)
        runCurrent()

        val action = backgroundScope.async { session.pair("spirit1cancelled") }
        runCurrent()
        action.cancel()
        pollGate.complete(Unit)
        runCurrent()

        assertTrue(node.adds.isEmpty())
        running.cancelAndJoin()
    }

    @Test
    fun `invalid or overflowing received ages are offline`() = runTest {
        var now = 0L
        val node = FakeNode(peerAge = -1)
        val session = PairingSession({ node }, "personal") { now }
        val running = start(session)

        assertFalse(session.state.value.peers.single().online)
        node.snapshot = node.snapshot.copy(peers = listOf(NodePeer("peer", "peer", true, 1, null)))
        node.statusFailure = IllegalStateException()
        now = Long.MAX_VALUE
        advanceTimeBy(1_000)
        runCurrent()

        assertFalse(session.state.value.peers.single().online)
        running.cancelAndJoin()
    }

    private fun TestScope.start(session: PairingSession) = backgroundScope.launch { session.run() }.also { runCurrent() }

    private class FakeNode(
        meshName: String? = null,
        private val peerAge: Long? = null,
        ticketLifetimeSeconds: Int = 300,
    ) : MeshNode {
        var snapshot = NodeStatus(
            id = "self",
            name = "self",
            meshName = meshName,
            peers = peerAge?.let { listOf(NodePeer("peer", "peer", false, it, "ignored")) } ?: emptyList(),
        )
        var statusFailure: Throwable? = null
        var statusGate: CompletableDeferred<Unit>? = null
        var pairGate: CompletableDeferred<Unit>? = null
        var addGate: CompletableDeferred<Unit>? = null
        var addFailure: Throwable? = null
        var shutdowns = 0
        val createdMeshes = mutableListOf<String>()
        val adds = mutableListOf<String>()
        private val ticket = PairingInvitation("spirit1own", 1, byteArrayOf(1), ticketLifetimeSeconds)

        override suspend fun status(): NodeStatus {
            statusGate?.await()
            statusFailure?.let { throw it }
            return snapshot
        }

        override suspend fun createMesh(name: String) {
            createdMeshes += name
            snapshot = snapshot.copy(meshName = name)
        }

        override suspend fun pair(): PairingInvitation {
            pairGate?.await()
            return ticket
        }

        override suspend fun add(ticket: String): String {
            adds += ticket
            addGate?.await()
            addFailure?.let { throw it }
            return ticket
        }

        override suspend fun ping(device: String): NodePong = NodePong(device, 0)

        override suspend fun shutdown() {
            shutdowns++
        }
    }
}
