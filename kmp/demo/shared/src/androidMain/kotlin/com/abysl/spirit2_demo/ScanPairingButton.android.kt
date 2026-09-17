package com.abysl.spirit2_demo

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.compose.material3.Button
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import com.google.zxing.client.android.Intents
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions

@Composable
actual fun ScanPairingButton(enabled: Boolean, onTicket: (String) -> Unit, onError: (String) -> Unit) {
    val scanner = rememberLauncherForActivityResult(ScanContract()) { result ->
        result.contents?.let(onTicket)
        if (result.originalIntent?.getBooleanExtra(Intents.Scan.MISSING_CAMERA_PERMISSION, false) == true) {
            onError("Camera permission is required to scan. You can paste a pairing ticket instead.")
        }
    }
    Button(enabled = enabled, onClick = {
        try {
            scanner.launch(ScanOptions().setDesiredBarcodeFormats(ScanOptions.QR_CODE)
                .setPrompt("Scan a Spirit pairing code")
                .setBeepEnabled(false).setOrientationLocked(false))
        } catch (error: Exception) {
            onError(error.message ?: "Could not open the camera")
        }
    }) { Text("Scan QR code") }
}
