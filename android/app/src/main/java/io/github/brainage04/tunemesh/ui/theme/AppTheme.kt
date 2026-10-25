package io.github.brainage04.tunemesh.ui.theme

import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.graphics.Color

val Accent = Color(0xFF1ED760)
val Background = Color(0xFF121212)
val Surface = Color(0xFF181818)
val SurfaceHigh = Color(0xFF282828)
val TextSecondary = Color(0xFFB3B3B3)

private val Colors =
    darkColorScheme(
        primary = Accent,
        onPrimary = Color.Black,
        secondary = Accent,
        onSecondary = Color.Black,
        background = Background,
        onBackground = Color.White,
        surface = Background,
        onSurface = Color.White,
        surfaceVariant = SurfaceHigh,
        onSurfaceVariant = TextSecondary,
        surfaceContainerLowest = Background,
        surfaceContainerLow = Surface,
        surfaceContainer = Surface,
        surfaceContainerHigh = SurfaceHigh,
        surfaceContainerHighest = Color(0xFF333333),
        outline = Color(0xFF535353),
        outlineVariant = Color(0xFF3E3E3E),
        error = Color(0xFFF15E6C),
        secondaryContainer = SurfaceHigh,
        onSecondaryContainer = Color.White,
        primaryContainer = Color(0xFF1A3D27),
        onPrimaryContainer = Color.White,
    )

/** Dark theme; TuneMesh has no light mode. */
@Composable
fun AppTheme(content: @Composable () -> Unit) {
    MaterialTheme(colorScheme = Colors) {
        CompositionLocalProvider(LocalContentColor provides Colors.onBackground, content = content)
    }
}
