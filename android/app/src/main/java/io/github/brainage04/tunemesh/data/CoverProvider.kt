package io.github.brainage04.tunemesh.data

import android.content.ContentProvider
import android.content.ContentValues
import android.content.Context
import android.database.Cursor
import android.net.Uri
import android.os.ParcelFileDescriptor
import io.github.brainage04.tunemesh.TuneMeshApp
import java.io.FileNotFoundException

/**
 * Serves cached album art to the media session, notification and lock screen so queue items only carry a URI
 * and covers are extracted lazily. Exported only to holders of MEDIA_CONTENT_CONTROL (System UI), read-only,
 * and only for audio files inside the library: `path` cannot reach other files the app can read.
 */
class CoverProvider : ContentProvider() {
    override fun onCreate(): Boolean = true

    override fun openFile(uri: Uri, mode: String): ParcelFileDescriptor {
        if (mode != "r") throw SecurityException("Covers are read-only")
        val path = uri.getQueryParameter(PARAM_PATH) ?: throw FileNotFoundException(uri.toString())
        val app = context?.applicationContext as? TuneMeshApp ?: throw FileNotFoundException(uri.toString())
        val root = app.repository.root
        val track = app.repository.track(path) ?: throw FileNotFoundException("Not a library track: $path")
        val cover = app.covers.diskFile(root, track) ?: throw FileNotFoundException("No cover for $path")
        return ParcelFileDescriptor.open(cover, ParcelFileDescriptor.MODE_READ_ONLY)
    }

    override fun getType(uri: Uri): String = "image/jpeg"

    override fun query(uri: Uri, projection: Array<out String>?, selection: String?, selectionArgs: Array<out String>?, sortOrder: String?): Cursor? = null

    override fun insert(uri: Uri, values: ContentValues?): Uri? = null

    override fun delete(uri: Uri, selection: String?, selectionArgs: Array<out String>?): Int = 0

    override fun update(uri: Uri, values: ContentValues?, selection: String?, selectionArgs: Array<out String>?): Int = 0

    companion object {
        private const val PARAM_PATH = "path"

        fun uri(context: Context, path: String): Uri = Uri.Builder()
            .scheme("content")
            .authority("${context.packageName}.covers")
            .path("cover")
            .appendQueryParameter(PARAM_PATH, path)
            .build()
    }
}
