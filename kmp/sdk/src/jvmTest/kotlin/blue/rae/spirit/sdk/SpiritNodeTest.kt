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
        val meshId = try {
            first.createMesh("personal")
            val createdId = first.status().meshId ?: error("mesh ID is missing")
            assertTrue(createdId.startsWith("mesh1_"))
            assertTrue(createdId != first.status().id)
            val code = second.pair()
            assertTrue(code.ticket.startsWith("spirit1"))
            assertEquals(code.width * code.width, code.modules.size)
            assertEquals("phone", first.add(code.ticket))
            assertEquals("phone", first.ping("phone").name)
            assertTrue(first.status().peers.single().connected)
            assertTrue(second.status().peers.single().connected)
            assertEquals(createdId, second.status().meshId)
            createdId
        } finally {
            first.close()
            second.close()
        }
        SpiritNode.open(firstDir, "desktop", local = true).use { reopened ->
            assertEquals("personal", reopened.status().meshName)
            assertEquals(meshId, reopened.status().meshId)
            assertEquals("phone", reopened.status().peers.single().name)
        }
    }
}

class SpiritNodeLeaveTest {
    @Test
    fun `a device leaves, members drop it, and a member readmits it`() = runBlocking {
        val memberDir = Files.createTempDirectory("spirit-kmp-node-a").toString()
        val leaverDir = Files.createTempDirectory("spirit-kmp-node-b").toString()
        SpiritNode.open(memberDir, "desktop", local = true).use { member ->
            SpiritNode.open(leaverDir, "phone", local = true).use { leaver ->
                member.createMesh("personal")
                assertEquals("phone", member.add(leaver.pair().ticket))
                val left = leaver.leaveMesh()
                assertEquals(LeftMesh(member.status().meshId!!, "personal", 1, 1), left)
                assertEquals(null, leaver.status().meshName)
                assertTrue(leaver.status().peers.isEmpty())
                assertTrue(member.status().peers.isEmpty())
                assertEquals("phone", member.add(leaver.pair().ticket))
                assertEquals("personal", leaver.status().meshName)
                assertEquals("phone", member.ping("phone").name)
            }
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
