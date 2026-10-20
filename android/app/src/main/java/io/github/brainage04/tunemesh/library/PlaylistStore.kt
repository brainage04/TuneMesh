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

/**
 * Playlist files below `<root>/Playlists`. Every change re-reads the file first and writes atomically. Files are
 * streamed ([M3u8.read]): listing keeps a few fields per playlist, and only the playlist that is open or being
 * changed is held entry by entry. A playlist beyond [M3u8]'s limits is listed with a [Playlist.problem] and is
 * never rewritten.
 */
class PlaylistStore(
    private val root: File,
    private val summaries: PlaylistSummaries? = null,
    private val lookup: (String) -> Track?,
) {
    private val directory get() = File(root, LibraryFormat.PLAYLISTS_DIR)

    fun file(name: String): File = File(directory, name + LibraryFormat.PLAYLIST_EXTENSION)

    /** The library's own string for [path], so entries do not each hold a copy. */
    private fun canonical(path: String): String? = lookup(path)?.path

    private fun stream(file: File, keepExtinf: Boolean, onEntry: (PlaylistEntry) -> Unit) {
        if (file.length() > Limits.MAX_PLAYLIST_BYTES) throw PlaylistTooLargeException("larger than ${Limits.MAX_PLAYLIST_BYTES / (1024 * 1024)} MiB")
        file.inputStream().use { M3u8.read(it, root.absolutePath, keepExtinf, canonical = ::canonical, onEntry = onEntry) }
    }

    /**
     * Liked Songs, then the first [Limits.MAX_PLAYLISTS_LISTED] other playlists by name ignoring case, as summaries.
     * Only those files are read, and an unchanged one is not read again ([summaries]). Files that cannot be read at
     * all are left out.
     */
    fun list(): List<Playlist> {
        val names = directory.list()?.filter { name ->
            name.endsWith(LibraryFormat.PLAYLIST_EXTENSION) && !LibraryFormat.isHidden(name) && !LibraryFormat.isConflictCopy(name)
        } ?: return emptyList()
        val liked = LibraryFormat.LIKED_SONGS + LibraryFormat.PLAYLIST_EXTENSION
        val others = names.filter { it != liked }.sortedWith(String.CASE_INSENSITIVE_ORDER).take(Limits.MAX_PLAYLISTS_LISTED)
        val listed = (if (liked in names) listOf(liked) else emptyList()) + others
        summaries?.retain(listed)
        return listed.mapNotNull { name ->
            val file = File(directory, name)
            if (!file.isFile) null else summaries?.get(file, lookup) { summary(file) } ?: summary(file)
        }
    }

    private fun summary(file: File): Playlist? {
        val name = file.name.removeSuffix(LibraryFormat.PLAYLIST_EXTENSION)
        var size = 0
        val art = ArrayList<String>(ART_PATHS)
        return try {
            stream(file, keepExtinf = false) { entry ->
                size++
                val path = entry.path
                if (art.size < ART_PATHS && path != null && lookup(path) != null) art += path
            }
            Playlist(name, size, art, file.lastModified())
        } catch (error: PlaylistTooLargeException) {
            Playlist(name, 0, emptyList(), file.lastModified(), problem = "Too large to open on this device (${error.message})")
        } catch (_: IOException) {
            null
        }
    }

    /**
     * The entries of playlist [name] for a screen (without `#EXTINF` lines), or null when it does not exist or
     * cannot be read. Throws [PlaylistTooLargeException] beyond the limits.
     */
    fun contents(name: String): PlaylistContents? = read(name, keepExtinf = false)

    /** The playlist as a writer needs it, `#EXTINF` lines included; null when the file does not exist. */
    fun read(name: String): PlaylistContents? = read(name, keepExtinf = true)

    private fun read(name: String, keepExtinf: Boolean): PlaylistContents? {
        val file = file(name)
        val entries = ArrayList<PlaylistEntry>()
        try {
            stream(file, keepExtinf) { entries += it }
        } catch (_: java.io.FileNotFoundException) {
            return null
        }
        entries.trimToSize()
        return PlaylistContents(name, entries, file.lastModified())
    }

    /** Library paths in Liked Songs, without keeping its entries; empty when it does not exist. */
    fun likedPaths(): Set<String> {
        val paths = HashSet<String>()
        try {
            stream(file(LibraryFormat.LIKED_SONGS), keepExtinf = false) { entry -> entry.path?.let(paths::add) }
        } catch (_: java.io.FileNotFoundException) {
        }
        return paths
    }

    /**
     * Entries to change. A missing file is empty; a file that cannot be read or is too large is never rewritten
     * from a partial read, so the change is refused.
     */
    private fun entries(name: String): List<PlaylistEntry> = try {
        read(name)?.entries ?: emptyList()
    } catch (_: PlaylistTooLargeException) {
        throw PlaylistException("\"$name\" is too large to change on this device")
    } catch (error: IOException) {
        throw PlaylistException("Cannot read \"$name\": ${error.message}")
    }

    /** Writes [entries]; a playlist that would be over the size limits is refused and the file left as it is. */
    private fun write(name: String, entries: List<PlaylistEntry>) {
        val text = try {
            M3u8.write(entries, lookup)
        } catch (error: PlaylistTooLargeException) {
            throw PlaylistException("\"$name\" would be too large (${error.message})")
        }
        AtomicFiles.write(file(name), text)
    }

    /**
     * The playlist whose name equals [name] ignoring letter case (Liked Songs included, whether or not its file
     * exists yet). Shared storage is case-insensitive, so such a playlist is the same file.
     */
    private fun existingName(name: String): String? {
        if (name.equals(LibraryFormat.LIKED_SONGS, ignoreCase = true)) return LibraryFormat.LIKED_SONGS
        return directory.list()?.firstNotNullOfOrNull { fileName ->
            fileName.takeIf {
                it.endsWith(LibraryFormat.PLAYLIST_EXTENSION) && !LibraryFormat.isHidden(it) && !LibraryFormat.isConflictCopy(it)
            }?.removeSuffix(LibraryFormat.PLAYLIST_EXTENSION)?.takeIf { it.equals(name, ignoreCase = true) }
        }
    }

    /** Creates an empty playlist and returns its (sanitised) name. */
    fun create(name: String): String {
        val fileName = LibraryFormat.playlistFileName(name)
        val finalName = fileName.removeSuffix(LibraryFormat.PLAYLIST_EXTENSION)
        if (name.isBlank()) throw PlaylistException("Enter a playlist name")
        existingName(finalName)?.let { throw PlaylistException("A playlist called \"$it\" already exists") }
        write(finalName, emptyList())
        return finalName
    }

    /** Renames a playlist; changing only the letter case of its name is allowed. */
    fun rename(from: String, to: String): String {
        if (from == LibraryFormat.LIKED_SONGS) throw PlaylistException("Liked Songs cannot be renamed")
        if (to.isBlank()) throw PlaylistException("Enter a playlist name")
        val finalName = LibraryFormat.playlistFileName(to).removeSuffix(LibraryFormat.PLAYLIST_EXTENSION)
        if (finalName == from) return from
        existingName(finalName)?.takeIf { it != from }?.let { throw PlaylistException("A playlist called \"$it\" already exists") }
        Files.move(file(from).toPath(), file(finalName).toPath(), StandardCopyOption.ATOMIC_MOVE)
        return finalName
    }

    fun delete(name: String) {
        if (name == LibraryFormat.LIKED_SONGS) throw PlaylistException("Liked Songs cannot be deleted")
        if (!file(name).delete() && file(name).exists()) throw PlaylistException("Could not delete \"$name\"")
    }

    /** Appends [tracks]; a track whose path contains a line break is skipped (see [LibraryFormat.hasLineBreak]). */
    fun add(name: String, tracks: List<Track>) {
        write(name, entries(name) + tracks.filter { M3u8.canWrite(it.path) }.map(M3u8::entryFor))
    }

    /**
     * Removes the entry the user saw at [entry]. The file is read again first, so an entry another device moved is
     * found by its key; one that is gone fails without writing.
     */
    fun remove(name: String, entry: EntryRef) {
        val entries = entries(name).toMutableList()
        entries.removeAt(locate(entries, listOf(entry))[0])
        write(name, entries)
    }

    /** Moves the entry the user saw at [from] to where the entry they saw at [to] is now (see [remove]). */
    fun move(name: String, from: EntryRef, to: EntryRef) {
        val entries = entries(name).toMutableList()
        val (source, target) = locate(entries, listOf(from, to))
        if (source == target) return
        entries.add(target, entries.removeAt(source))
        write(name, entries)
    }

    fun setLiked(track: Track, liked: Boolean) {
        val current = entries(LibraryFormat.LIKED_SONGS)
        val updated = if (liked) like(current, track) else unlike(current, track.path)
        if (updated != current || !file(LibraryFormat.LIKED_SONGS).exists()) write(LibraryFormat.LIKED_SONGS, updated)
    }

    companion object {
        /** Candidate tracks kept per listed playlist for its cover art (the first one that is available). */
        const val ART_PATHS = 8

        /** Liking inserts the track at the top unless it is already liked. */
        fun like(entries: List<PlaylistEntry>, track: Track): List<PlaylistEntry> = when {
            !M3u8.canWrite(track.path) || entries.any { it.path == track.path } -> entries
            else -> listOf(M3u8.entryFor(track)) + entries
        }

        /** Unliking removes every entry for the track. */
        fun unlike(entries: List<PlaylistEntry>, path: String): List<PlaylistEntry> = entries.filter { it.path != path }

        /**
         * Finds shown entries in freshly read [entries]: at their position if it still holds the same entry, else
         * the first unclaimed entry with that key. Throws when one is gone, so nothing wrong is changed.
         */
        fun locate(entries: List<PlaylistEntry>, refs: List<EntryRef>): List<Int> {
            val found = ArrayList<Int>(refs.size)
            for (ref in refs) {
                fun same(i: Int) = i in entries.indices && entries[i].key == ref.key
                val at = if (same(ref.position) && ref.position !in found) {
                    ref.position
                } else {
                    entries.indices.firstOrNull { it !in found && same(it) }
                }
                found += at ?: throw PlaylistException("The playlist changed on another device. Nothing was changed; try again.")
            }
            return found
        }
    }
}
