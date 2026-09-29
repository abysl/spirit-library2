package blue.rae.spirit.sdk

import kotlinx.coroutines.ExperimentalCoroutinesApi
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

    private fun TestScope.start(session: MeshSession) = backgroundScope.launch { session.run() }.also { runCurrent() }
}
