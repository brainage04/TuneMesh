package io.github.brainage04.tunemesh

import android.content.Intent
import android.graphics.Typeface
import android.os.Parcel
import android.text.SpannableString
import android.text.Spanned
import android.text.style.StyleSpan
import androidx.test.ext.junit.runners.AndroidJUnit4
import leakcanary.DetectLeaksAfterTestSuccess
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/** What the share sheet takes from an `ACTION_SEND` intent. */
@RunWith(AndroidJUnit4::class)
class ShareInputTest {
    @get:Rule val leaks = DetectLeaksAfterTestSuccess()

    private fun share(text: CharSequence?): Intent {
        val intent = Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, text)
        // Through a parcel, as between the sharing app and TuneMesh.
        val parcel = Parcel.obtain()
        try {
            intent.writeToParcel(parcel, 0)
            parcel.setDataPosition(0)
            return Intent.CREATOR.createFromParcel(parcel)
        } finally {
            parcel.recycle()
        }
    }

    @Test
    fun styledTextIsRead() {
        val styled = SpannableString("Listen to this https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC?si=1 now")
        styled.setSpan(StyleSpan(Typeface.BOLD), 0, 6, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE)
        val intent = share(styled)
        assertEquals(null, intent.getStringExtra(Intent.EXTRA_TEXT))
        assertEquals("https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC?si=1", ShareActivity.sharedInput(intent))
    }

    @Test
    fun plainTextAndSearches() {
        assertEquals("https://youtu.be/jNQXAC9IVRw", ShareActivity.sharedInput(share("https://youtu.be/jNQXAC9IVRw")))
        assertEquals("artist – song", ShareActivity.sharedInput(share("  artist – song \n")))
        assertEquals("", ShareActivity.sharedInput(share(null)))
        assertEquals("", ShareActivity.sharedInput(null))
    }

    /** Punctuation that ends the sentence around a link is not part of it; brackets the link opened stay. */
    @Test
    fun trailingPunctuationIsDropped() {
        fun input(text: String) = ShareActivity.sharedInput(share(text))
        assertEquals("https://youtu.be/jNQXAC9IVRw", input("Watch https://youtu.be/jNQXAC9IVRw."))
        assertEquals("https://youtu.be/jNQXAC9IVRw", input("(https://youtu.be/jNQXAC9IVRw)!"))
        assertEquals("https://youtu.be/jNQXAC9IVRw", input("\"https://youtu.be/jNQXAC9IVRw\","))
        assertEquals("https://youtu.be/jNQXAC9IVRw", input("<https://youtu.be/jNQXAC9IVRw>;"))
        assertEquals("https://youtu.be/jNQXAC9IVRw", input("[https://youtu.be/jNQXAC9IVRw]?"))
        assertEquals("https://youtu.be/jNQXAC9IVRw", input("'https://youtu.be/jNQXAC9IVRw':"))
        assertEquals("https://en.wikipedia.org/wiki/Song_(band)", input("See https://en.wikipedia.org/wiki/Song_(band)."))
        assertEquals("https://example.com/a?b=c", input("https://example.com/a?b=c?!"))
        assertEquals("https://music.youtube.com/watch?v=x&list=y", input("https://music.youtube.com/watch?v=x&list=y"))
    }
}
