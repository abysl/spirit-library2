package blue.rae.spirit.nativeprep

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith

class NativeArtifactsTest {
    @Test
    fun resolvesHostNativeArtifactNames() {
        assertEquals("libspirit_ffi.so", NativeArtifacts.sharedLibraryName("Linux"))
        assertEquals("libspirit_ffi.dylib", NativeArtifacts.sharedLibraryName("Mac OS X"))
        assertEquals("spirit_ffi.dll", NativeArtifacts.sharedLibraryName("Windows Server 2025"))
        assertEquals("spirit", NativeArtifacts.executableName("Linux"))
        assertEquals("spirit", NativeArtifacts.executableName("Mac OS X"))
        assertEquals("spirit.exe", NativeArtifacts.executableName("Windows 11"))
        assertEquals("uniffi-bindgen", NativeArtifacts.uniffiBindgenName("Linux"))
        assertEquals("uniffi-bindgen", NativeArtifacts.uniffiBindgenName("Mac OS X"))
        assertEquals("uniffi-bindgen.exe", NativeArtifacts.uniffiBindgenName("Windows 11"))
    }

    @Test
    fun rejectsUnsupportedOperatingSystems() {
        assertFailsWith<IllegalStateException> {
            NativeArtifacts.sharedLibraryName("FreeBSD")
        }
    }

    @Test
    fun createsArchivesOnlyForSupportedTargets() {
        assertEquals("spirit-cli-linux-x64.tar.gz", NativeArtifacts.archiveFileName("linux-x64"))
        assertEquals("spirit-cli-macos-arm64.tar.gz", NativeArtifacts.archiveFileName("macos-arm64"))
        assertEquals("spirit-cli-windows-x64.tar.gz", NativeArtifacts.archiveFileName("windows-x64"))
        assertFailsWith<IllegalArgumentException> {
            NativeArtifacts.archiveFileName("linux-arm64")
        }
    }
}
