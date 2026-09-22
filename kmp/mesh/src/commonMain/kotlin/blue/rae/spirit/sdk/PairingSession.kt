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
            mutableState.update { it.copy(loading = false, error = "Could not open node") }
        } finally {
            withContext(NonCancellable) {
                operations.withLock {
                    val closing = node ?: opened
                    node = null
                    opened = null
                    runCatching { closing?.shutdown() }
                }
            }
        }
    }

    suspend fun refreshTicket() {
        perform("Could not create pairing ticket") { activeNode ->
            val generationStartedAt = nowMillis()
            val invitation = activeNode.pair()
            val expiresAt = generationStartedAt + invitation.lifetimeSeconds.coerceAtLeast(0) * 1_000L
            ticketExpiresAtMillis = expiresAt
            offeredInitialTicket = true
            mutableState.update {
                it.copy(
                    invitation = invitation,
                    invitationSecondsRemaining = remainingSeconds(expiresAt),
                    error = null,
                    notice = null,
                )
            }
        }
    }

    suspend fun pair(value: String) {
        val ticket = value.trim()
        when {
            !isTicketSyntax(ticket) -> {
                mutableState.update { it.copy(error = "Enter a valid pairing ticket", notice = null) }
                return
            }
            state.value.invitation?.ticket == ticket -> {
                mutableState.update { it.copy(error = "This pairing ticket belongs to this device", notice = null) }
                return
            }
        }
        perform("Could not add device. Independent meshes cannot merge.") { activeNode ->
            if (state.value.meshName == null) {
                activeNode.createMesh(meshName)
                mutableState.update { it.copy(meshName = meshName) }
            }
            val addedName = activeNode.add(ticket)
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

    fun reportError(message: String) {
        val safeMessage = message.trim().takeIf { it.isNotEmpty() && !it.contains("spirit1") } ?: "Operation failed"
        mutableState.update { it.copy(error = safeMessage, notice = null) }
    }

    private suspend fun poll() {
        val snapshot = try {
            withContext(NonCancellable) { operations.withLock { node?.status() } }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (_: Throwable) {
            mutableState.update { it.copy(loading = false, error = "Could not read node status") }
            return
        } ?: return
        acceptSnapshot(snapshot)
        if (!offeredInitialTicket) refreshTicket()
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
                error = null,
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
            if (invitationExpired) ticketExpiresAtMillis = null
            mutableState.update {
                it.copy(
                    peers = devicesAt(now, currentSamples),
                    invitation = if (invitationExpired) null else it.invitation,
                    invitationSecondsRemaining = if (invitationExpired) 0 else expiresAt?.let(::remainingSeconds) ?: 0,
                )
            }
        }
    }

    private suspend fun perform(error: String, block: suspend (MeshNode) -> Unit) {
        mutableState.update { it.copy(busy = true, error = null, notice = null) }
        try {
            withContext(NonCancellable) {
                operations.withLock {
                    val activeNode = node ?: throw IllegalStateException()
                    block(activeNode)
                }
            }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (_: Throwable) {
            mutableState.update { it.copy(error = error) }
        } finally {
            mutableState.update { it.copy(busy = false) }
        }
    }

    private fun devicesAt(now: Long, peers: Collection<PeerSample>): List<DeviceStatus> = peers.map { peer ->
        val age = peer.receivedAgoMs?.let { max(0, it) + max(0, now - peer.observedAtMillis) }
        DeviceStatus(peer.id, peer.name, age != null && age < PRESENCE_LIMIT_MILLIS)
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
    }
}
