package blue.rae.spirit.sdk

import kotlinx.coroutines.runBlocking
import uniffi.spirit_ffi.FfiException
import java.nio.file.Files
import kotlin.io.path.absolutePathString
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class SpiritStoreTest {
    private fun tempStore(): String = Files.createTempDirectory("spirit-sdk-test").absolutePathString()

    @Test
    fun `put then get round trips through the rust store`() {
        runBlocking {
            SpiritStore.open(tempStore()).use { store ->
                val hash = store.put("there and back again".encodeToByteArray())
                assertEquals(64, hash.hex.length)
                assertTrue(store.has(hash))
                assertContentEquals("there and back again".encodeToByteArray(), store.get(hash))
            }
        }
    }

    @Test
    fun `put is idempotent`() {
        runBlocking {
            SpiritStore.open(tempStore()).use { store ->
                val first = store.put("same bytes".encodeToByteArray())
                val second = store.put("same bytes".encodeToByteArray())
                assertEquals(first, second)
            }
        }
    }

    @Test
    fun `a missing blob is a typed exception`() {
        runBlocking {
            SpiritStore.open(tempStore()).use { store ->
                val zero = BlobHash("0".repeat(64))
                assertFalse(store.has(zero))
                assertFailsWith<FfiException.Missing> { store.get(zero) }
            }
        }
    }

    @Test
    fun `a malformed hash is rejected before the store is touched`() {
        runBlocking {
            SpiritStore.open(tempStore()).use { store ->
                assertFailsWith<FfiException.Invalid> { store.get(BlobHash("not-a-hash")) }
            }
        }
    }
}
