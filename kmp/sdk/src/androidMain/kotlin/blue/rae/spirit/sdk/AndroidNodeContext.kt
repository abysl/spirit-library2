package blue.rae.spirit.sdk

import android.content.Context

object AndroidNodeContext {
    init { System.loadLibrary("spirit_ffi") }

    private external fun install(context: Context)

    fun initialize(context: Context) = install(context.applicationContext)
}
