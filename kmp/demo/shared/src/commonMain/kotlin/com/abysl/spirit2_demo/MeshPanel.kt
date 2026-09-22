package com.abysl.spirit2_demo

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlin.math.floor

@Composable
fun MeshPanel(node: MeshNode) {
    val scope = rememberCoroutineScope()
    var snapshot by remember { mutableStateOf<MeshSnapshot?>(null) }
    var pollError by remember { mutableStateOf<String?>(null) }
    var actionError by remember { mutableStateOf<String?>(null) }
    var message by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    var meshName by remember { mutableStateOf("personal") }
    var ticket by remember { mutableStateOf("") }
    var invitation by remember { mutableStateOf<MeshInvitation?>(null) }

    LaunchedEffect(node) {
        while (true) {
            try {
                val next = node.status()
                if (snapshot?.meshName == null && next.meshName != null && invitation != null) {
                    invitation = null
                    message = "Joined ${next.meshName}"
                }
                snapshot = next
                pollError = null
            } catch (cancelled: CancellationException) { throw cancelled }
            catch (error: Exception) { pollError = error.message ?: "Could not read node status" }
            delay(1000)
        }
    }

    fun attempt(block: suspend () -> Unit) {
        if (busy) return
        busy = true
        actionError = null
        message = null
        scope.launch {
            try { block(); snapshot = node.status() }
            catch (cancelled: CancellationException) { throw cancelled }
            catch (error: Exception) { actionError = error.message ?: "Operation failed" }
            finally { busy = false }
        }
    }

    fun enroll(value: String) = attempt {
        message = "Added ${node.add(value.trim())}"
        ticket = ""
    }

    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text(snapshot?.name ?: "Starting device…", style = MaterialTheme.typography.headlineSmall)
        Text(snapshot?.meshName?.let { "Mesh: $it" } ?: "This device has not joined a mesh")
        val ready = snapshot != null && pollError == null && !busy
        if (snapshot?.meshName == null) {
            Text("Create a mesh to invite devices, or show your QR code for an existing member to scan.")
            OutlinedTextField(meshName, { meshName = it }, label = { Text("Mesh name") }, singleLine = true, modifier = Modifier.fillMaxWidth())
            Button(enabled = ready && meshName.isNotBlank(), onClick = { attempt { node.createMesh(meshName.trim()); message = "Mesh created" } }) {
                Text("Create mesh")
            }
        }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            OutlinedButton(enabled = ready, onClick = { attempt { invitation = node.pair() } }) { Text("Show pairing QR") }
            if (snapshot?.meshName != null) {
                ScanPairingButton(ready, ::enroll) { actionError = it }
            }
        }
        if (snapshot?.meshName != null) {
            OutlinedTextField(ticket, { ticket = it }, label = { Text("Or paste a pairing ticket") }, maxLines = 3, modifier = Modifier.fillMaxWidth())
            Button(enabled = ready && ticket.isNotBlank(), onClick = { enroll(ticket) }) { Text("Add device") }
            HorizontalDivider()
            Text("Devices", style = MaterialTheme.typography.titleLarge)
            Text("Heartbeats every 5 seconds. Devices turn red without a valid ping or pong for 60 seconds.", style = MaterialTheme.typography.bodySmall)
            if (snapshot?.peers.isNullOrEmpty()) { Text("No other devices yet. Scan a device's pairing QR to add it.") }
            snapshot?.peers?.sortedWith(compareBy<MeshPeer> { it.name }.thenBy { it.id })?.forEach { peer ->
                key(peer.id) {
                    PeerRow(peer, ready, pollError != null, snapshot!!.peers.count { it.name == peer.name } > 1) {
                        attempt { message = node.ping(peer.id) }
                    }
                }
            }
        }
        if (busy) { LinearProgressIndicator(modifier = Modifier.fillMaxWidth()) }
        message?.let { Text(it, color = MaterialTheme.colorScheme.primary) }
        (actionError ?: pollError)?.let { Text(it, color = MaterialTheme.colorScheme.error) }
    }
    invitation?.let { code -> PairingDialog(code) { invitation = null } }
}

@Composable
private fun PeerRow(peer: MeshPeer, enabled: Boolean, statusUnavailable: Boolean, duplicateName: Boolean, ping: () -> Unit) {
    val connected = peer.connected && !statusUnavailable
    Card(modifier = Modifier.fillMaxWidth()) {
        Row(Modifier.padding(12.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Box(Modifier.size(12.dp).background(if (connected) Color(0xFF218A45) else Color(0xFFC63737), CircleShape)
                .semantics { contentDescription = if (connected) "Connected" else "Disconnected" })
            Column(Modifier.weight(1f)) {
                Text(peer.name, style = MaterialTheme.typography.titleMedium)
                if (duplicateName) Text(peer.id.take(12), style = MaterialTheme.typography.bodySmall)
                Text(if (statusUnavailable) "Disconnected · status unavailable" else peer.connectionLabel(), style = MaterialTheme.typography.bodySmall)
            }
            OutlinedButton(onClick = ping, enabled = enabled) { Text("Ping") }
        }
    }
}

@Composable
private fun PairingDialog(code: MeshInvitation, dismiss: () -> Unit) {
    var remaining by remember(code) { mutableIntStateOf(code.lifetimeSeconds) }
    LaunchedEffect(code) { while (remaining > 0) { delay(1000); remaining-- } }
    AlertDialog(
        onDismissRequest = dismiss,
        title = { Text("Pair this device") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("Scan from a device already in your mesh. This code can be used once.")
                if (remaining > 0) {
                    Canvas(Modifier.fillMaxWidth().aspectRatio(1f).background(Color.White)) {
                        val unit = floor(size.minDimension / (code.width + 8))
                        val marginX = (size.width - code.width * unit) / 2
                        val marginY = (size.height - code.width * unit) / 2
                        code.modules.forEachIndexed { index, value ->
                            if (value.toInt() != 0) drawRect(Color.Black,
                                Offset(marginX + (index % code.width) * unit, marginY + (index / code.width) * unit), Size(unit, unit))
                        }
                    }
                    Text("Expires in ${remaining / 60}:${(remaining % 60).toString().padStart(2, '0')}")
                    OutlinedTextField(code.ticket, {}, readOnly = true, label = { Text("Ticket for CLI pairing") }, maxLines = 2)
                } else { Text("Code expired. Close this window and generate a new one.") }
            }
        },
        confirmButton = { TextButton(onClick = dismiss) { Text("Close") } },
    )
}
