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
private typealias FakeNode = MeshSessionTest.FakeNode

@OptIn(ExperimentalCoroutinesApi::class)
class MeshSessionRegressionTest {
    @Test
    fun createReadsMembershipWhileHoldingOperationsLock() = runTest {
        val node = FakeNode()
        val session = MeshSession({ node }) { 0L }
        val running = start(session)
        val invitation = session.state.value.invitation
        node.statusGate = CompletableDeferred()
        val create = backgroundScope.async { session.createGroup("new") }
        runCurrent()
        node.snapshot = node.snapshot.copy(meshes = listOf(MeshStatus("mesh1_remote", "remote", emptyList())), ticketPending = false)
        node.statusGate!!.complete(Unit)
        create.await()
        assertEquals(listOf("new", "remote"), node.snapshot.meshes.map { it.name }.sorted())
        assertTrue(invitation != session.state.value.invitation)
        assertEquals(2, node.pairCalls)
        running.cancelAndJoin()
    }

    @Test
    fun leavingReportsAllNoneAndPartialNotifications() = runTest {
        for ((count, expected) in listOf(
            2 to "Left personal and notified its other devices",
            0 to "Left personal. Notified 0 of 2 devices; the rest can also learn it when they next reach this device",
            1 to "Left personal. Notified 1 of 2 devices; notified devices relay the departure; the rest can also learn it when they next reach this device",
        )) {
            val node = FakeNode(meshName = "personal", peerAge = 0)
            node.snapshot = node.snapshot.copy(peers = listOf(NodePeer("one", "one", false, 0, null), NodePeer("two", "two", false, 0, null)))
            node.notifiedOnLeave = count
            val session = MeshSession({ node }) { 0L }
            val running = start(session)
            session.leaveGroup("mesh1_example")
            assertEquals(expected, session.state.value.notice)
            assertTrue(session.state.value.invitation != null)
            running.cancelAndJoin()
        }
        val node = FakeNode(meshName = "personal", peerAge = 0)
        node.notifiedOnLeave = 0
        val session = MeshSession({ node }) { 0L }
        val running = start(session)
        session.leaveGroup("mesh1_example")
        assertEquals("Left personal. Notified 0 of 1 device; the rest can also learn it when they next reach this device", session.state.value.notice)
        running.cancelAndJoin()
    }

    @Test
    fun suspendingPostLeaveTicketIsCancellableAndReleasesTheNode() = runTest {
        val node = FakeNode(meshName = "personal")
        val session = MeshSession({ node }) { 0L }
        val running = start(session)
        node.pairGate = CompletableDeferred()
        val leaving = backgroundScope.launch { session.leaveGroup("mesh1_example") }
        runCurrent()
        assertEquals(1, node.leaves)
        assertTrue(session.state.value.groups.isEmpty())
        leaving.cancelAndJoin()
        running.cancelAndJoin()
        assertEquals(1, node.shutdowns)
    }

    @Test
    fun actionThatSeesRemoteEnrollmentRestoresTicketAndJoinedNotice() = runTest {
        val node = FakeNode(meshName = "personal")
        val session = MeshSession({ node }) { 0L }
        val running = start(session)
        node.remoteEnrollmentOnAdd = true
        session.addDevice("mesh1_example", "spirit1member")
        assertEquals(2, node.pairCalls)
        assertTrue(session.state.value.invitation != null)
        assertEquals("Joined remote; Added peer", session.state.value.notice)
        node.snapshot = node.snapshot.copy(meshes = node.snapshot.meshes + MeshStatus("mesh1_more", "more", emptyList()), ticketPending = false)
        advanceTimeBy(1_000)
        runCurrent()
        assertEquals("Joined more", session.state.value.notice)
        assertTrue(session.state.value.invitation != null)
        assertEquals(3, node.pairCalls)
        running.cancelAndJoin()
    }

