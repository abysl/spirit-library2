package blue.rae.spirit.sdk

import kotlinx.coroutines.runBlocking
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class FakeMeshFilesTest {
    @Test
    fun contentsDetermineHashAndExportsWriteBytes() = runBlocking {
        val files = FakeMeshFiles()
        val path = kotlin.io.path.createTempFile().toString()
        fakeWriteFile(path, byteArrayOf(1, 2, 3))
        val imported = files.importFile(path)
        val first = imported.hash
        assertEquals(3L, imported.size)
        assertEquals(64, first.length)
        fakeWriteFile(path, byteArrayOf(4, 5))
        val second = files.importFile(path).hash
        assertFalse(first == second)
        assertEquals(3L, files.exportFile(first, path))
        assertContentEquals(byteArrayOf(1, 2, 3), fakeReadFile(path))
        assertFalse(files.admitted("mesh", "peer", 0))
        files.admit("mesh", "peer", 0)
        assertTrue(files.admitted("mesh", "peer", 0))
        assertFalse(files.admitted("mesh", "peer", 1))
        val signature = files.signApp("afm/catalog/op/1", byteArrayOf(1))
        assertTrue(signature.startsWith("fake-unsigned:"))
        assertFalse(files.verifyApp("fake-node", "afm/catalog/op/1", byteArrayOf(2), signature))
    }
}
