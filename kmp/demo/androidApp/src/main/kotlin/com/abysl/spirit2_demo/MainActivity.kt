package com.abysl.spirit2_demo

import android.app.Application
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.ViewModelProvider
import blue.rae.spirit.sdk.AndroidNodeContext
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch

class DeviceModel(application: Application) : AndroidViewModel(application) {
    init { AndroidNodeContext.initialize(application) }
    val store = openBlobStore(application.filesDir.resolve("spirit").path)
    val node = openMeshNode(application.noBackupFilesDir.resolve("spirit-node").path, Build.MODEL)

    override fun onCleared() {
        CoroutineScope(Dispatchers.IO).launch {
            try { node?.close() } finally { store?.close() }
        }
    }
}

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        val model = ViewModelProvider(this)[DeviceModel::class.java]
        setContent { App(model.store, model.node) }
    }
}
