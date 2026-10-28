package io.github.brainage04.tunemesh.library

import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.security.DigestInputStream
import java.security.MessageDigest
import java.security.SecureRandom
import java.util.Locale

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
        return bytes.joinToString("") { "%02x".format(Locale.ROOT, it) }.take(length)
    }
}

class PlaylistException(message: String, cause: Throwable? = null) : Exception(message, cause)

/**
 * Playlist files below `<root>/Playlists`. Every change re-reads the file first and writes atomically. Files are streamed ([M3u8.read]):
 * listing keeps a few fields per playlist, and only the playlist that is open or being changed is held entry by entry. A playlist beyond
 * [M3u8]'s limits is listed with a [Playlist.problem] and is never rewritten.
 */
class PlaylistStore(
    private val root: File,
    private val summaries: PlaylistSummaries? = null,
    /** This device's bases for merging conflict copies ([mergeConflicts]); null: none kept and copies left alone (tests). */
    private val bases: PlaylistBases? = null,
    private val lookup: (String) -> Track?,
) {
    private val directory
        get() = File(root, LibraryFormat.PLAYLISTS_DIR)

    fun file(name: String): File = File(directory, name + LibraryFormat.PLAYLIST_EXTENSION)

    /** The library's own string for [path], so entries do not each hold a copy. */
    private fun canonical(path: String): String? = lookup(path)?.path

    /** Streams [file]'s entries to [onEntry]; [digest], when given, gets every byte read. */
    private fun stream(file: File, keepExtinf: Boolean, digest: MessageDigest? = null, onEntry: (PlaylistEntry) -> Unit) {
        if (file.length() > Limits.MAX_PLAYLIST_BYTES)
            throw PlaylistTooLargeException("larger than ${Limits.MAX_PLAYLIST_BYTES / (1024 * 1024)} MiB")
        file.inputStream().use { stream ->
            @Suppress("MissingUseCall") // only wraps the stream that `use` closes
            val input = if (digest == null) stream else DigestInputStream(stream, digest)
            M3u8.read(input, root.absolutePath, keepExtinf, canonical = ::canonical, onEntry = onEntry)
        }
    }

    /**
     * Liked Songs, then the first [Limits.MAX_PLAYLISTS_LISTED] other playlists by name ignoring case, as summaries. Only those files are
     * read, and an unchanged one is not read again ([summaries]). Files that cannot be read at all are left out.
     */
    fun list(): List<Playlist> {
        val names =
            directory.list()?.filter { name ->
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
            Playlist(name, 0, emptyList(), file.lastModified(), problem = "Too large to open on this device (${error.reason})")
        } catch (_: IOException) {
            null
        }
    }

    /**
     * The entries of playlist [name] for a screen (without `#EXTINF` lines), or null when it does not exist or cannot be read. Throws
     * [PlaylistTooLargeException] beyond the limits.
     */
    fun contents(name: String): PlaylistContents? = read(name, keepExtinf = false)

    /** The playlist as a writer needs it, `#EXTINF` lines included; null when the file does not exist. */
    fun read(name: String): PlaylistContents? = read(name, keepExtinf = true)

    private fun read(name: String, keepExtinf: Boolean, digest: MessageDigest? = null): PlaylistContents? {
        val file = file(name)
        val entries = readFile(file, keepExtinf, digest) ?: return null
        return PlaylistContents(name, entries, file.lastModified())
    }

    /** [file]'s entries, or null when it does not exist. */
    private fun readFile(file: File, keepExtinf: Boolean = true, digest: MessageDigest? = null): PlaylistEntries? {
        val entries = PlaylistEntries.Builder(root.absolutePath, keepExtinf)
        try {
            stream(file, keepExtinf, digest) { entries.add(it) }
        } catch (_: java.io.FileNotFoundException) {
            return null
        }
        return entries.build()
    }

    /** Library paths in Liked Songs, without keeping its entries; empty when it does not exist. */
    fun likedPaths(): Set<String> {
        val paths = HashSet<String>()
        try {
            stream(file(LibraryFormat.LIKED_SONGS), keepExtinf = false) { entry -> entry.path?.let(paths::add) }
        } catch (_: java.io.FileNotFoundException) {}
        return paths
    }

    /**
     * Entries to change. A missing file is empty; a file that cannot be read or is too large is never rewritten from a partial read, so the
     * change is refused. The version read becomes the playlist's base ([PlaylistBases.beforeEdit]).
     */
    private fun entries(name: String): PlaylistEntries {
        val digest = MessageDigest.getInstance("SHA-256")
        val entries =
            try {
                read(name, keepExtinf = true, digest)?.entries
            } catch (error: PlaylistTooLargeException) {
                throw PlaylistException("\"$name\" is too large to change on this device", error)
            } catch (error: IOException) {
                throw PlaylistException("Cannot read \"$name\": ${error.message ?: error.javaClass.simpleName}", error)
            } ?: return PlaylistEntries.empty(root.absolutePath, keepExtinf = true)
        bases?.beforeEdit(fileName(name), entries, digest.digest())
        return entries
    }

    private fun fileName(name: String) = name + LibraryFormat.PLAYLIST_EXTENSION

    /** Writes [entries]; a playlist that would be over the size limits is refused and the file left as it is. */
    private fun write(name: String, entries: List<PlaylistEntry>) {
        val text =
            try {
                M3u8.write(entries, lookup)
            } catch (error: PlaylistTooLargeException) {
                throw PlaylistException("\"$name\" would be too large (${error.reason})", error)
            }
        val bytes = text.toByteArray(Charsets.UTF_8)
        AtomicFiles.write(file(name), bytes)
        bases?.written(fileName(name), bytes)
    }

    /**
     * The playlist whose name equals [name] ignoring letter case (Liked Songs included, whether or not its file exists yet). Shared storage
     * is case-insensitive, so such a playlist is the same file.
     */
    private fun existingName(name: String): String? {
        if (name.equals(LibraryFormat.LIKED_SONGS, ignoreCase = true)) return LibraryFormat.LIKED_SONGS
        return directory.list()?.firstNotNullOfOrNull { fileName ->
            fileName
                .takeIf {
                    it.endsWith(LibraryFormat.PLAYLIST_EXTENSION) && !LibraryFormat.isHidden(it) && !LibraryFormat.isConflictCopy(it)
                }
                ?.removeSuffix(LibraryFormat.PLAYLIST_EXTENSION)
                ?.takeIf { it.equals(name, ignoreCase = true) }
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
        bases?.rename(fileName(from), fileName(finalName))
        return finalName
    }

    fun delete(name: String) {
        if (name == LibraryFormat.LIKED_SONGS) throw PlaylistException("Liked Songs cannot be deleted")
        if (!file(name).delete() && file(name).exists()) throw PlaylistException("Could not delete \"$name\"")
        bases?.delete(fileName(name))
    }

    /** Whether `Playlists/` holds any Syncthing conflict copy of a playlist (names only; nothing is read). */
    fun hasConflictCopies(): Boolean = directory.list()?.any { PlaylistMerge.conflictOf(it) != null } == true

    /**
     * Merges the conflict copies of playlists into them (contract: "Playlist conflict copies") and deletes them; returns the names of the
     * playlists written. This device does not know its Syncthing device ID, so it merges only copies modified at least [CONFLICT_AGE_MS]
     * before [now]. A copy is merged only when its playlist exists; a too-large or unreadable playlist leaves its copies alone, and a
     * too-large or unreadable copy, or one whose result would be over the limits, is skipped and stays. Copies of one playlist go one at a
     * time in byte order of their names, at most [MAX_COPIES_PER_PASS] per call, each against the playlist's base as it was before the
     * first of them, in memory; then the playlist is written once, and the merged playlist becomes the base unless a copy stays (skipped,
     * or left for the next call). The result is written without library lookups, so every device writes the same bytes.
     */
    fun mergeConflicts(now: Long = System.currentTimeMillis()): Set<String> {
        val bases = bases ?: return emptySet()
        val copies = HashMap<String, MutableList<String>>()
        for (fileName in directory.list().orEmpty()) {
            val name = PlaylistMerge.conflictOf(fileName)
            if (name != null && isDue(File(directory, fileName), name, now)) copies.getOrPut(name) { ArrayList() } += fileName
        }
        return copies.mapNotNullTo(HashSet()) { (name, names) -> name.takeIf { mergeCopies(bases, name, names) } }
    }

    /** Whether [copy] of playlist [name] is old enough to merge and its playlist exists. */
    private fun isDue(copy: File, name: String, now: Long): Boolean =
        copy.isFile && now - copy.lastModified() >= CONFLICT_AGE_MS && file(name).isFile

    /**
     * Merges the first [MAX_COPIES_PER_PASS] of the copies [names] of playlist [name] one at a time, skipping those that cannot be merged;
     * returns whether it was written. While a copy stays (skipped, or left for the next pass) the base stays, so it is merged against the
     * same base later and the entries the other copies added are not taken for its removals.
     */
    private fun mergeCopies(bases: PlaylistBases, name: String, names: List<String>): Boolean {
        var current = readOrNull(file(name)) ?: return false
        val base = bases.read(fileName(name), root.absolutePath)
        var written: ByteArray? = null
        var skipped = names.size > MAX_COPIES_PER_PASS
        val merged = ArrayList<File>()
        for (copyName in names.sortedWith { a, b -> compareBytes(a.toByteArray(), b.toByteArray()) }.take(MAX_COPIES_PER_PASS)) {
            val copy = File(directory, copyName)
            val result = readOrNull(copy)?.let { withoutLineBreaks(PlaylistMerge.merge(base, current, it)) }
            val bytes = result?.let(::serialised)
            if (result != null && bytes != null) {
                current = result
                written = bytes
                merged += copy
            } else if (copy.exists()) {
                // Not gone (merged by another device and the deletion synced): it stays.
                skipped = true
            }
        }
        written?.let {
            AtomicFiles.write(file(name), it)
            for (copy in merged) copy.delete()
            if (skipped) bases.written(fileName(name), it) else bases.merged(fileName(name), it)
        }
        return written != null
    }

    /** [entries] without those writers leave out (a line break in the line), as the next copy would read them back from the file. */
    private fun withoutLineBreaks(entries: PlaylistEntries): PlaylistEntries {
        fun writable(i: Int) = !LibraryFormat.hasLineBreak(entries.path(i) ?: entries.raw(i))
        if ((0 until entries.size).all(::writable)) return entries
        val kept = entries.builder(entries.size)
        for (i in 0 until entries.size) if (writable(i)) kept.add(entries, i)
        return kept.build()
    }

    /** [file]'s entries for a writer, or null when it is gone, too large or unreadable. */
    private fun readOrNull(file: File): PlaylistEntries? =
        try {
            readFile(file)
        } catch (_: IOException) {
            null
        }

    /** A merge result as every device writes it (no library lookups), or null when it would be over the limits. */
    private fun serialised(entries: PlaylistEntries): ByteArray? =
        try {
            M3u8.write(entries) { null }.toByteArray(Charsets.UTF_8)
        } catch (_: PlaylistTooLargeException) {
            null
        }

    /** Appends [tracks]; a track whose path contains a line break is skipped (see [LibraryFormat.hasLineBreak]). */
    fun add(name: String, tracks: List<Track>) {
        val current = entries(name)
        val updated = current.builder(current.size + tracks.size).addAll(current)
        for (track in tracks) if (M3u8.canWrite(track.path)) updated.add(M3u8.entryFor(track), canonical(track.path))
        write(name, updated.build())
    }

    /**
     * Removes the entry the user saw at [entry]. The file is read again first, so an entry another device moved is found by its key; one
     * that is gone fails without writing.
     */
    fun remove(name: String, entry: EntryRef) {
        val entries = entries(name)
        val at = locate(entries, listOf(entry))[0]
        write(name, entries.builder(entries.size).addAll(entries, 0 until at).addAll(entries, at + 1 until entries.size).build())
    }

    /** Moves the entry the user saw at [from] to where the entry they saw at [to] is now (see [remove]). */
    fun move(name: String, from: EntryRef, to: EntryRef) {
        val entries = entries(name)
        val (source, target) = locate(entries, listOf(from, to))
        if (source == target) return
        val moved = entries.builder(entries.size)
        if (source < target) {
            moved
                .addAll(entries, 0 until source)
                .addAll(entries, source + 1..target)
                .add(entries, source)
                .addAll(entries, target + 1 until entries.size)
        } else {
            moved
                .addAll(entries, 0 until target)
                .add(entries, source)
                .addAll(entries, target until source)
                .addAll(entries, source + 1 until entries.size)
        }
        write(name, moved.build())
    }

    fun setLiked(track: Track, liked: Boolean) {
        val current = entries(LibraryFormat.LIKED_SONGS)
        val updated = if (liked) like(current, track, canonical(track.path)) else unlike(current, track.path)
        if (updated !== current || !file(LibraryFormat.LIKED_SONGS).exists()) write(LibraryFormat.LIKED_SONGS, updated)
    }

    companion object {
        /** Candidate tracks kept per listed playlist for its cover art (the first one that is available). */
        const val ART_PATHS = 8

        /** How old a conflict copy must be before this device merges it (contract: "Who merges"). */
        const val CONFLICT_AGE_MS = 10 * 60 * 1000L

        /** At most this many copies of one playlist are merged in one pass; the rest wait for the next (contract: "How"). */
        const val MAX_COPIES_PER_PASS = 32

        /** Unsigned byte order, as the contract orders conflict copies. */
        private fun compareBytes(a: ByteArray, b: ByteArray): Int {
            for (i in 0 until minOf(a.size, b.size)) {
                val order = (a[i].toInt() and 0xFF) - (b[i].toInt() and 0xFF)
                if (order != 0) return order
            }
            return a.size - b.size
        }

        /**
         * Liking inserts the track at the top unless it is already liked ([entries] itself is returned then). [libraryPath] is the library
         * index's string for the track's path, shared by the new entry.
         */
        fun like(entries: PlaylistEntries, track: Track, libraryPath: String? = null): PlaylistEntries =
            when {
                !M3u8.canWrite(track.path) || (0 until entries.size).any { entries.path(it) == track.path } -> entries
                else -> entries.builder().add(M3u8.entryFor(track), libraryPath).addAll(entries).build()
            }

        /** Unliking removes every entry for the track ([entries] itself is returned when there is none). */
        fun unlike(entries: PlaylistEntries, path: String): PlaylistEntries {
            if ((0 until entries.size).none { entries.path(it) == path }) return entries
            val updated = entries.builder(entries.size)
            for (index in 0 until entries.size) if (entries.path(index) != path) updated.add(entries, index)
            return updated.build()
        }

        /**
         * Finds shown entries in freshly read [entries]: at their position if it still holds the same entry, else the first unclaimed entry
         * with that key. Throws when one is gone, so nothing wrong is changed.
         */
        fun locate(entries: PlaylistEntries, refs: List<EntryRef>): List<Int> {
            val found = ArrayList<Int>(refs.size)
            for (ref in refs) {
                fun same(i: Int) = i in entries.indices && entries.key(i) == ref.key
                val at =
                    if (same(ref.position) && ref.position !in found) {
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
