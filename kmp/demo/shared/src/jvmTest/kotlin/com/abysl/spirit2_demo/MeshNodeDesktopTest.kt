package com.abysl.spirit2_demo

import java.nio.file.Files
import kotlinx.coroutines.runBlocking
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

class MeshNodeDesktopTest {
    @Test
    fun shared_node_adapter_owns_the_node_and_releases_its_directory() = runBlocking {
        val firstDir = Files.createTempDirectory("spirit-adapter-a").toString()
        val secondDir = Files.createTempDirectory("spirit-adapter-b").toString()
        val first = openMeshNode(firstDir, "desktop", true)!!
        val second = openMeshNode(secondDir, "phone", true)!!
        try {
            first.createMesh("personal")
            first.add(second.pair().ticket)
            val peer = first.status().peers.single()
            assertTrue(peer.connected)
            assertTrue(first.ping(peer.id).contains("Pong from phone"))
        } finally {
            first.close()
            second.close()
        }
        assertFailsWith<IllegalStateException> { first.status() }
        val reopened = openMeshNode(firstDir, "desktop", true)!!
        try { assertEquals("personal", reopened.status().meshName) }
        finally { reopened.close() }
    }
}
