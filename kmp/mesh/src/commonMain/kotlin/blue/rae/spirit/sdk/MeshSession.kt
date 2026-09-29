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
data class MemberStatus(val id: String, val name: String, val online: Boolean, val generation: Long)
data class GroupState(val id: String, val name: String, val members: List<MemberStatus>)

sealed interface AddDeviceResult {
    data class Added(val name: String) : AddDeviceResult
    data class Failed(val failure: MeshFailure) : AddDeviceResult
}

private val monotonicOrigin = TimeSource.Monotonic.markNow()

data class MeshState(
    val loading: Boolean = true,
    val nodeId: String = "",
    val name: String = "",
    val groups: List<GroupState> = emptyList(),
    val peers: List<DeviceStatus> = emptyList(),
    val invitation: PairingInvitation? = null,
    val invitationSecondsRemaining: Int = 0,
    val busy: Boolean = false,
    val error: String? = null,
    val failure: MeshFailure? = null,
    val notice: String? = null,
    val joinedGroupIds: List<String> = emptyList(),
)

class MeshSession(
    private val nodeFactory: suspend () -> MeshNode,
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
    private val mutableState = MutableStateFlow(MeshState())
    private var node: MeshNode? = null
    private var started = false
    private var offeredInitialTicket = false
    private var needsTicket = false
    private var ticketExpiresAtMillis: Long? = null
    private var peerSamples: List<PeerSample> = emptyList()
    private var meshSamples: List<MeshStatus> = emptyList()

    val state: StateFlow<MeshState> = mutableState.asStateFlow()

    suspend fun run() {
        operations.withLock {
            check(!started) { "MeshSession.run may only be called once" }
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
        beginAction()
        try {
            withActiveNode(cancellable = true) { offerTicket(it) }
            tracking.withLock { needsTicket = false }
            mutableState.update { it.copy(error = null, failure = null) }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (error: Exception) {
            mutableState.update { it.copy(error = TICKET_ERROR, failure = failureOf(error)) }
        } finally {
            finishAction()
        }
    }

    suspend fun createGroup(name: String): String? {
        var createdId: String? = null
        beginAction()
        try {
            val groupName = name.trim()
            if (groupName.isEmpty()) {
                mutableState.update { it.copy(error = GROUP_NAME_ERROR, failure = MeshFailure.Invalid, notice = null) }
                return null
            }
            withActiveNode { activeNode ->
                val before = activeNode.status()
                acceptSnapshot(before)
                val wasUnenrolled = before.meshes.isEmpty()
                createdId = activeNode.createMesh(groupName)
                val id = checkNotNull(createdId)
                tracking.withLock {
                    if (wasUnenrolled) {
                        clearInvitation()
                        needsTicket = true
                    }
                    meshSamples = meshSamples + MeshStatus(id, groupName, emptyList())
                    mutableState.update {
                        it.copy(groups = groupsAt(nowMillis()), error = null, failure = null, notice = combineNotice(it.notice, "Created $groupName"))
                    }
                }
            }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (error: Exception) {
            mutableState.update { it.copy(error = actionError(error, CREATE_ERROR), failure = failureOf(error), notice = null) }
        } finally {
            finishAction()
        }
        offerPendingTicket()
        return createdId
    }

    suspend fun addDevice(meshId: String, value: String): AddDeviceResult {
        var result: AddDeviceResult = AddDeviceResult.Failed(MeshFailure.Node)
        beginAction()
        try {
            val ticket = value.trim()
            when {
                !isTicketSyntax(ticket) -> {
                    mutableState.update { it.copy(error = INVALID_TICKET_ERROR, failure = MeshFailure.Invalid, notice = null) }
                    return AddDeviceResult.Failed(MeshFailure.Invalid)
                }
                state.value.invitation?.ticket == ticket -> {
                    mutableState.update { it.copy(error = OWN_TICKET_ERROR, failure = MeshFailure.TicketRejected, notice = null) }
                    return AddDeviceResult.Failed(MeshFailure.TicketRejected)
                }
                state.value.groups.none { it.id == meshId } -> {
                    mutableState.update { it.copy(error = GROUP_NOT_FOUND_ERROR, failure = MeshFailure.NotMember, notice = null) }
                    return AddDeviceResult.Failed(MeshFailure.NotMember)
                }
            }
            withActiveNode { activeNode ->
                val addedName = activeNode.add(meshId, ticket)
                result = AddDeviceResult.Added(addedName)
                mutableState.update { it.copy(error = null, failure = null, notice = combineNotice(it.notice, "Added " + addedName)) }
                try {
                    acceptSnapshot(activeNode.status())
                } catch (_: Exception) {
                }
            }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (error: Exception) {
            val failure = failureOf(error)
            result = AddDeviceResult.Failed(failure)
            mutableState.update { it.copy(error = actionError(error, ADD_ERROR), failure = failure, notice = null) }
        } finally {
            finishAction()
        }
        offerPendingTicket()
        return result
    }

    suspend fun leaveGroup(meshId: String): LeftMesh? {
        var left: LeftMesh? = null
        beginAction()
        try {
            if (state.value.groups.none { it.id == meshId }) {
                mutableState.update { it.copy(error = GROUP_NOT_FOUND_ERROR, failure = MeshFailure.NotMember, notice = null) }
                return null
            }
            withActiveNode { activeNode ->
                left = activeNode.leaveMesh(meshId)
                val notice = departureNotice(checkNotNull(left))
                tracking.withLock {
                    meshSamples = meshSamples.filterNot { it.id == meshId }
                    clearInvitation()
                    needsTicket = true
                    mutableState.update {
                        it.copy(groups = it.groups.filterNot { group -> group.id == meshId }, error = null, failure = null, notice = combineNotice(it.notice, notice))
                    }
                }
            }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (error: Exception) {
            mutableState.update { it.copy(error = actionError(error, LEAVE_ERROR), failure = failureOf(error), notice = null) }
        } finally {
            finishAction()
        }
        offerPendingTicket()
        return left
    }

    suspend fun ping(deviceId: String) {
        beginAction()
        try {
            withActiveNode { activeNode ->
                val pong = activeNode.ping(deviceId)
                mutableState.update { it.copy(notice = combineNotice(it.notice, "Pong from " + pong.name), error = null, failure = null) }
            }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (error: Exception) {
            mutableState.update { it.copy(error = actionError(error, "Could not ping device"), failure = failureOf(error), notice = null) }
        } finally {
            finishAction()
        }
    }

    fun reportError(message: String) {
        val safeMessage = message.trim().takeIf { it.isNotEmpty() && !it.contains(TICKET_PREFIX, ignoreCase = true) } ?: "Operation failed"
        mutableState.update { it.copy(error = safeMessage, failure = null, notice = null) }
    }

    fun clearMessages() {
        mutableState.update { it.copy(notice = null, error = null, failure = null, joinedGroupIds = emptyList()) }
    }

    private suspend fun offerTicket(activeNode: MeshNode) {
        val generationStartedAt = nowMillis()
        val invitation = try { activeNode.pair().also { currentCoroutineContext().ensureActive() } } catch (cancelled: CancellationException) {
            withContext(NonCancellable) { tracking.withLock { clearInvitation(); needsTicket = true } }
            throw cancelled
        }
        val expiresAt = generationStartedAt + invitation.lifetimeSeconds.coerceAtLeast(0) * 1_000L
        tracking.withLock {
            ticketExpiresAtMillis = expiresAt
            offeredInitialTicket = true
            mutableState.update {
                it.copy(
                    invitation = invitation,
                    invitationSecondsRemaining = remainingSeconds(expiresAt),
                    error = if (it.error == TICKET_ERROR) null else it.error,
                    failure = if (it.error == TICKET_ERROR) null else it.failure,

                )
            }
        }
    }

    private suspend fun offerPendingTicket() {
        actions.withLock {
            val offer = tracking.withLock {
                val pending = needsTicket
                needsTicket = false
                pending
            }
            if (!offer) return@withLock
            try {
                withActiveNode(cancellable = true) { offerTicket(it) }
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (error: Exception) {
                mutableState.update { if (it.error == null) it.copy(error = TICKET_ERROR, failure = failureOf(error)) else it }
            }
        }
    }

    private fun failureOf(error: Exception): MeshFailure = (error as? MeshNodeException)?.failure ?: MeshFailure.Node

    private fun combineNotice(current: String?, action: String): String =
        if (current?.startsWith("Joined ") == true) "$current; $action" else action

    private fun actionError(error: Exception, fallback: String): String =
        if (error is MeshNodeException && error.failure == MeshFailure.MeshLimit) GROUP_LIMIT_ERROR else fallback

    private fun clearInvitation() {
        ticketExpiresAtMillis = null
        mutableState.update { it.copy(invitation = null, invitationSecondsRemaining = 0) }
    }

    private fun departureNotice(left: LeftMesh): String = when {
        left.remainingMembers <= 0 -> "Left ${left.meshName}"
        left.notifiedMembers >= left.remainingMembers -> "Left ${left.meshName} and notified its other devices"
        else -> {
            val devices = if (left.remainingMembers == 1) "device" else "devices"
            val relay = if (left.notifiedMembers == 0) "" else " notified devices relay the departure;"
            "Left ${left.meshName}. Notified ${left.notifiedMembers} of ${left.remainingMembers} $devices;$relay the rest can also learn it when they next reach this device"
        }
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
        } catch (error: Exception) {
            mutableState.update { it.copy(loading = false, error = OPEN_ERROR, failure = failureOf(error)) }
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
            meshSamples = emptyList()
            needsTicket = false
            ticketExpiresAtMillis = null
            mutableState.update {
                it.copy(
                    loading = false,
                    peers = emptyList(),
                    groups = emptyList(),
                    invitation = null,
                    invitationSecondsRemaining = 0,
                    busy = false,
                    notice = null,
                    joinedGroupIds = emptyList(),
                )
            }
        }
    }

    private suspend fun poll() {
        try {
            operations.withLock {
                currentCoroutineContext().ensureActive()
                val activeNode = node ?: return
                val snapshot = withContext(NonCancellable) { activeNode.status() }
                acceptSnapshot(snapshot)
            }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (error: Exception) {
            mutableState.update {
                if (it.error == null || it.error == POLL_ERROR) it.copy(loading = false, error = POLL_ERROR, failure = failureOf(error)) else it.copy(loading = false)
            }
            return
        }
        offerPendingTicket()
    }

    private suspend fun acceptSnapshot(snapshot: NodeStatus) {
        val observedAtMillis = nowMillis()
        val samples = snapshot.peers
            .filter { it.id != snapshot.id }
            .distinctBy { it.id }
            .map { PeerSample(it.id, it.name, it.lastReceivedAgoMs, observedAtMillis) }
        tracking.withLock {
            peerSamples = samples
            meshSamples = snapshot.meshes
            val newGroup = snapshot.meshes.firstOrNull { mesh -> state.value.groups.none { it.id == mesh.id } }
            val wasLoading = state.value.loading
            val consumed = state.value.invitation != null && !snapshot.ticketPending
            if (consumed) {
                clearInvitation()
                needsTicket = true
            }
            mutableState.update {
                it.copy(
                    loading = false,
                    nodeId = snapshot.id,
                    name = snapshot.name,
                    groups = groupsAt(observedAtMillis, snapshot.id),
                    peers = devicesAt(observedAtMillis, samples),
                    error = if (it.error == POLL_ERROR) null else it.error,
                    failure = if (it.error == POLL_ERROR) null else it.failure,
                    joinedGroupIds = if (newGroup != null && !wasLoading) it.joinedGroupIds + newGroup.id else it.joinedGroupIds,
                    notice = if (newGroup != null && !wasLoading) {
                        val joined = "Joined " + newGroup.name
                        if (it.notice?.startsWith("Added ") == true) "$joined; ${it.notice}" else joined
                    } else it.notice,
                )
            }
            if (!offeredInitialTicket) {
                offeredInitialTicket = true
                needsTicket = true
            }
        }
    }

    private fun groupsAt(now: Long, selfId: String = state.value.nodeId): List<GroupState> = meshSamples.sortedWith(compareBy<MeshStatus> { it.name.lowercase() }.thenBy { it.id }).map { mesh ->
        GroupState(mesh.id, mesh.name, mesh.members.map { member ->
            val peer = peerSamples.firstOrNull { it.id == member.id }
            MemberStatus(member.id, member.name,
                member.id == selfId || (peer != null && receivedWithinPresenceWindow(peer, now)),
                member.generation)
        })
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
                        groups = groupsAt(now),
                        invitation = if (invitationExpired) null else it.invitation,
                        invitationSecondsRemaining = if (invitationExpired || expiresAt == null) 0 else remainingSeconds(expiresAt),
                    )
                }
            }
        }
    }

    private suspend fun beginAction() {
        actions.lock()
        mutableState.update { it.copy(busy = true) }
    }

    private fun finishAction() {
        mutableState.update { it.copy(busy = false) }
        actions.unlock()
    }

    private suspend fun <T> withActiveNode(cancellable: Boolean = false, block: suspend (MeshNode) -> T): T {
        currentCoroutineContext().ensureActive()
        return operations.withLock {
            currentCoroutineContext().ensureActive()
            val activeNode = checkNotNull(node) { "MeshSession node is not open" }
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
        const val CREATE_ERROR = "Could not create group"
        const val GROUP_LIMIT_ERROR = "This device has reached the 64-group limit"
        const val GROUP_NAME_ERROR = "Enter a group name"
        const val GROUP_NOT_FOUND_ERROR = "Choose a group"
        const val INVALID_TICKET_ERROR = "Enter a valid pairing ticket"
        const val OWN_TICKET_ERROR = "This pairing ticket belongs to this device"
        const val ADD_ERROR = "Could not add device"
        const val LEAVE_ERROR = "Could not leave group"
    }
}
