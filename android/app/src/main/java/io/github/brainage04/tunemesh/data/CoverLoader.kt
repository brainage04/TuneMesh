package io.github.brainage04.tunemesh.data

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.media.MediaMetadataRetriever
import android.util.LruCache
import io.github.brainage04.tunemesh.library.AtomicFiles
import io.github.brainage04.tunemesh.library.LibraryFormat
import io.github.brainage04.tunemesh.library.Limits
import io.github.brainage04.tunemesh.library.Track
import java.io.ByteArrayOutputStream
import java.io.File
import java.io.IOException
import java.security.MessageDigest
import java.util.Collections
import kotlin.math.max

/**
 * Album art: the embedded picture, else `cover.jpg`/`folder.jpg`/... next to the track. Extracted art is stored
 * once as a [DISK_SIZE] JPEG in the app cache; decoded bitmaps live in a memory LRU per requested size.
 */
class CoverLoader(private val directory: File) {
    private val memory = object : LruCache<String, Bitmap>((Runtime.getRuntime().maxMemory() / 8).toInt()) {
        override fun sizeOf(key: String, value: Bitmap): Int = value.allocationByteCount
    }
    private val missing: MutableSet<String> = Collections.synchronizedSet(HashSet())
    private val locks = Array(16) { Any() }

    fun cached(track: Track, sizePx: Int): Bitmap? = memory.get(memoryKey(track, sizePx))

    /** Blocking; call from a background thread. Returns null when the track has no art. */
    fun load(root: File, track: Track, sizePx: Int): Bitmap? {
        val memoryKey = memoryKey(track, sizePx)
        memory.get(memoryKey)?.let { return it }
        val file = diskFile(root, track) ?: return null
        val bitmap = decodeSampled(file, sizePx) ?: return null
        memory.put(memoryKey, bitmap)
        return bitmap
    }

    /** Cached JPEG for [track], extracting it on first use; null when the track has no art. Blocking. */
    fun diskFile(root: File, track: Track): File? {
        val key = diskKey(track)
        if (key in missing) return null
        val file = File(directory, "$key.jpg")
        val marker = File(directory, "$key.none")
        synchronized(locks[(key.hashCode() and 0x7fffffff) % locks.size]) {
            if (file.exists()) return file
            if (marker.exists()) {
                missing += key
                return null
            }
            val bytes = extract(File(root, track.path))
            val scaled = bytes?.let { scaleToJpeg(it) }
            try {
                if (scaled == null) {
                    directory.mkdirs()
                    marker.createNewFile()
                    missing += key
                    return null
                }
                AtomicFiles.write(file, scaled)
            } catch (_: IOException) {
                return null
            }
            return file
        }
    }

    /**
     * The embedded picture, else the first folder cover. A picture over [Limits.MAX_COVER_BYTES] counts as none.
     * MediaMetadataRetriever hands over the whole embedded picture, so a huge one is loaded once to see its size;
     * a folder cover is checked before it is read.
     */
    private fun extract(audio: File): ByteArray? {
        val retriever = MediaMetadataRetriever()
        val embedded = try {
            retriever.setDataSource(audio.absolutePath)
            retriever.embeddedPicture
        } catch (_: RuntimeException) {
            null
        } finally {
            retriever.release()
        }
        // An embedded picture over the limit is not shown; the folder cover does not stand in for it.
        if (embedded != null) return embedded.takeIf { it.size <= Limits.MAX_COVER_BYTES }
        val folder = audio.parentFile ?: return null
        for (name in LibraryFormat.COVER_FILE_NAMES) {
            val candidate = File(folder, name)
            if (!candidate.isFile) continue
            return try {
                Limits.readBytes(candidate, Limits.MAX_COVER_BYTES, followLinks = false)
            } catch (_: IOException) {
                null
            }
        }
        return null
    }

    private fun scaleToJpeg(bytes: ByteArray): ByteArray? {
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
        // A size that cannot be read from the header, or a side over the limit, is not shown.
        if (bounds.outWidth <= 0 || bounds.outHeight <= 0) return null
        if (bounds.outWidth > Limits.MAX_COVER_SIDE || bounds.outHeight > Limits.MAX_COVER_SIDE) return null
        val options = BitmapFactory.Options().apply { inSampleSize = sampleSize(bounds.outWidth, bounds.outHeight, DISK_SIZE) }
        val decoded = BitmapFactory.decodeByteArray(bytes, 0, bytes.size, options) ?: return null
        val side = minOf(decoded.width, decoded.height)
        val square = Bitmap.createBitmap(decoded, (decoded.width - side) / 2, (decoded.height - side) / 2, side, side)
        val target = if (side > DISK_SIZE) Bitmap.createScaledBitmap(square, DISK_SIZE, DISK_SIZE, true) else square
        return ByteArrayOutputStream().use { output ->
            target.compress(Bitmap.CompressFormat.JPEG, 88, output)
            output.toByteArray()
        }
    }

    private fun decodeSampled(file: File, sizePx: Int): Bitmap? {
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeFile(file.absolutePath, bounds)
        if (bounds.outWidth <= 0) return null
        val options = BitmapFactory.Options().apply { inSampleSize = sampleSize(bounds.outWidth, bounds.outHeight, sizePx) }
        val decoded = BitmapFactory.decodeFile(file.absolutePath, options) ?: return null
        return if (decoded.width > sizePx * 3 / 2) Bitmap.createScaledBitmap(decoded, sizePx, sizePx, true) else decoded
    }

    private fun sampleSize(width: Int, height: Int, target: Int): Int {
        var sample = 1
        while (max(width, height) / (sample * 2) >= target) sample *= 2
        return sample
    }

    private fun memoryKey(track: Track, sizePx: Int) = "${diskKey(track)}@$sizePx"

    private fun diskKey(track: Track): String {
        val digest = MessageDigest.getInstance("SHA-1")
            .digest("${track.path}|${track.size}|${track.modifiedMs}".toByteArray(Charsets.UTF_8))
        return digest.joinToString("") { "%02x".format(it) }
    }

    companion object {
        const val DISK_SIZE = 600
    }
}
