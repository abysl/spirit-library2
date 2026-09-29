package blue.rae.spirit.sdk

actual fun fakeReadFile(path: String): ByteArray = java.io.File(path).readBytes()
actual fun fakeWriteFile(path: String, bytes: ByteArray) { java.io.File(path).writeBytes(bytes) }
