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
    private val samples = Mutex()
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
        var opened: MeshNode? = null
        try {
            currentCoroutineContext().ensureActive()
            opened = withContext(NonCancellable) { nodeFactory() }
            withContext(NonCancellable) {
                operations.withLock {
                    node = opened
                    opened = null
                }
            }
            currentCoroutineContext().ensureActive()
            coroutineScope {
                launch { ageState() }
                while (true) {
                    poll()
                    delay(POLL_INTERVAL_MILLIS)
                }
            }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (_: Throwable) {
            mutableState.update { it.copy(loading = false, error = OPEN_ERROR) }
        } finally {
            withContext(NonCancellable) {
                operations.withLock {
                    val closing = node ?: opened
                    node = null
                    opened = null
                    runCatching { closing?.shutdown() }
                }
                samples.withLock { peerSamples = emptyList() }
                ticketExpiresAtMillis = null
                mutableState.update {
                    it.copy(
                        loading = false,
                        peers = emptyList(),
                        invitation = null,
                        invitationSecondsRemaining = 0,
                        busy = false,
                    )
                }
            }
        }
    }

    suspend fun refreshTicket() {
        if (!beginAction()) return
        try {
            val invitation = withActiveNode { activeNode ->
                val generationStartedAt = nowMillis()
                val created = activeNode.pair()
                created to generationStartedAt
            }
            val expiresAt = invitation.second + invitation.first.lifetimeSeconds.coerceAtLeast(0) * 1_000L
            ticketExpiresAtMillis = expiresAt
            offeredInitialTicket = true
            mutableState.update {
                it.copy(
                    invitation = invitation.first,
                    invitationSecondsRemaining = remainingSeconds(expiresAt),
                    error = null,
                    notice = null,
                )
            }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (_: Throwable) {
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
            val addedName = withActiveNode { activeNode ->
                if (activeNode.status().meshName == null) {
                    activeNode.createMesh(meshName)
                    mutableState.update { it.copy(meshName = meshName) }
                }
                activeNode.add(ticket)
            }
            ticketExpiresAtMillis = null
            mutableState.update {
                it.copy(
                    invitation = null,
                    invitationSecondsRemaining = 0,
                    error = null,
                    notice = "Added $addedName",
                )
            }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (_: Throwable) {
            mutableState.update { it.copy(error = ADD_ERROR) }
        } finally {
            finishAction()
        }
    }

    fun reportError(message: String) {
        val safeMessage = message.trim().takeIf { it.isNotEmpty() && !it.contains(TICKET_PREFIX) } ?: "Operation failed"
        mutableState.update { it.copy(error = safeMessage, notice = null) }
    }

    private suspend fun poll() {
        val offeredTicket = try {
            operations.withLock {
                currentCoroutineContext().ensureActive()
                val activeNode = node ?: return
                val snapshot = withContext(NonCancellable) { activeNode.status() }
                acceptSnapshot(snapshot)
                !offeredInitialTicket
            }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (_: Throwable) {
            mutableState.update {
                if (it.error == null || it.error == POLL_ERROR) it.copy(loading = false, error = POLL_ERROR) else it.copy(loading = false)
            }
            return
        }
        if (offeredTicket) refreshTicket()
    }

    private suspend fun acceptSnapshot(snapshot: NodeStatus) {
        val observedAtMillis = nowMillis()
        val distinctPeers = LinkedHashMap<String, PeerSample>()
        snapshot.peers.forEach { peer ->
            if (peer.id != snapshot.id && peer.id !in distinctPeers) {
                distinctPeers[peer.id] = PeerSample(peer.id, peer.name, peer.lastReceivedAgoMs, observedAtMillis)
            }
        }
        samples.withLock {
            peerSamples = distinctPeers.values.toList()
        }
        val joinedMesh = state.value.meshName == null && snapshot.meshName != null
        if (joinedMesh) ticketExpiresAtMillis = null
        mutableState.update {
            it.copy(
                loading = false,
                nodeId = snapshot.id,
                name = snapshot.name,
                meshName = snapshot.meshName,
                peers = devicesAt(observedAtMillis, distinctPeers.values),
                invitation = if (joinedMesh) null else it.invitation,
                invitationSecondsRemaining = if (joinedMesh) 0 else it.invitationSecondsRemaining,
                error = if (it.error == POLL_ERROR) null else it.error,
                notice = if (joinedMesh) "Joined ${snapshot.meshName}" else it.notice,
            )
        }
    }

    private suspend fun ageState() {
        while (true) {
            delay(POLL_INTERVAL_MILLIS)
            val currentSamples = samples.withLock { peerSamples }
            val now = nowMillis()
            val expiresAt = ticketExpiresAtMillis
            val invitationExpired = expiresAt != null && now >= expiresAt
            if (invitationExpired && ticketExpiresAtMillis == expiresAt) ticketExpiresAtMillis = null
            mutableState.update {
                it.copy(
                    peers = devicesAt(now, currentSamples),
                    invitation = if (invitationExpired) null else it.invitation,
                    invitationSecondsRemaining = if (invitationExpired) 0 else expiresAt?.let(::remainingSeconds) ?: 0,
                )
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

    private suspend fun <T> withActiveNode(block: suspend (MeshNode) -> T): T {
        currentCoroutineContext().ensureActive()
        return operations.withLock {
            currentCoroutineContext().ensureActive()
            val activeNode = node ?: throw IllegalStateException()
            withContext(NonCancellable) { block(activeNode) }
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
        const val ADD_ERROR = "Could not add device. Independent meshes cannot merge."
    }
}
