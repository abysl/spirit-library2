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
import blue.rae.spirit.sdk.AddDeviceResult
import blue.rae.spirit.sdk.MeshSession
import blue.rae.spirit.sdk.MemberStatus
import blue.rae.spirit.sdk.PairingInvitation
import kotlinx.coroutines.launch
import kotlin.math.floor

@Composable
fun MeshPanel(session: MeshSession) {
    val snapshot by session.state.collectAsState()
    val scope = rememberCoroutineScope()
    var groupName by remember { mutableStateOf("") }
    val tickets = remember { mutableStateMapOf<String, String>() }
    var selectedGroup by remember { mutableStateOf<String?>(null) }
    var showQr by remember { mutableStateOf(false) }

    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text(snapshot.name.ifBlank { "Starting device…" }, style = MaterialTheme.typography.headlineSmall)
        val ready = !snapshot.loading && !snapshot.busy
        Text("Groups", style = MaterialTheme.typography.titleLarge)
        OutlinedTextField(groupName, { groupName = it }, label = { Text("New group name") }, singleLine = true, modifier = Modifier.fillMaxWidth())
        Button(enabled = ready && groupName.isNotBlank(), onClick = {
            scope.launch { if (session.createGroup(groupName) != null) groupName = "" }
        }) { Text("Create group") }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            OutlinedButton(enabled = ready && snapshot.invitation != null, onClick = { showQr = true }) { Text("Show my QR") }
            OutlinedButton(enabled = ready, onClick = { scope.launch { session.refreshTicket() } }) { Text("Refresh QR") }
        }
        if (snapshot.groups.isEmpty()) Text("Create a group, or show your QR to a member of another group.")
        snapshot.groups.forEach { group ->
            key(group.id) {
                HorizontalDivider()
                Text(group.name, style = MaterialTheme.typography.titleLarge)
                Text(group.id.take(16), style = MaterialTheme.typography.bodySmall)
                if (group.members.isEmpty()) Text("No members yet")
                group.members.sortedWith(compareBy<MemberStatus> { it.name.lowercase() }.thenBy { it.id }).forEach { member ->
                    PeerRow(member, ready && member.id != snapshot.nodeId,
                        group.members.count { it.name == member.name } > 1) {
                        scope.launch { session.ping(member.id) }
                    }
                }
                OutlinedTextField(tickets[group.id].orEmpty(), { tickets[group.id] = it }, label = { Text("Paste a device ticket") }, maxLines = 3, modifier = Modifier.fillMaxWidth())
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Button(enabled = ready && !tickets[group.id].isNullOrBlank(), onClick = {
                        scope.launch { if (session.addDevice(group.id, tickets[group.id].orEmpty()) is AddDeviceResult.Added) tickets[group.id] = "" }
                    }) { Text("Add device") }
                    ScanPairingButton(ready, { value -> scope.launch { session.addDevice(group.id, value) } }, session::reportError)
                }
                OutlinedButton(enabled = ready, onClick = { selectedGroup = group.id }) { Text("Leave group") }
            }
        }
        if (snapshot.busy) LinearProgressIndicator(modifier = Modifier.fillMaxWidth())
        snapshot.notice?.let { Text(it, color = MaterialTheme.colorScheme.primary) }
        snapshot.error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
    }
    selectedGroup?.let { id ->
        AlertDialog(onDismissRequest = { selectedGroup = null }, title = { Text("Leave group?") },
            text = { Text("This device will leave the selected group. Other members may learn about the departure when they reconnect.") },
            confirmButton = { TextButton(onClick = { selectedGroup = null; scope.launch { session.leaveGroup(id) } }) { Text("Leave") } },
            dismissButton = { TextButton(onClick = { selectedGroup = null }) { Text("Cancel") } })
    }
    if (showQr) snapshot.invitation?.let { invitation ->
        PairingDialog(invitation, snapshot.invitationSecondsRemaining) { showQr = false }
    }
}

@Composable
private fun PeerRow(member: MemberStatus, enabled: Boolean, duplicateName: Boolean, ping: () -> Unit) {
    Card(modifier = Modifier.fillMaxWidth()) {
        Row(Modifier.padding(12.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Box(Modifier.size(12.dp).background(if (member.online) Color(0xFF218A45) else Color(0xFFC63737), CircleShape)
                .semantics { contentDescription = if (member.online) "Online" else "Offline" })
            Column(Modifier.weight(1f)) {
                Text(member.name, style = MaterialTheme.typography.titleMedium)
                if (duplicateName) Text(member.id.take(12), style = MaterialTheme.typography.bodySmall)
                Text(if (member.online) "Online" else "Offline", style = MaterialTheme.typography.bodySmall)
            }
            OutlinedButton(onClick = ping, enabled = enabled) { Text("Ping") }
        }
    }
}

@Composable
private fun PairingDialog(code: PairingInvitation, remaining: Int, dismiss: () -> Unit) {
    AlertDialog(
        onDismissRequest = dismiss,
        title = { Text("Pair this device") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("A member chooses which group to add this device to. This code can be used once.")
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
