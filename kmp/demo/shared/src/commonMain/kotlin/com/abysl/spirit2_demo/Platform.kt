package com.abysl.spirit2_demo

interface Platform {
    val name: String
}

expect fun getPlatform(): Platform