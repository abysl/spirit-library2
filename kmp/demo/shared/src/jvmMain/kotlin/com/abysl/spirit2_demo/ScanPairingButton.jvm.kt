package com.abysl.spirit2_demo

import androidx.compose.material3.Button
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.rememberCoroutineScope
import com.google.zxing.BinaryBitmap
import com.google.zxing.MultiFormatReader
import com.google.zxing.client.j2se.BufferedImageLuminanceSource
import com.google.zxing.common.HybridBinarizer
import java.io.File
import javax.imageio.ImageIO
import javax.swing.JFileChooser
import javax.swing.filechooser.FileNameExtensionFilter
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

fun decodePairingImage(file: File): String {
    val image = ImageIO.read(file) ?: error("Choose a PNG or JPEG QR image")
    return MultiFormatReader().decode(BinaryBitmap(HybridBinarizer(BufferedImageLuminanceSource(image)))).text
}

@Composable
actual fun ScanPairingButton(enabled: Boolean, onTicket: (String) -> Unit, onError: (String) -> Unit) {
    val scope = rememberCoroutineScope()
    Button(enabled = enabled, onClick = {
        val chooser = JFileChooser().apply { fileFilter = FileNameExtensionFilter("QR images", "png", "jpg", "jpeg") }
        if (chooser.showOpenDialog(null) == JFileChooser.APPROVE_OPTION) {
            scope.launch {
                try { onTicket(withContext(Dispatchers.IO) { decodePairingImage(chooser.selectedFile) }) }
                catch (cancelled: CancellationException) { throw cancelled }
                catch (error: Exception) { onError(error.message ?: "No QR code found in this image") }
            }
        }
    }) { Text("Open QR image") }
}
