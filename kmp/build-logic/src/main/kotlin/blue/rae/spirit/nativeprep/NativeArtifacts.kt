package blue.rae.spirit.nativeprep

internal object NativeArtifacts {
    private val archiveTargets = setOf("linux-x64", "macos-arm64", "windows-x64")

    fun sharedLibraryName(systemName: String): String = when (operatingSystem(systemName)) {
        OperatingSystem.LINUX -> "libspirit_ffi.so"
        OperatingSystem.MACOS -> "libspirit_ffi.dylib"
        OperatingSystem.WINDOWS -> "spirit_ffi.dll"
    }

    fun executableName(systemName: String): String = when (operatingSystem(systemName)) {
        OperatingSystem.WINDOWS -> "spirit.exe"
        OperatingSystem.LINUX, OperatingSystem.MACOS -> "spirit"
    }

    fun uniffiBindgenName(systemName: String): String = when (operatingSystem(systemName)) {
        OperatingSystem.WINDOWS -> "uniffi-bindgen.exe"
        OperatingSystem.LINUX, OperatingSystem.MACOS -> "uniffi-bindgen"
    }

    fun archiveFileName(target: String): String {
        require(target in archiveTargets) { "Unsupported archive target: $target" }
        return "spirit-cli-$target.tar.gz"
    }

    private fun operatingSystem(systemName: String): OperatingSystem = when {
        systemName == "Linux" -> OperatingSystem.LINUX
        systemName == "Mac OS X" || systemName == "Darwin" -> OperatingSystem.MACOS
        systemName.startsWith("Windows") -> OperatingSystem.WINDOWS
        else -> error("Unsupported operating system: $systemName")
    }

    private enum class OperatingSystem {
        LINUX,
        MACOS,
        WINDOWS,
    }
}
