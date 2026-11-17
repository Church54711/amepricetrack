package app.amepricetrack

import app.amepricetrack.data.SessionState
import app.amepricetrack.ui.Light
import app.amepricetrack.ui.Midnight
import app.amepricetrack.ui.paletteFor
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** Which theme a device shows: the website's, or one chosen for the device alone. */
class LookTest {
    private val website = SessionState(theme = "midnight", accent = "#ff8800")

    @Test
    fun aDeviceLooksLikeTheWebsiteUntilItGetsAThemeOfItsOwn() {
        assertEquals("midnight", website.shownTheme)
        assertEquals("#ff8800", website.shownAccent)

        val own = website.copy(deviceTheme = "light")
        assertEquals("light", own.shownTheme)
        assertNull("the website's accent goes with the website's theme", own.shownAccent)
        // What the website chose is still remembered, for going back.
        assertEquals("midnight", own.theme)
        assertEquals("midnight", own.copy(deviceTheme = null).shownTheme)
    }

    @Test
    fun aChangeOnTheWebsiteDoesNotChangeADeviceWithItsOwnTheme() {
        val own = website.copy(deviceTheme = "light")
        val afterWebsiteChange = own.copy(theme = "oled-gold", accent = null)
        assertEquals("light", afterWebsiteChange.shownTheme)
        assertEquals(Light, paletteFor(afterWebsiteChange.shownTheme, afterWebsiteChange.shownAccent, systemDark = true))
        // "Match my device" follows the phone's own light or dark setting.
        val system = website.copy(deviceTheme = "system")
        assertEquals(Midnight, paletteFor(system.shownTheme, system.shownAccent, systemDark = true))
        assertEquals(Light, paletteFor(system.shownTheme, system.shownAccent, systemDark = false))
    }
}
