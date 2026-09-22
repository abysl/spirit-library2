package blue.rae.spirit.sdk

import kotlinx.coroutines.runBlocking
import java.nio.file.Files
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class SpiritNodeTest {
    @Test
    fun `native nodes pair ping report presence and reopen`() = runBlocking {
        val firstDir = Files.createTempDirectory("spirit-kmp-node-a").toString()
        val secondDir = Files.createTempDirectory("spirit-kmp-node-b").toString()
        val first = SpiritNode.open(firstDir, "desktop", local = true)
        val second = SpiritNode.open(secondDir, "phone", local = true)
        try {
            first.createMesh("personal")
            val code = second.pair()
            assertTrue(code.ticket.startsWith("spirit1"))
            assertEquals(code.width * code.width, code.modules.size)
            assertEquals("phone", first.add(code.ticket))
            assertEquals("phone", first.ping("phone").name)
            assertTrue(first.status().peers.single().connected)
            assertTrue(second.status().peers.single().connected)
        } finally {
            first.close()
            second.close()
        }
        SpiritNode.open(firstDir, "desktop", local = true).use { reopened ->
            assertEquals("personal", reopened.status().meshName)
            assertEquals("phone", reopened.status().peers.single().name)
        }
    }
}

class SpiritNodeNoIntroducerTest {
    @Test
    fun `third node enrolls through a member while the original inviter is offline`() = runBlocking {
        val aDir = Files.createTempDirectory("spirit-kmp-node-a").toString()
        val bDir = Files.createTempDirectory("spirit-kmp-node-b").toString()
        val cDir = Files.createTempDirectory("spirit-kmp-node-c").toString()
        val b = SpiritNode.open(bDir, "b", local = true)
        val c = SpiritNode.open(cDir, "c", local = true)
        val a = SpiritNode.open(aDir, "a", local = true)
        try {
            b.createMesh("personal")
            assertEquals("c", b.add(c.pair().ticket))
            b.close()
            assertEquals("a", c.add(a.pair().ticket))
            assertEquals("a", c.ping("a").name)
            assertTrue(c.status().peers.any { it.name == "a" })
        } finally {
            b.close()
            c.close()
            a.close()
        }
        SpiritNode.open(cDir, "c", local = true).use { reopened ->
            assertEquals("personal", reopened.status().meshName)
            assertTrue(reopened.status().peers.any { it.name == "a" })
        }
    }
}
