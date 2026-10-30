package io.github.brainage04.spotless.data

import android.media.MediaMetadataRetriever
import io.github.brainage04.spotless.library.AtomicFiles
import io.github.brainage04.spotless.library.DirectoryStamps
import io.github.brainage04.spotless.library.FoundFiles
import io.github.brainage04.spotless.library.LibraryFormat
import io.github.brainage04.spotless.library.LibraryIndex
import io.github.brainage04.spotless.library.LibraryIndexFile
import io.github.brainage04.spotless.library.LibraryScan
import io.github.brainage04.spotless.library.Limits
import io.github.brainage04.spotless.library.RawTags
import java.io.File
import java.io.IOException
import java.nio.file.FileVisitResult
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.SimpleFileVisitor
import java.nio.file.attribute.BasicFileAttributes

/** A scan's [index], and the [directories] it walked with their modification times (what the periodic check compares). */
class ScanResult(val index: LibraryIndex, val directories: DirectoryStamps)

/**
 * Walks the library and reads tags for new or changed files only: a file whose path, size and modification time are those of a track of the
 * index it is given keeps that track. The index of the last scan is saved in app storage ([LibraryIndexFile], in [directory]), so a start
 * shows it at once; it is the only copy of the tags the app keeps.
 */
class LibraryScanner(private val directory: File) {
    private val cacheFile = File(directory, CACHE_FILE)

    /** What [cacheFile] holds. */
    private var saved: LibraryIndex? = null

    /**
     * The index the last scan of [root] saved, or null. With none saved yet, the JSON tag cache of earlier versions is converted once (and
     * deleted), so an update does not read every file's tags again.
     */
    @Synchronized
    fun cached(root: File): LibraryIndex? {
        val rootPath = root.absolutePath
        val legacy = File(directory, LEGACY_CACHE_FILE)
        val index =
            readSaved(rootPath)
                ?: legacy
                    .takeIf { it.isFile }
                    ?.let { file ->
                        val migrated =
                            try {
                                LibraryScan.fromJsonTagCache(file.readText(), rootPath)
                            } catch (_: IOException) {
                                null
                            }
                        migrated?.also { save(it, rootPath) }
                    }
        legacy.delete()
        return index
    }

    private fun readSaved(root: String): LibraryIndex? =
        try {
            cacheFile.inputStream().use { LibraryIndexFile.read(it, root, cacheFile.length()) }?.also { saved = it }
        } catch (_: IOException) {
            null
        }

    /**
     * Scans [root]. [previous] is the index shown for it: its tracks whose files did not change are kept without reading their tags, and
     * when nothing changed the result is [previous] itself. With nothing shown yet (an empty [previous]: a scan that starts before the
     * saved index is shown), the saved index takes its place, so a start never reads every file's tags again.
     */
    @Synchronized
    fun scan(root: File, previous: LibraryIndex): ScanResult {
        val rootPath = root.toPath()
        val found = FoundFiles()
        val directories = DirectoryStamps()
        if (root.isDirectory) {
            // Entries seen so far in each open directory, innermost last (Limits.MAX_FOLDER_ENTRIES).
            val seen = ArrayDeque<IntArray>()

            /** Counts one entry of the current directory; false once the rest of it must be skipped. */
            fun count(): Boolean {
                val counter = seen.lastOrNull() ?: return true
                return ++counter[0] <= Limits.MAX_FOLDER_ENTRIES
            }

            Files.walkFileTree(
                rootPath,
                object : SimpleFileVisitor<Path>() {
                    override fun preVisitDirectory(dir: Path, attrs: BasicFileAttributes): FileVisitResult {
                        if (!count()) return FileVisitResult.SKIP_SIBLINGS
                        val relativePath = rootPath.relativize(dir)
                        val relative = relativePath.toString()
                        if (relative.isNotEmpty() && LibraryFormat.isExcludedDirectory(relative)) return FileVisitResult.SKIP_SUBTREE
                        if (relative.isNotEmpty() && relativePath.nameCount > Limits.MAX_DEPTH) return FileVisitResult.SKIP_SUBTREE
                        directories.add(dir, attrs.lastModifiedTime().toMillis())
                        seen.addLast(IntArray(1))
                        return FileVisitResult.CONTINUE
                    }

                    override fun postVisitDirectory(dir: Path, exc: IOException?): FileVisitResult {
                        seen.removeLastOrNull()
                        return FileVisitResult.CONTINUE
                    }

                    override fun visitFile(file: Path, attrs: BasicFileAttributes): FileVisitResult {
                        if (!count()) return FileVisitResult.SKIP_SIBLINGS
                        if (attrs.isRegularFile) {
                            val relative = rootPath.relativize(file).toString()
                            if (LibraryFormat.isAudioPath(relative)) {
                                found.add(relative, attrs.size(), attrs.lastModifiedTime().toMillis())
                                if (found.size >= Limits.MAX_AUDIO_FILES) return FileVisitResult.TERMINATE
                            }
                        }
                        return FileVisitResult.CONTINUE
                    }

                    override fun visitFileFailed(file: Path, exc: IOException): FileVisitResult =
                        if (count()) FileVisitResult.CONTINUE else FileVisitResult.SKIP_SIBLINGS
                },
            )
        }

        val base = if (previous.size > 0) previous else readSaved(root.absolutePath) ?: previous
        val index = LibraryScan.index(base, found) { path -> readTags(File(root, path)) }
        if (index !== saved) save(index, root.absolutePath)
        return ScanResult(index, directories)
    }

    private fun save(index: LibraryIndex, root: String) {
        try {
            AtomicFiles.write(cacheFile) { LibraryIndexFile.write(index, root, it) }
            saved = index
        } catch (_: IOException) {
            // The cache is an optimisation; the next scan saves it again.
        }
    }

    companion object {
        /** The saved index's file name in the app's files directory. */
        const val CACHE_FILE = "library-index.bin"

        /** The JSON tag cache of earlier versions. */
        private const val LEGACY_CACHE_FILE = "tag-cache.json"

        fun readTags(file: File): RawTags =
            MediaMetadataRetriever().use { retriever ->
                try {
                    retriever.setDataSource(file.absolutePath)
                    // A text item over Limits.MAX_TAG_ITEM_BYTES is not used; the others are cut to MAX_TAG_TEXT_BYTES.
                    fun text(key: Int) =
                        retriever
                            .extractMetadata(key)
                            ?.takeUnless { it.length > Limits.MAX_TAG_ITEM_BYTES / 4 && Limits.utf8Length(it) > Limits.MAX_TAG_ITEM_BYTES }
                            ?.let { RawTags.boundedText(it) }
                            ?.takeIf { it.isNotEmpty() }
                    RawTags(
                        title = text(MediaMetadataRetriever.METADATA_KEY_TITLE),
                        artist = text(MediaMetadataRetriever.METADATA_KEY_ARTIST),
                        album = text(MediaMetadataRetriever.METADATA_KEY_ALBUM),
                        albumArtist = text(MediaMetadataRetriever.METADATA_KEY_ALBUMARTIST),
                        trackNumber = text(MediaMetadataRetriever.METADATA_KEY_CD_TRACK_NUMBER)?.leadingInt(),
                        discNumber = text(MediaMetadataRetriever.METADATA_KEY_DISC_NUMBER)?.leadingInt(),
                        durationMs = text(MediaMetadataRetriever.METADATA_KEY_DURATION)?.toLongOrNull() ?: 0,
                    )
                } catch (_: RuntimeException) {
                    RawTags()
                }
            }

        private fun String.leadingInt(): Int? = substringBefore('/').trim().toIntOrNull()
    }
}
