package io.github.brainage04.tunemesh

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import io.github.brainage04.tunemesh.ui.App
import io.github.brainage04.tunemesh.ui.AppViewModel
import io.github.brainage04.tunemesh.ui.theme.AppTheme

class MainActivity : ComponentActivity() {
    private val viewModel: AppViewModel by viewModels()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge(
            statusBarStyle = SystemBarStyle.dark(android.graphics.Color.TRANSPARENT),
            navigationBarStyle = SystemBarStyle.dark(android.graphics.Color.TRANSPARENT),
        )
        setContent {
            AppTheme {
                App(viewModel, finish = ::finish)
            }
        }
    }

    override fun onStart() {
        super.onStart()
        viewModel.repository.setForeground(true)
        viewModel.player.connect()
    }

    override fun onStop() {
        viewModel.repository.setForeground(false)
        viewModel.player.disconnect()
        super.onStop()
    }
}
