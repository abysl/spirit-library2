package com.abysl.spirit2_demo

import androidx.compose.ui.window.ComposeUIViewController
import platform.Foundation.NSHomeDirectory

fun MainViewController() = ComposeUIViewController {
    App(openBlobStore(NSHomeDirectory() + "/Documents/spirit"))
}