package io.github.brainage04.tunemesh.library

import java.io.File
import java.io.IOException
import java.security.MessageDigest

/**
 * This device's playlist bases for one library (contract: "Playlist conflict copies", "Base"), in app-private storage, never in the
 * library. Per playlist file name `<Name>.m3u8` the directory holds `<Name>.m3u8`, the base entries in the writers' format (without library
 * lookups), and `<Name>.m3u8.written`, the SHA-256 of the bytes this device last wrote to that playlist. Only [PlaylistStore] changes them,
 * one change at a time.
 */
class PlaylistBases(private val directory: File) {
    private fun baseFile(fileName: String) = File(directory, fileName)

    private fun writtenFile(fileName: String) = File(directory, "$fileName$WRITTEN")

    /** The base of [fileName] read as a playlist below [root], or null when there is none or it cannot be read (then: no base). */
    fun read(fileName: String, root: String): PlaylistEntries? {
        val file = baseFile(fileName)
        if (!file.isFile) return null
        val entries = PlaylistEntries.Builder(root, keepExtinf = true)
        return try {
            file.inputStream().use { input -> M3u8.read(input, root, keepExtinf = true) { entries.add(it) } }
            entries.build()
        } catch (_: IOException) {
            null
        }
    }

    /**
     * Before this device edits [fileName]: the base becomes [reread], the version the edit re-read (with SHA-256 [digest]), unless that is
     * exactly what this device wrote last; then the base stays as it is, so several edits in a row keep the version they started from.
     */
    fun beforeEdit(fileName: String, reread: PlaylistEntries, digest: ByteArray) {
        val written = writtenFile(fileName)
        if (written.isFile && written.readBytes().contentEquals(digest)) return
        set(fileName, reread)
    }

    /** After this device wrote [bytes] to [fileName]. */
    fun written(fileName: String, bytes: ByteArray) {
        AtomicFiles.write(writtenFile(fileName), sha256(bytes))
    }

    /** Makes [entries] the base of [fileName]; a playlist over the limits gets no base. */
    fun set(fileName: String, entries: PlaylistEntries) {
        val text =
            try {
                M3u8.write(entries) { null }
            } catch (_: PlaylistTooLargeException) {
                baseFile(fileName).delete()
                return
            }
        AtomicFiles.write(baseFile(fileName), text)
    }

    /** Makes the merged playlist, which this device just wrote as [bytes], the base of [fileName]. */
    fun merged(fileName: String, bytes: ByteArray) {
        AtomicFiles.write(baseFile(fileName), bytes)
        written(fileName, bytes)
    }

    /** The base of a renamed playlist moves with it; whatever [to] had before goes. */
    fun rename(from: String, to: String) {
        if (from == to) return
        for ((source, target) in listOf(baseFile(from) to baseFile(to), writtenFile(from) to writtenFile(to))) {
            if (!source.isFile || !source.renameTo(target)) {
                source.delete()
                target.delete()
            }
        }
    }

    fun delete(fileName: String) {
        baseFile(fileName).delete()
        writtenFile(fileName).delete()
    }

    companion object {
        private const val WRITTEN = ".written"

        fun sha256(bytes: ByteArray): ByteArray = MessageDigest.getInstance("SHA-256").digest(bytes)
    }
}
