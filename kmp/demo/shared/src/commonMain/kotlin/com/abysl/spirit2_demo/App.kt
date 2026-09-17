package com.abysl.spirit2_demo

import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeContentPadding
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.tooling.preview.Preview
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch

@Composable
@Preview
fun App(store: BlobStore? = null, node: MeshNode? = null) {
    var showDevices by remember { mutableStateOf(true) }
    MaterialTheme {
        Column(
            modifier = Modifier
                .background(MaterialTheme.colorScheme.primaryContainer)
                .safeContentPadding()
                .fillMaxSize()
                .padding(16.dp)
                .verticalScroll(rememberScrollState()),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text("Spirit", style = MaterialTheme.typography.titleMedium)
            Spacer(Modifier.height(16.dp))
            Row {
                TextButton(onClick = { showDevices = true }) { Text("Devices") }
                TextButton(onClick = { showDevices = false }) { Text("Blobs") }
            }
            if (showDevices) {
                if (node == null) Text("Device pairing is available on Android and desktop")
                else MeshPanel(node)
            } else if (store == null) {
                Text("spirit is not available on this platform")
            } else {
                SpiritPanel(store)
            }
        }
    }
}

@Composable
private fun SpiritPanel(store: BlobStore) {
    val scope = rememberCoroutineScope()
    var text by remember { mutableStateOf("hello, spirit") }
    var hash by remember { mutableStateOf("") }
    var fetched by remember { mutableStateOf<String?>(null) }
    var error by remember { mutableStateOf<String?>(null) }

    fun attempt(block: suspend () -> Unit) = scope.launch {
        error = null
        try {
            block()
        } catch (e: Exception) {
            error = e.message ?: e.toString()
        }
    }

    Column(modifier = Modifier.fillMaxWidth()) {
        OutlinedTextField(
            value = text,
            onValueChange = { text = it },
            label = { Text("bytes to store") },
            modifier = Modifier.fillMaxWidth(),
        )
        Spacer(Modifier.height(8.dp))
        Row {
            Button(onClick = { attempt { hash = store.put(text.encodeToByteArray()) } }) {
                Text("put")
            }
            Spacer(Modifier.width(8.dp))
            Button(
                onClick = { attempt { fetched = store.get(hash).decodeToString() } },
                enabled = hash.isNotEmpty(),
            ) {
                Text("get")
            }
        }
        Spacer(Modifier.height(8.dp))
        OutlinedTextField(
            value = hash,
            onValueChange = { hash = it.trim() },
            label = { Text("blob hash") },
            modifier = Modifier.fillMaxWidth(),
        )
        fetched?.let {
            Spacer(Modifier.height(8.dp))
            Text("got back: $it")
        }
        error?.let {
            Spacer(Modifier.height(8.dp))
            Text(it, color = MaterialTheme.colorScheme.error)
        }
    }
}
