package io.github.brainage04.tunemesh.library

import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.security.SecureRandom

object AtomicFiles {
    private val random = SecureRandom()

    /** Writes `.~tunemesh-<random>.tmp` next to [target], syncs it, then renames it over [target]. */
    fun write(target: File, bytes: ByteArray) {
        val directory = target.absoluteFile.parentFile ?: throw IOException("No parent for $target")
        directory.mkdirs()
        val temp = File(directory, ".~tunemesh-${randomHex(8)}.tmp")
        try {
            FileOutputStream(temp).use { stream ->
                stream.write(bytes)
                stream.fd.sync()
            }
            Files.move(temp.toPath(), target.toPath(), StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING)
        } finally {
            if (temp.exists()) temp.delete()
        }
    }

    fun write(target: File, text: String) = write(target, text.toByteArray(Charsets.UTF_8))

    fun randomHex(length: Int): String {
        val bytes = ByteArray((length + 1) / 2)
        random.nextBytes(bytes)
        return bytes.joinToString("") { "%02x".format(it) }.take(length)
    }
}

class PlaylistException(message: String) : Exception(message)

/** Playlist files below `<root>/Playlists`. Every change re-reads the file first and writes atomically. */
class PlaylistStore(private val root: File, private val lookup: (String) -> Track?) {
    private val directory get() = File(root, LibraryFormat.PLAYLISTS_DIR)

    fun file(name: String): File = File(directory, name + LibraryFormat.PLAYLIST_EXTENSION)

    fun list(): List<Playlist> {
        val files = directory.listFiles { file ->
            file.isFile && file.name.endsWith(LibraryFormat.PLAYLIST_EXTENSION) &&
                !LibraryFormat.isHidden(file.name) && !LibraryFormat.isConflictCopy(file.name)
        } ?: return emptyList()
        return files.mapNotNull { read(it) }.sortedWith(
            compareBy<Playlist> { !it.isLikedSongs }.thenBy(String.CASE_INSENSITIVE_ORDER) { it.name },
        )
    }

    fun read(name: String): Playlist? = read(file(name))

    private fun read(file: File): Playlist? {
        val text = try {
            file.readText(Charsets.UTF_8)
        } catch (_: IOException) {
            return null
        }
        return Playlist(
            name = file.name.removeSuffix(LibraryFormat.PLAYLIST_EXTENSION),
            entries = M3u8.parse(text, root.absolutePath),
            modifiedMs = file.lastModified(),
        )
    }

    private fun entries(name: String): List<PlaylistEntry> = read(name)?.entries ?: emptyList()

    private fun write(name: String, entries: List<PlaylistEntry>) {
        AtomicFiles.write(file(name), M3u8.write(entries, lookup))
    }

    /** Creates an empty playlist and returns its (sanitised) name. */
    fun create(name: String): String {
        val fileName = LibraryFormat.playlistFileName(name)
        val finalName = fileName.removeSuffix(LibraryFormat.PLAYLIST_EXTENSION)
        if (name.isBlank()) throw PlaylistException("Enter a playlist name")
        if (file(finalName).exists()) throw PlaylistException("A playlist called \"$finalName\" already exists")
        write(finalName, emptyList())
        return finalName
    }

    fun rename(from: String, to: String): String {
        if (from == LibraryFormat.LIKED_SONGS) throw PlaylistException("Liked Songs cannot be renamed")
        if (to.isBlank()) throw PlaylistException("Enter a playlist name")
        val finalName = LibraryFormat.playlistFileName(to).removeSuffix(LibraryFormat.PLAYLIST_EXTENSION)
        if (finalName == from) return from
        val target = file(finalName)
        if (target.exists() && !target.canonicalPath.equals(file(from).canonicalPath)) {
            throw PlaylistException("A playlist called \"$finalName\" already exists")
        }
        Files.move(file(from).toPath(), target.toPath(), StandardCopyOption.ATOMIC_MOVE)
        return finalName
    }

    fun delete(name: String) {
        if (name == LibraryFormat.LIKED_SONGS) throw PlaylistException("Liked Songs cannot be deleted")
        if (!file(name).delete() && file(name).exists()) throw PlaylistException("Could not delete \"$name\"")
    }

    fun add(name: String, tracks: List<Track>) {
        write(name, entries(name) + tracks.map(M3u8::entryFor))
    }

    /** Removes the entry at [index], or the first entry for [path] if the file changed since it was shown. */
    fun removeAt(name: String, index: Int, path: String?) {
        write(name, removeEntry(entries(name), index, path))
    }

    fun move(name: String, from: Int, to: Int) {
        val entries = entries(name).toMutableList()
        if (from !in entries.indices || to !in entries.indices) return
        entries.add(to, entries.removeAt(from))
        write(name, entries)
    }

    fun isLiked(path: String): Boolean = entries(LibraryFormat.LIKED_SONGS).any { it.path == path }

    fun setLiked(track: Track, liked: Boolean) {
        val current = entries(LibraryFormat.LIKED_SONGS)
        val updated = if (liked) like(current, track) else unlike(current, track.path)
        if (updated != current || !file(LibraryFormat.LIKED_SONGS).exists()) write(LibraryFormat.LIKED_SONGS, updated)
    }

    companion object {
        /** Liking inserts the track at the top unless it is already liked. */
        fun like(entries: List<PlaylistEntry>, track: Track): List<PlaylistEntry> =
            if (entries.any { it.path == track.path }) entries else listOf(M3u8.entryFor(track)) + entries

        /** Unliking removes every entry for the track. */
        fun unlike(entries: List<PlaylistEntry>, path: String): List<PlaylistEntry> = entries.filter { it.path != path }

        fun removeEntry(entries: List<PlaylistEntry>, index: Int, path: String?): List<PlaylistEntry> {
            val target = when {
                index in entries.indices && entries[index].path == path -> index
                else -> entries.indexOfFirst { it.path == path }
            }
            if (target < 0) return entries
            return entries.toMutableList().also { it.removeAt(target) }
        }
    }
}
