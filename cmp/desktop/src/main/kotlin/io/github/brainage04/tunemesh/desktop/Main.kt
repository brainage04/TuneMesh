package io.github.brainage04.tunemesh.desktop

import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.unit.DpSize
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Window
import androidx.compose.ui.window.application
import androidx.compose.ui.window.rememberWindowState
import io.github.brainage04.tunemesh.desktop.data.Covers
import io.github.brainage04.tunemesh.desktop.data.LibraryRepository
import io.github.brainage04.tunemesh.desktop.playback.Player
import io.github.brainage04.tunemesh.desktop.ui.AppModel
import io.github.brainage04.tunemesh.desktop.ui.DesktopApp
import io.github.brainage04.tunemesh.ui.theme.AppTheme
import java.awt.Dimension
import java.io.File
import java.lang.management.ManagementFactory
import javax.swing.SwingUtilities
import kotlin.system.exitProcess

private const val VERSION = "0.1.0-cmp-prototype"
private const val USAGE = "usage: tunemesh-cmp [--library <folder>] [--version]"

private fun env(name: String): String? = System.getenv(name)?.takeIf { it.isNotEmpty() }

private fun xdg(name: String, fallback: String): File =
    env(name)?.let(::File)?.takeIf { it.isAbsolute } ?: File(System.getProperty("user.home"), fallback)

fun main(args: Array<String>) {
    var library = File(System.getProperty("user.home"), "Music/TuneMesh")
    var index = 0
    while (index < args.size) {
        when (val arg = args[index++]) {
            "--library" -> library = File(args.getOrNull(index++) ?: run { System.err.println(USAGE); exitProcess(2) })
            "--version" -> { println("tunemesh-cmp $VERSION"); return }
            "-h", "--help" -> { println(USAGE); return }
            else -> { System.err.println("tunemesh-cmp: unknown argument \"$arg\"\n$USAGE"); exitProcess(2) }
        }
    }

    val repository = LibraryRepository(library.absoluteFile, File(xdg("XDG_CACHE_HOME", ".cache"), "tunemesh-cmp/tags.json"))
    val player = Player { repository.root }
    val model = AppModel(repository, Covers(), player)
    repository.start()
    Runtime.getRuntime().addShutdownHook(Thread { player.shutdown() })
    env("TUNEMESH_CONTROL_FIFO")?.let { startControl(File(it), model) }

    application {
        val state = rememberWindowState(size = DpSize(1280.dp, 820.dp))
        Window(onCloseRequest = ::exitApplication, state = state, title = "TuneMesh") {
            remember { window.minimumSize = Dimension(900, 560) }
            LaunchedEffect(Unit) {
                withFrameNanos { }
                val uptime = ManagementFactory.getRuntimeMXBean().uptime
                System.err.println("tunemesh-cmp: first frame after ${uptime} ms of JVM uptime, render API ${window.renderApi}")
            }
            AppTheme { DesktopApp(model) }
        }
    }
}

/**
 * Benchmark control: reads the GTK app's action names (`navigate <route>`, `play-route <route>`,
 * `toggle-queue`, `play-pause`, `quit`) line by line from a FIFO, so both apps are driven the same way.
 */
private fun startControl(fifo: File, model: AppModel) {
    Thread({
        while (true) {
            fifo.bufferedReader().useLines { lines ->
                lines.forEach { line ->
                    val command = line.substringBefore(' ')
                    val arg = line.substringAfter(' ', "")
                    SwingUtilities.invokeLater {
                        when (command) {
                            "navigate" -> model.parseRoute(arg)?.let(model::navigate) ?: System.err.println("tunemesh-cmp: unknown route \"$arg\"")
                            "play-route" -> model.parseRoute(arg)?.let { route ->
                                val (name, tracks) = model.routeTracks(route)
                                model.play(name, tracks, 0, shuffle = false)
                            }
                            "toggle-queue" -> model.queueOpen = !model.queueOpen
                            "play-pause" -> model.player.togglePlay()
                            "back" -> model.goBack()
                            "quit" -> exitProcess(0)
                            else -> System.err.println("tunemesh-cmp: unknown command \"$line\"")
                        }
                    }
                }
            }
        }
    }, "control").apply { isDaemon = true; start() }
}
