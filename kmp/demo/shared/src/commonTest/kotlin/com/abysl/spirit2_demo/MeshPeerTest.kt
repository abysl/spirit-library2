package com.abysl.spirit2_demo

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class MeshPeerTest {
    @Test
    fun errors_are_visible_without_disconnecting_a_recently_heartbeating_peer() {
        val peer = MeshPeer("id", "desktop", true, 10, "connection refused")
        assertTrue(peer.connectionLabel().contains("connection refused"))
        assertTrue(peer.connectionLabel().startsWith("Connected"))
    }

    @Test
    fun never_seen_and_stale_members_are_explained() {
        assertTrue(MeshPeer("id", "desktop", false, null, null).connectionLabel().contains("waiting"))
        assertTrue(MeshPeer("id", "desktop", false, 61000, null).connectionLabel().contains("61s"))
        assertEquals("Connected", MeshPeer("id", "desktop", true, 0, null).connectionLabel())
    }
}
