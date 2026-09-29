package blue.rae.spirit.sdk

import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.async
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.withContext
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

@OptIn(ExperimentalCoroutinesApi::class)
class MeshSessionReviewTest {

    @Test
    fun failedLeaveKeepsMembershipAndTicket() = runTest {
        val node = MeshSessionTest.FakeNode(meshName = "personal", peerAge = 0)
        val session = MeshSession({ node }) { 0L }
        val running = start(session)
        node.leaveFailure = IllegalStateException("spirit1secret")
        session.leaveGroup("mesh1_example")
        assertEquals("Could not leave group", session.state.value.error)
        assertEquals("personal", session.state.value.groups.single().name)
        assertTrue(session.state.value.invitation != null)
        assertEquals(0, node.leaves)
        running.cancelAndJoin()
    }

    @Test
    fun completedLeaveAndCreateKeepTheirNoticesWhenTicketFails() = runTest {
        for (leave in listOf(true, false)) {
            val node = MeshSessionTest.FakeNode(meshName = if (leave) "personal" else null)
            val session = MeshSession({ node }) { 0L }
            val running = start(session)
            node.pairFailure = IllegalStateException("spirit1secret")
            if (leave) session.leaveGroup("mesh1_example") else session.createGroup("new")
            assertEquals("Could not create pairing ticket", session.state.value.error)
            assertEquals(if (leave) "Left personal" else "Created new", session.state.value.notice)
            assertEquals(if (leave) 1 else 0, node.leaves)
            assertEquals(if (leave) 0 else 1, node.createdMeshes.size)
            running.cancelAndJoin()
        }
    }

    @Test
    fun pendingNativeTicketBeforeDeadlineDoesNotRefresh() = runTest {
        var now = 0L
        val node = MeshSessionTest.FakeNode(ticketLifetimeSeconds = 5)
        val session = MeshSession({ node }) { now }
        val running = start(session)
        val offered = session.state.value.invitation
        now = 4_999
        advanceTimeBy(1_000)
        runCurrent()
        assertEquals(offered, session.state.value.invitation)
        assertEquals(1, node.pairCalls)
        now = 5_000
        advanceTimeBy(1_000)
        runCurrent()
        assertNull(session.state.value.invitation)
        assertEquals(1, node.pairCalls)
        running.cancelAndJoin()
    }

    @Test
    fun cancelledNativeRefreshWithdrawsStaleInvitation() = runTest {
        val original = MeshSessionTest.FakeNode()
        val entered = CompletableDeferred<Unit>()
        val release = CompletableDeferred<Unit>()
        var pairCalls = 0
        val node = object : MeshNode by original {
            override suspend fun pair(): PairingInvitation {
                pairCalls++
                if (pairCalls == 2) {
                    entered.complete(Unit)
                    withContext(NonCancellable) { release.await() }
                }
                return PairingInvitation("spirit1offer" + pairCalls, 1, byteArrayOf(1), 300)
            }
        }
        val session = MeshSession({ node }) { 0L }
        val running = start(session)
        assertTrue(session.state.value.invitation != null)
        val refresh = backgroundScope.async { session.refreshTicket() }
        runCurrent()
        entered.await()
        refresh.cancel()
        release.complete(Unit)
        refresh.join()
        assertNull(session.state.value.invitation)
        assertFalse(session.state.value.busy)
        advanceTimeBy(1_000)
        runCurrent()
        assertEquals("spirit1offer3", session.state.value.invitation?.ticket)
        running.cancelAndJoin()
    }

    @Test
    fun remoteAdmissionProvidesJoinedIdUntilMessagesAreCleared() = runTest {
        val node = MeshSessionTest.FakeNode(meshName = "personal")
        val session = MeshSession({ node }) { 0L }
        val running = start(session)
        assertTrue(session.state.value.joinedGroupIds.isEmpty())
        node.snapshot = node.snapshot.copy(meshes = node.snapshot.meshes + MeshStatus("mesh1_remote", "remote", emptyList()), ticketPending = false)
        advanceTimeBy(1_000)
        runCurrent()
        assertEquals(listOf("mesh1_remote"), session.state.value.joinedGroupIds)
        assertEquals("Joined remote", session.state.value.notice)
        session.clearMessages()
        assertTrue(session.state.value.joinedGroupIds.isEmpty())
        running.cancelAndJoin()
    }

    @Test
    fun openTicketAndPollKeepTypedFailures() = runTest {
        val open = MeshSession({ throw MeshNodeException(MeshFailure.NodeBusy) })
        open.run()
        assertEquals(MeshFailure.NodeBusy, open.state.value.failure)
        val node = MeshSessionTest.FakeNode()
        val session = MeshSession({ node }) { 0L }
        val running = start(session)
        node.pairFailure = MeshNodeException(MeshFailure.Unavailable)
        session.refreshTicket()
        assertEquals(MeshFailure.Unavailable, session.state.value.failure)
        session.clearMessages()
        node.statusFailure = MeshNodeException(MeshFailure.NodeClosed)
        advanceTimeBy(1_000)
        runCurrent()
        assertEquals(MeshFailure.NodeClosed, session.state.value.failure)
        running.cancelAndJoin()
    }

    private fun TestScope.start(session: MeshSession) = backgroundScope.launch { session.run() }.also { runCurrent() }
}
