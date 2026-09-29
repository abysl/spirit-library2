package com.abysl.spirit2_demo

import java.nio.file.Files
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotNull

class MeshSessionDesktopTest {
    @Test
    fun cancelled_owner_releases_directory_before_next_session_opens() = runBlocking {
        val dir = Files.createTempDirectory("spirit-session-owner").toString()
        val first = assertNotNull(openMeshSession(dir, "desktop", true))
        val running = launch(Dispatchers.IO) { first.run() }
        withTimeout(10_000) { first.state.first { !it.loading && it.invitation != null } }
        first.createGroup("personal")
        running.cancelAndJoin()

        val second = assertNotNull(openMeshSession(dir, "desktop", true))
        val reopened = launch(Dispatchers.IO) { second.run() }
        try {
            val state = withTimeout(10_000) { second.state.first { !it.loading && it.groups.isNotEmpty() } }
            assertEquals("personal", state.groups.single().name)
        } finally {
            reopened.cancelAndJoin()
        }
    }
}
