package io.github.brainage04.tunemesh.ui

import java.util.Locale
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Test

class FormatDurationTest {
    private val previous = Locale.getDefault()

    @After
    fun restoreLocale() {
        Locale.setDefault(previous)
    }

    /** detekt ImplicitDefaultLocale: times were formatted with the device locale, so Arabic devices got mixed digit sets. */
    @Test
    fun digitsDoNotFollowTheDeviceLocale() {
        val arabic = Locale.forLanguageTag("ar-EG")
        Locale.setDefault(arabic)
        assertNotEquals("the locale must format digits differently for this test to mean anything", "5", "%d".format(arabic, 5))
        assertEquals("1:05", formatDuration(65_000))
        assertEquals("1:01:05", formatDuration(3_665_000))
    }
}
