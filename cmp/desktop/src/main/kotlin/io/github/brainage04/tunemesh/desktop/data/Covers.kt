package io.github.brainage04.tunemesh.desktop.data

import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.toComposeImageBitmap
import io.github.brainage04.tunemesh.library.LibraryFormat
import io.github.brainage04.tunemesh.library.Track
import org.jetbrains.skia.Image
import org.jetbrains.skia.Rect
import org.jetbrains.skia.SamplingMode
import org.jetbrains.skia.Surface
import java.io.File
import java.util.Collections

/**
 * Album art: the embedded picture, else `cover.jpg`/`folder.jpg`/... next to the track, decoded with Skia,
 * square-cropped and scaled to the requested size. Like the GTK app: no disk cache, a 24 MiB memory LRU.
 */
class Covers {
    private val memory = LinkedHashMap<String, ImageBitmap>(64, 0.75f, true)
    private var memoryBytes = 0L
    private val missing: MutableSet<String> = Collections.synchronizedSet(HashSet())

    fun cached(track: Track, sizePx: Int): ImageBitmap? = synchronized(memory) { memory[key(track, sizePx)] }

    /** Blocking; returns null when the track has no art. */
    fun load(root: File, track: Track, sizePx: Int): ImageBitmap? {
        val key = key(track, sizePx)
        synchronized(memory) { memory[key] }?.let { return it }
        if (track.path in missing) return null
        val bytes = extract(File(root, track.path))
        val image = bytes?.let { runCatching { Image.makeFromEncoded(it) }.getOrNull() }
        if (image == null) {
            missing += track.path
            return null
        }
        val bitmap = image.use { scale(it, sizePx) }
        synchronized(memory) {
            memory.put(key, bitmap)?.let { memoryBytes -= it.byteCount() }
            memoryBytes += bitmap.byteCount()
            val iterator = memory.values.iterator()
            while (memoryBytes > CACHE_BYTES && memory.size > 1 && iterator.hasNext()) {
                memoryBytes -= iterator.next().byteCount()
                iterator.remove()
            }
        }
        return bitmap
    }

    private fun scale(image: Image, size: Int): ImageBitmap {
        val side = minOf(image.width, image.height).toFloat()
        val source = Rect.makeXYWH((image.width - side) / 2, (image.height - side) / 2, side, side)
        val target = minOf(size, side.toInt())
        return Surface.makeRasterN32Premul(target, target).use { surface ->
            surface.canvas.drawImageRect(image, source, Rect.makeWH(target.toFloat(), target.toFloat()), SamplingMode.MITCHELL, null, true)
            surface.makeImageSnapshot().toComposeImageBitmap()
        }
    }

    private fun extract(audio: File): ByteArray? {
        TagReader.picture(audio)?.let { return it }
        val folder = audio.parentFile ?: return null
        for (name in LibraryFormat.COVER_FILE_NAMES) {
            val candidate = File(folder, name)
            if (candidate.isFile) return runCatching { candidate.readBytes() }.getOrNull()
        }
        return null
    }

    private fun ImageBitmap.byteCount() = width.toLong() * height * 4

    private fun key(track: Track, sizePx: Int) = "${track.path}|${track.modifiedMs}@$sizePx"

    private companion object {
        const val CACHE_BYTES = 24L * 1024 * 1024
    }
}
