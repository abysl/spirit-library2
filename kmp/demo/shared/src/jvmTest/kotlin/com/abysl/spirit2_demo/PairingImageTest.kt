package com.abysl.spirit2_demo

import com.google.zxing.BarcodeFormat
import com.google.zxing.MultiFormatWriter
import com.google.zxing.client.j2se.MatrixToImageWriter
import java.nio.file.Files
import java.awt.image.BufferedImage
import javax.imageio.ImageIO
import blue.rae.spirit.sdk.SpiritNode
import kotlinx.coroutines.runBlocking
import kotlin.test.Test
import kotlin.test.assertEquals

class PairingImageTest {
    @Test
    fun qr_image_import_preserves_the_ticket() {
        val ticket = "spirit1-test-ticket-from-cli"
        val image = Files.createTempFile("spirit-qr", ".png")
        try {
            val matrix = MultiFormatWriter().encode(ticket, BarcodeFormat.QR_CODE, 512, 512)
            MatrixToImageWriter.writeToPath(matrix, "PNG", image)
            assertEquals(ticket, decodePairingImage(image.toFile()))
        } finally { Files.deleteIfExists(image) }
    }
    @Test
    fun native_pairing_qr_can_be_scanned_and_redeemed() = runBlocking {
        val inviterDir = Files.createTempDirectory("spirit-qr-inviter")
        val joiningDir = Files.createTempDirectory("spirit-qr-joining")
        val imagePath = Files.createTempFile("spirit-native-qr", ".png")
        val inviter = SpiritNode.open(inviterDir.toString(), "desktop", local = true)
        val joining = SpiritNode.open(joiningDir.toString(), "phone", local = true)
        try {
            inviter.createMesh("personal")
            val code = joining.pair()
            val scale = 6
            val pixels = (code.width + 8) * scale
            val image = BufferedImage(pixels, pixels, BufferedImage.TYPE_INT_RGB)
            for (y in 0 until pixels) for (x in 0 until pixels) {
                val column = x / scale - 4
                val row = y / scale - 4
                val dark = column in 0 until code.width && row in 0 until code.width &&
                    code.modules[row * code.width + column].toInt() != 0
                image.setRGB(x, y, if (dark) 0 else 0xFFFFFF)
            }
            ImageIO.write(image, "PNG", imagePath.toFile())
            val scanned = decodePairingImage(imagePath.toFile())
            assertEquals(code.ticket, scanned)
            assertEquals("phone", inviter.add(scanned))
            assertEquals("phone", inviter.ping("phone").name)
        } finally {
            inviter.close()
            joining.close()
            Files.deleteIfExists(imagePath)
        }
    }
}
