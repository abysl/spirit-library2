package blue.rae.spirit.sdk

actual fun fakeReadFile(path: String): ByteArray = throw MeshNodeException(MeshFailure.Invalid, "fake path IO is only available on Android and JVM")
actual fun fakeWriteFile(path: String, bytes: ByteArray): Unit = throw MeshNodeException(MeshFailure.Invalid, "fake path IO is only available on Android and JVM")
