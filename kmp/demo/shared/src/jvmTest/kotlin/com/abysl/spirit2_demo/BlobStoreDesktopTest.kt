package com.abysl.spirit2_demo

import kotlinx.coroutines.runBlocking
import java.nio.file.Files
import kotlin.io.path.absolutePathString
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertTrue

class BlobStoreDesktopTest {
    @Test
    fun theDesktopStoreIsSpirit() {
        val dir = Files.createTempDirectory("spirit-demo-store").absolutePathString()
        val store = assertNotNull(openBlobStore(dir))
        runBlocking {
            store.use {
                val hash = it.put("from the demo".encodeToByteArray())
                assertEquals(64, hash.length)
                assertTrue(it.has(hash))
                assertContentEquals("from the demo".encodeToByteArray(), it.get(hash))
            }
        }
        assertTrue(java.io.File(dir, hash(dir)).isFile)
    }

    private fun hash(dir: String): String = java.io.File(dir).list()!!.single()
}