    @Test
    fun failedTicketIsAttemptedOnlyOncePerTrigger() = runTest {
        val node = FakeNode()
        node.pairFailure = IllegalStateException("spirit1secret")
        val session = MeshSession({ node }) { 0L }
        val running = start(session)
        repeat(5) { advanceTimeBy(1_000); runCurrent() }
        assertEquals(1, node.pairCalls)
        session.refreshTicket()
        assertEquals(2, node.pairCalls)
        repeat(5) { advanceTimeBy(1_000); runCurrent() }
        assertEquals(2, node.pairCalls)
        running.cancelAndJoin()
    }

    @Test
    fun emptyNameMissingGroupAndGroupLimitAreDistinct() = runTest {
        val node = FakeNode()
        val session = MeshSession({ node }) { 0L }
        val running = start(session)
        session.createGroup("   ")
        assertEquals("Enter a group name", session.state.value.error)
        session.addDevice("missing", "spirit1other")
        assertEquals("Choose a group", session.state.value.error)
        session.leaveGroup("missing")
        assertEquals("Choose a group", session.state.value.error)
        node.createFailure = MeshNodeException(MeshFailure.MeshLimit)
        session.createGroup("overflow")
        assertEquals("This device has reached the 64-group limit", session.state.value.error)
        running.cancelAndJoin()
    }

    @Test
    fun queuedActionDoesNotEraseEarlierFailureBeforeItsOwnResult() = runTest {
        val node = FakeNode(meshName = "personal")
        val session = MeshSession({ node }) { 0L }
        val running = start(session)
        node.addGate = CompletableDeferred()
        node.addFailure = IllegalStateException("spirit1secret")
        val first = backgroundScope.async { session.addDevice("mesh1_example", "spirit1first") }
        val second = backgroundScope.async { session.addDevice("mesh1_example", "spirit1second") }
        runCurrent()
        node.secondAddGate = CompletableDeferred()
        node.addGate!!.complete(Unit)
        runCurrent()
        assertEquals("Could not add device", session.state.value.error)
        assertTrue(session.state.value.busy)
        node.secondAddGate!!.complete(Unit)
        first.await()
        second.await()
        running.cancelAndJoin()
    }

    @Test
    fun createLeaveAndPingFailuresDoNotRevealNativeTickets() = runTest {
        val node = FakeNode(meshName = "personal")
        val session = MeshSession({ node }) { 0L }
        val running = start(session)
        node.createFailure = IllegalStateException("spirit1secret")
        session.createGroup("failed")
        assertEquals("Could not create group", session.state.value.error)
        node.leaveFailure = IllegalStateException("spirit1secret")
        session.leaveGroup("mesh1_example")
        assertEquals("Could not leave group", session.state.value.error)
        node.pingFailure = IllegalStateException("spirit1secret")
        session.ping("peer")
        assertEquals("Could not ping device", session.state.value.error)
        running.cancelAndJoin()
    }

    @Test
    fun successfulTicketRefreshClearsPriorTicketError() = runTest {
        val node = FakeNode(meshName = "home")
        val session = MeshSession({ node }) { 0L }
        val running = start(session)
        node.pairFailure = IllegalStateException()
        session.refreshTicket()
        assertEquals("Could not create pairing ticket", session.state.value.error)
        node.pairFailure = null
        session.refreshTicket()
        assertNull(session.state.value.error)
        assertNull(session.state.value.failure)
        assertTrue(session.state.value.invitation != null)
        running.cancelAndJoin()
    }

    @Test
    fun creationThatSeesEnrollmentCombinesNotices() = runTest {
        val node = FakeNode()
        val session = MeshSession({ node }) { 0L }
        val running = start(session)
        node.snapshot = node.snapshot.copy(meshes = listOf(MeshStatus("mesh1_remote", "remote", emptyList())), ticketPending = false)
        assertEquals("mesh1_1", session.createGroup("new"))
        assertEquals("Joined remote; Created new", session.state.value.notice)
        running.cancelAndJoin()
    }

    private fun TestScope.start(session: MeshSession) = backgroundScope.launch { session.run() }.also { runCurrent() }
}
