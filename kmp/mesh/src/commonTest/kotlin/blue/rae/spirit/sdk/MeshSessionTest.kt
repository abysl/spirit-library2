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
class MeshSessionTest {
    @Test
    fun presenceAgesThroughPollFailuresAtTheSixtySecondBoundary() = runTest {
        var now = 0L
        val node = FakeNode(peerAge = 0)
        val session = MeshSession({ node }) { now }
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
    fun presenceAgesWhileANativePollIsStalled() = runTest {
        var now = 0L
        val node = FakeNode(peerAge = 0)
        val session = MeshSession({ node }) { now }
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
    fun ticketExpiresConservativelyAndRejectsMalformedAndOwnValues() = runTest {
        var now = 0L
        val node = FakeNode(ticketLifetimeSeconds = 5)
        val session = MeshSession({ node }) { now }
        val running = start(session)
        val offered = session.state.value.invitation

        assertTrue(offered != null)
        assertEquals(5, session.state.value.invitationSecondsRemaining)
        session.addDevice("mesh1_example", "not-a-ticket")
        assertEquals("Enter a valid pairing ticket", session.state.value.error)
        assertEquals(0, node.adds.size)
        session.addDevice("mesh1_example", offered.ticket)
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
    fun scanningNeedsAChosenGroupAndNeverCreatesOne() = runTest {
        val node = FakeNode()
        val session = MeshSession({ node }) { 0L }
        val running = start(session)
        session.addDevice("mesh1_example", "spirit1first")
        assertTrue(node.adds.isEmpty())
        assertTrue(node.createdMeshes.isEmpty())
        session.createGroup("personal")
        session.addDevice("mesh1_example", " spirit1first ")
        assertEquals(listOf("personal"), node.createdMeshes)
        assertEquals(listOf("spirit1first"), node.adds)
        assertEquals("personal", session.state.value.groups.single().name)
        assertTrue(session.state.value.invitation != null)
        running.cancelAndJoin()
    }

    @Test
    fun sessionExposesTheMeshIdentityFromStatus() = runTest {
        val node = FakeNode(meshName = "personal")
        node.snapshot = node.snapshot.copy(meshes = listOf(MeshStatus("mesh1_example", "personal", emptyList())))
        val session = MeshSession({ node }) { 0L }
        val running = start(session)

        assertEquals("mesh1_example", session.state.value.groups.firstOrNull()?.id)
        assertEquals("self", session.state.value.nodeId)
        running.cancelAndJoin()
    }

    @Test
    fun failedAdmissionLeavesGroupAndTicketAvailable() = runTest {
        val node = FakeNode(meshName = "personal")
        node.addFailure = IllegalStateException("spirit1secret")
        val session = MeshSession({ node }) { 0L }
        val running = start(session)
        session.addDevice("mesh1_example", "spirit1secret")
        assertEquals("Could not add device", session.state.value.error)
        assertTrue(session.state.value.invitation != null)
        running.cancelAndJoin()
    }

    @Test
    fun busyActionsAreQueuedWhileAnotherIsBusyAndCancellationStillClosesTheNode() = runTest {
        val node = FakeNode(meshName = "personal")
        val session = MeshSession({ node }) { 0L }
        val running = start(session)
        val gate = CompletableDeferred<Unit>()
        node.addGate = gate

        val first = backgroundScope.async { session.addDevice("mesh1_example", "spirit1first") }
        val second = backgroundScope.async { session.addDevice("mesh1_example", "spirit1second") }
        runCurrent()
        assertEquals(listOf("spirit1first"), node.adds)

        first.cancel()
        gate.complete(Unit)
        runCurrent()
        assertEquals(listOf("spirit1first", "spirit1second"), node.adds)

        running.cancelAndJoin()
        assertTrue(session.state.value.peers.isEmpty())
        assertNull(session.state.value.invitation)
        assertEquals(1, node.shutdowns)
    }

    @Test
    fun runCancellationClearsATicketCompletedBeforeShutdown() = runTest {
        val node = FakeNode(meshName = null)
        val session = MeshSession({ node }) { 0L }
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
    fun callerCancellationStillReconcilesACompletedNativeEnrollment() = runTest {
        val node = FakeNode(meshName = "personal")
        val session = MeshSession({ node }) { 0L }
        val running = start(session)
        val addGate = CompletableDeferred<Unit>()
        node.addGate = addGate
        val enrollment = backgroundScope.async { session.addDevice("mesh1_example", "spirit1member") }
        runCurrent()

        enrollment.cancel()
        addGate.complete(Unit)
        enrollment.join()

        assertFalse(session.state.value.busy)
        assertTrue(session.state.value.invitation != null)
        assertEquals("Added peer", session.state.value.notice)
        running.cancelAndJoin()
    }

    @Test
    fun cancelledOpeningIsCleanedUpAfterTheFactoryReturns() = runTest {
        val opened = CompletableDeferred<MeshNode>()
        val node = FakeNode()
        val session = MeshSession({ opened.await() }) { 0L }
        val running = backgroundScope.launch { session.run() }
        runCurrent()

        running.cancel()
        opened.complete(node)
        running.join()

        assertEquals(1, node.shutdowns)
    }

    @Test
    fun openFailureBecomesReusableStateWithoutLeakingANode() = runTest {
        val session = MeshSession({ throw IllegalStateException() }) { 0L }

        session.run()
        assertFalse(session.state.value.loading)
        assertEquals("Could not open node", session.state.value.error)
    }

    @Test
    fun scannerErrorsSurviveSuccessfulPollsUntilTheNextAction() = runTest {
        val node = FakeNode(meshName = "personal")
        val session = MeshSession({ node }) { 0L }
        val running = start(session)

        session.reportError("Camera permission denied")
        advanceTimeBy(1_000)
        runCurrent()

        assertEquals("Camera permission denied", session.state.value.error)
        running.cancelAndJoin()
    }

    @Test
    fun cancelledQueuedActionDoesNotReachTheNativeNode() = runTest {
        val node = FakeNode(meshName = "personal")
        val session = MeshSession({ node }) { 0L }
        val running = start(session)
        val pollGate = CompletableDeferred<Unit>()
        node.statusGate = pollGate
        advanceTimeBy(1_000)
        runCurrent()

        val action = backgroundScope.async { session.addDevice("mesh1_example", "spirit1cancelled") }
        runCurrent()
        action.cancel()
        pollGate.complete(Unit)
        runCurrent()

        assertTrue(node.adds.isEmpty())
        running.cancelAndJoin()
    }

    @Test
    fun invalidOrOverflowingReceivedAgesAreOffline() = runTest {
        var now = 0L
        val node = FakeNode(peerAge = -1)
        val session = MeshSession({ node }) { now }
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

    @Test
    fun groupListAddAndLeaveOnlyTheSelectedGroup() = runTest {
        val node = FakeNode(meshName = "personal", peerAge = 0)
        node.snapshot = node.snapshot.copy(meshes = listOf(
            MeshStatus("mesh1_example", "personal", listOf(MeshMember("self", "self", 0), MeshMember("peer", "peer", 1))),
            MeshStatus("mesh1_second", "other", listOf(MeshMember("self", "self", 0))),
        ))
        val session = MeshSession({ node }) { 0L }
        val running = start(session)
        assertEquals(listOf("other", "personal"), session.state.value.groups.map { it.name })
        assertTrue(session.state.value.groups.single { it.id == "mesh1_example" }.members.single { it.id == "peer" }.online)
        assertEquals(1L, session.state.value.groups.single { it.id == "mesh1_example" }.members.single { it.id == "peer" }.generation)
        session.addDevice("mesh1_second", "spirit1member")
        assertEquals(listOf("mesh1_second"), node.addedTo)
        session.leaveGroup("mesh1_example")
        assertEquals(listOf("mesh1_second"), session.state.value.groups.map { it.id })
        assertEquals("Left personal and notified its other devices", session.state.value.notice)
        advanceTimeBy(1_000)
        runCurrent()
        assertEquals(listOf("mesh1_second"), session.state.value.groups.map { it.id })
        assertEquals("Left personal and notified its other devices", session.state.value.notice)
        running.cancelAndJoin()
    }

    @Test
    fun additionalGroupKeepsTheEnrolledDevicesInvitation() = runTest {
        val node = FakeNode(meshName = "personal")
        val session = MeshSession({ node }) { 0L }
        val running = start(session)
        val offered = session.state.value.invitation
        session.createGroup("second")
        assertEquals(listOf("personal", "second"), session.state.value.groups.map { it.name })
        assertEquals(offered, session.state.value.invitation)
        assertEquals(1, node.pairCalls)
        running.cancelAndJoin()
    }

    @Test
    fun groupMemberPresenceAgesDuringFailedPolls() = runTest {
        var now = 0L
        val node = FakeNode(meshName = "personal", peerAge = 0)
        node.snapshot = node.snapshot.copy(meshes = listOf(MeshStatus("mesh1_example", "personal", listOf(MeshMember("peer", "peer", 2)))))
        val session = MeshSession({ node }) { now }
        val running = start(session)
        assertTrue(session.state.value.groups.single().members.single().online)
        node.statusFailure = IllegalStateException()
        now = 60_000
        advanceTimeBy(1_000)
        runCurrent()
        assertFalse(session.state.value.groups.single().members.single().online)
        running.cancelAndJoin()
    }

    @Test
    fun enrolledDevicesRefreshTicketsAndRemoteEnrollmentConsumesThem() = runTest {
        val node = FakeNode(meshName = "personal")
        val session = MeshSession({ node }) { 0L }
        val running = start(session)
        assertTrue(session.state.value.invitation != null)
        session.refreshTicket()
        assertEquals(2, node.pairCalls)
        node.snapshot = node.snapshot.copy(meshes = node.snapshot.meshes + MeshStatus("mesh1_joined", "joined", emptyList()), ticketPending = false)
        advanceTimeBy(1_000)
        runCurrent()
        assertTrue(session.state.value.invitation != null)
        assertEquals(3, node.pairCalls)
        assertEquals(listOf("joined", "personal"), session.state.value.groups.map { it.name })
        running.cancelAndJoin()
    }

    @Test
    fun errorsCannotContainTickets() = runTest {
        val node = FakeNode(meshName = "personal")
        val session = MeshSession({ node }) { 0L }
        val running = start(session)
        session.reportError("camera read spirit1secret")
        assertEquals("Operation failed", session.state.value.error)
        node.addFailure = IllegalStateException("spirit1secret")
        session.addDevice("mesh1_example", "spirit1secret")
        assertEquals("Could not add device", session.state.value.error)
        running.cancelAndJoin()
    }

    @Test
    fun successfulAdmissionSurvivesAFailedStatusRefresh() = runTest {
        val node = FakeNode(meshName = "home")
        val session = MeshSession({ node }) { 0L }
        val running = start(session)
        node.statusFailureAfterAdd = IllegalStateException("poll failed")
        assertEquals(AddDeviceResult.Added("peer"), session.addDevice("mesh1_example", "spirit1other"))
        assertEquals("Added peer", session.state.value.notice)
        assertNull(session.state.value.error)
        running.cancelAndJoin()
    }

    @Test
    fun addFailuresExposeTypedRecoveryCategory() = runTest {
        val node = FakeNode(meshName = "home")
        val session = MeshSession({ node }) { 0L }
        val running = start(session)
        for (category in listOf(MeshFailure.TicketRejected, MeshFailure.Unavailable, MeshFailure.MeshLimit)) {
            node.adds.clear()
            node.addFailure = MeshNodeException(category)
            assertEquals(AddDeviceResult.Failed(category), session.addDevice("mesh1_example", "spirit1other"))
            assertEquals(category, session.state.value.failure)
        }
        session.clearMessages()
        assertNull(session.state.value.failure)
        assertNull(session.state.value.error)
        assertNull(session.state.value.notice)
        running.cancelAndJoin()
    }

    @Test
    fun sameGroupReadmissionConsumesTheTicketWithoutChangingMembership() = runTest {
        val node = FakeNode(meshName = "home")
        val session = MeshSession({ node }) { 0L }
        val running = start(session)
        val previous = session.state.value.invitation
        node.snapshot = node.snapshot.copy(ticketPending = false)
        advanceTimeBy(1_000)
        runCurrent()
        assertEquals(listOf("home"), session.state.value.groups.map { it.name })
        assertEquals(2, node.pairCalls)
        assertTrue(previous != session.state.value.invitation)
        running.cancelAndJoin()
    }

    @Test
    fun groupCreationAndDepartureReturnStableIdentitiesAndCounts() = runTest {
        val node = FakeNode()
        val session = MeshSession({ node }) { 0L }
        val running = start(session)
        val first = session.createGroup("same")
        val second = session.createGroup("same")
        assertEquals(listOf("mesh1_example", "mesh1_2"), listOf(first, second))
        val left = session.leaveGroup(first!!)
        assertEquals(first, left?.meshId)
        assertEquals(0, left?.remainingMembers)
        assertEquals(0, left?.notifiedMembers)
        running.cancelAndJoin()
    }

    private fun TestScope.start(session: MeshSession) = backgroundScope.launch { session.run() }.also { runCurrent() }

    internal class FakeNode(
        meshName: String? = null,
        private val peerAge: Long? = null,
        ticketLifetimeSeconds: Int = 300,
    ) : MeshNode {
        var snapshot = NodeStatus(
            id = "self",
            name = "self",
            meshes = meshName?.let { listOf(MeshStatus("mesh1_example", it, emptyList())) } ?: emptyList(),
            peers = peerAge?.let { listOf(NodePeer("peer", "peer", false, it, "ignored")) } ?: emptyList(),
            ticketPending = false,
        )
        var statusFailure: Throwable? = null
        var statusFailureAfterAdd: Throwable? = null
        var statusGate: CompletableDeferred<Unit>? = null
        var pairGate: CompletableDeferred<Unit>? = null
        var pairFailure: Throwable? = null
        var createFailure: Throwable? = null
        var pingFailure: Throwable? = null
        var secondAddGate: CompletableDeferred<Unit>? = null
        var remoteEnrollmentOnAdd = false
        var addGate: CompletableDeferred<Unit>? = null
        var addFailure: Throwable? = null
        var leaveFailure: Throwable? = null
        var notifiedOnLeave: Int? = null
        var leaves = 0
        var pairCalls = 0
        var shutdowns = 0
        val createdMeshes = mutableListOf<String>()
        val adds = mutableListOf<String>()
        val addedTo = mutableListOf<String>()
        private val ticketLifetime = ticketLifetimeSeconds

        override suspend fun status(): NodeStatus {
            statusGate?.await()
            statusFailure?.let { throw it }
            if (adds.isNotEmpty()) statusFailureAfterAdd?.let { throw it }
            return snapshot
        }

        override suspend fun createMesh(name: String): String {
            createFailure?.let { throw it }
            createdMeshes += name
            val id = if (snapshot.meshes.isEmpty()) "mesh1_example" else "mesh1_" + createdMeshes.size
            snapshot = snapshot.copy(meshes = snapshot.meshes + MeshStatus(id, name, emptyList()), ticketPending = if (snapshot.meshes.isEmpty()) false else snapshot.ticketPending)
            return id
        }

        override suspend fun pair(): PairingInvitation {
            pairCalls++
            pairGate?.await()
            pairFailure?.let { throw it }
            snapshot = snapshot.copy(ticketPending = true)
            return PairingInvitation("spirit1own" + pairCalls, 1, byteArrayOf(1), ticketLifetime)
        }

        override suspend fun add(meshId: String, ticket: String): String {
            adds += ticket
            addedTo += meshId
            if (adds.size == 2) secondAddGate?.await() else addGate?.await()
            if (adds.size == 1) addFailure?.let { throw it }
            if (remoteEnrollmentOnAdd) {
                remoteEnrollmentOnAdd = false
                snapshot = snapshot.copy(meshes = snapshot.meshes + MeshStatus("mesh1_remote", "remote", emptyList()), ticketPending = false)
            }
            return "peer"
        }

        override suspend fun ping(device: String): NodePong {
            pingFailure?.let { throw it }
            return NodePong(device, 0)
        }

        override suspend fun leaveMesh(meshId: String): LeftMesh {
            leaveFailure?.let { throw it }
            val meshName = snapshot.meshes.single { it.id == meshId }.name
            val remaining = snapshot.peers.size
            snapshot = snapshot.copy(meshes = snapshot.meshes.filterNot { it.id == meshId }, peers = emptyList(), ticketPending = false)
            leaves++
            return LeftMesh(meshId, meshName, remaining, notifiedOnLeave ?: remaining)
        }

        override suspend fun shutdown() {
            shutdowns++
        }
    }
}
