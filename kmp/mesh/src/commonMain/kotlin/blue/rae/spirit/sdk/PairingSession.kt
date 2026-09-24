package blue.rae.spirit.sdk

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlin.math.max
import kotlin.time.TimeSource

data class DeviceStatus(val id: String, val name: String, val online: Boolean)

private val monotonicOrigin = TimeSource.Monotonic.markNow()

data class PairingState(
    val loading: Boolean = true,
    val nodeId: String = "",
    val name: String = "",
    val meshName: String? = null,
    val peers: List<DeviceStatus> = emptyList(),
    val invitation: PairingInvitation? = null,
    val invitationSecondsRemaining: Int = 0,
    val busy: Boolean = false,
    val error: String? = null,
    val notice: String? = null,
    val meshId: String? = null,
)

class PairingSession(
    private val nodeFactory: suspend () -> MeshNode,
    private val meshName: String,
    private val nowMillis: () -> Long = { monotonicOrigin.elapsedNow().inWholeMilliseconds },
) {
    private data class PeerSample(
        val id: String,
        val name: String,
        val receivedAgoMs: Long?,
        val observedAtMillis: Long,
    )

    private val operations = Mutex()
    private val actions = Mutex()
    private val tracking = Mutex()
    private val mutableState = MutableStateFlow(PairingState())
    private var node: MeshNode? = null
    private var started = false
    private var offeredInitialTicket = false
    private var ticketExpiresAtMillis: Long? = null
    private var peerSamples: List<PeerSample> = emptyList()

    val state: StateFlow<PairingState> = mutableState.asStateFlow()

    suspend fun run() {
        operations.withLock {
            check(!started) { "PairingSession.run may only be called once" }
            started = true
        }
        try {
            if (!openNode()) return
            currentCoroutineContext().ensureActive()
            coroutineScope {
                launch { ageState() }
                while (true) {
                    poll()
                    delay(POLL_INTERVAL_MILLIS)
                }
            }
        } finally {
            withContext(NonCancellable) { closeNode() }
        }
    }

    suspend fun refreshTicket() {
        if (!beginAction()) return
        try {
            withActiveNode { activeNode -> offerTicket(activeNode, notice = null) }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (_: Exception) {
            mutableState.update { it.copy(error = TICKET_ERROR) }
        } finally {
            finishAction()
        }
    }

    suspend fun pair(value: String) {
        if (!beginAction()) return
        try {
            val ticket = value.trim()
            when {
                !isTicketSyntax(ticket) -> {
                    mutableState.update { it.copy(error = INVALID_TICKET_ERROR, notice = null) }
                    return
                }
                state.value.invitation?.ticket == ticket -> {
                    mutableState.update { it.copy(error = OWN_TICKET_ERROR, notice = null) }
                    return
                }
            }
            withActiveNode { activeNode ->
                if (activeNode.status().meshName == null) {
                    activeNode.createMesh(meshName)
                    tracking.withLock { recordMeshMembership(meshName) }
                }
                val addedName = activeNode.add(ticket)
                tracking.withLock {
                    ticketExpiresAtMillis = null
                    mutableState.update {
                        it.copy(
                            invitation = null,
                            invitationSecondsRemaining = 0,
                            error = null,
                            notice = "Added $addedName",
                        )
                    }
                }
            }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (_: Exception) {
            mutableState.update { it.copy(error = ADD_ERROR) }
        } finally {
            finishAction()
        }
    }

    suspend fun leaveMesh() {
        if (!beginAction()) return
        try {
            withActiveNode { activeNode ->
                val notice = departureNotice(activeNode.leaveMesh())
                tracking.withLock {
                    peerSamples = emptyList()
                    ticketExpiresAtMillis = null
                    mutableState.update {
                        it.copy(
                            meshName = null,
                            meshId = null,
                            peers = emptyList(),
                            invitation = null,
                            invitationSecondsRemaining = 0,
                            error = null,
                            notice = notice,
                        )
                    }
                }
            }
            try {
                withActiveNode(cancellable = true) { activeNode ->
                    offerTicket(activeNode, mutableState.value.notice)
                }
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (_: Exception) {
                mutableState.update { it.copy(error = TICKET_ERROR) }
            }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (_: Exception) {
            mutableState.update { it.copy(error = LEAVE_ERROR, notice = null) }
        } finally {
            finishAction()
        }
    }

    fun reportError(message: String) {
        val safeMessage = message.trim().takeIf { it.isNotEmpty() && !it.contains(TICKET_PREFIX) } ?: "Operation failed"
        mutableState.update { it.copy(error = safeMessage, notice = null) }
    }

    private suspend fun offerTicket(activeNode: MeshNode, notice: String?) {
        val generationStartedAt = nowMillis()
        val invitation = activeNode.pair()
        val expiresAt = generationStartedAt + invitation.lifetimeSeconds.coerceAtLeast(0) * 1_000L
        tracking.withLock {
            ticketExpiresAtMillis = expiresAt
            offeredInitialTicket = true
            mutableState.update {
                it.copy(
                    invitation = invitation,
                    invitationSecondsRemaining = remainingSeconds(expiresAt),
                    error = null,
                    notice = notice,
                )
            }
        }
    }

    private fun departureNotice(left: LeftMesh): String = when {
        left.remainingMembers <= 0 -> "Left ${left.meshName}"
        left.notifiedMembers >= left.remainingMembers -> "Left ${left.meshName} and notified its other devices"
        else -> "Left ${left.meshName}. Notified ${left.notifiedMembers} of ${left.remainingMembers} devices; notified devices relay the departure; the rest can also learn it when they next reach this device"
    }

    private suspend fun openNode(): Boolean {
        currentCoroutineContext().ensureActive()
        try {
            withContext(NonCancellable) {
                val opened = nodeFactory()
                operations.withLock { node = opened }
            }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (_: Exception) {
            mutableState.update { it.copy(loading = false, error = OPEN_ERROR) }
            return false
        }
        return true
    }

    private suspend fun closeNode() {
        operations.withLock {
            val closing = node
            node = null
            try {
                closing?.shutdown()
            } catch (_: Exception) {
            }
        }
        tracking.withLock {
            peerSamples = emptyList()
            ticketExpiresAtMillis = null
            mutableState.update {
                it.copy(
                    loading = false,
                    peers = emptyList(),
                    invitation = null,
                    invitationSecondsRemaining = 0,
                    busy = false,
                    notice = null,
                )
            }
        }
    }

    private suspend fun poll() {
        val offerInitialTicket = try {
            operations.withLock {
                currentCoroutineContext().ensureActive()
                val activeNode = node ?: return
                val snapshot = withContext(NonCancellable) { activeNode.status() }
                acceptSnapshot(snapshot)
            }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (_: Exception) {
            mutableState.update {
                if (it.error == null || it.error == POLL_ERROR) it.copy(loading = false, error = POLL_ERROR) else it.copy(loading = false)
            }
            return
        }
        if (offerInitialTicket) refreshTicket()
    }

    private suspend fun acceptSnapshot(snapshot: NodeStatus): Boolean {
        val observedAtMillis = nowMillis()
        val samples = snapshot.peers
            .filter { it.id != snapshot.id }
            .distinctBy { it.id }
            .map { PeerSample(it.id, it.name, it.lastReceivedAgoMs, observedAtMillis) }
        return tracking.withLock {
            peerSamples = samples
            val joinedMesh = recordMeshMembership(snapshot.meshName)
            mutableState.update {
                it.copy(
                    loading = false,
                    nodeId = snapshot.id,
                    name = snapshot.name,
                    meshId = snapshot.meshId,
                    peers = devicesAt(observedAtMillis, samples),
                    error = if (it.error == POLL_ERROR) null else it.error,
                    notice = if (joinedMesh) "Joined ${snapshot.meshName}" else it.notice,
                )
            }
            !offeredInitialTicket
        }
    }

    private fun recordMeshMembership(membership: String?): Boolean {
        val enteredMesh = state.value.meshName == null && membership != null
        if (enteredMesh) ticketExpiresAtMillis = null
        mutableState.update {
            if (enteredMesh) {
                it.copy(meshName = membership, invitation = null, invitationSecondsRemaining = 0)
            } else {
                it.copy(meshName = membership)
            }
        }
        return enteredMesh
    }

    private suspend fun ageState() {
        while (true) {
            delay(POLL_INTERVAL_MILLIS)
            tracking.withLock {
                val now = nowMillis()
                val expiresAt = ticketExpiresAtMillis
                val invitationExpired = expiresAt != null && now >= expiresAt
                if (invitationExpired) ticketExpiresAtMillis = null
                mutableState.update {
                    it.copy(
                        peers = devicesAt(now, peerSamples),
                        invitation = if (invitationExpired) null else it.invitation,
                        invitationSecondsRemaining = if (invitationExpired || expiresAt == null) 0 else remainingSeconds(expiresAt),
                    )
                }
            }
        }
    }

    private suspend fun beginAction(): Boolean {
        currentCoroutineContext().ensureActive()
        if (!actions.tryLock()) return false
        mutableState.update { it.copy(busy = true, error = null, notice = null) }
        return true
    }

    private fun finishAction() {
        mutableState.update { it.copy(busy = false) }
        actions.unlock()
    }

    private suspend fun <T> withActiveNode(cancellable: Boolean = false, block: suspend (MeshNode) -> T): T {
        currentCoroutineContext().ensureActive()
        return operations.withLock {
            currentCoroutineContext().ensureActive()
            val activeNode = checkNotNull(node) { "PairingSession node is not open" }
            if (cancellable) block(activeNode) else withContext(NonCancellable) { block(activeNode) }
        }
    }
    private fun devicesAt(now: Long, peers: Collection<PeerSample>): List<DeviceStatus> = peers.map { peer ->
        DeviceStatus(peer.id, peer.name, receivedWithinPresenceWindow(peer, now))
    }

    private fun receivedWithinPresenceWindow(peer: PeerSample, now: Long): Boolean {
        val receivedAgoMs = peer.receivedAgoMs ?: return false
        if (receivedAgoMs < 0 || receivedAgoMs >= PRESENCE_LIMIT_MILLIS) return false
        return elapsedSince(peer.observedAtMillis, now) < PRESENCE_LIMIT_MILLIS - receivedAgoMs
    }

    private fun elapsedSince(observedAtMillis: Long, now: Long): Long {
        if (now <= observedAtMillis) return 0
        val elapsed = now - observedAtMillis
        return if (elapsed < 0) Long.MAX_VALUE else elapsed
    }

    private fun remainingSeconds(expiresAtMillis: Long): Int =
        max(0, ((expiresAtMillis - nowMillis()) / 1_000L).toInt())

    private fun isTicketSyntax(ticket: String): Boolean =
        ticket.encodeToByteArray().size <= MAX_TICKET_BYTES &&
            ticket.length > TICKET_PREFIX.length &&
            ticket.startsWith(TICKET_PREFIX) &&
            ticket.drop(TICKET_PREFIX.length).all { it.isAsciiTicketCharacter() }

    private fun Char.isAsciiTicketCharacter(): Boolean =
        this in 'A'..'Z' || this in 'a'..'z' || this in '0'..'9' || this == '-' || this == '_'

    private companion object {
        const val POLL_INTERVAL_MILLIS = 1_000L
        const val PRESENCE_LIMIT_MILLIS = 60_000L
        const val MAX_TICKET_BYTES = 8_192
        const val TICKET_PREFIX = "spirit1"
        const val OPEN_ERROR = "Could not open node"
        const val POLL_ERROR = "Could not read node status"
        const val TICKET_ERROR = "Could not create pairing ticket"
        const val INVALID_TICKET_ERROR = "Enter a valid pairing ticket"
        const val OWN_TICKET_ERROR = "This pairing ticket belongs to this device"
        const val ADD_ERROR = "Could not add device"
        const val LEAVE_ERROR = "Could not leave mesh"
    }
}
