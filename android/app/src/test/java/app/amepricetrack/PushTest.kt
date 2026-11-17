package app.amepricetrack

import app.amepricetrack.data.newNtfyAddress
import app.amepricetrack.data.ntfyAppLink
import app.amepricetrack.data.paceChoices
import app.amepricetrack.data.paceText
import app.amepricetrack.data.parseMe
import app.amepricetrack.data.productIdFromAppLink
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Pushed alerts through ntfy, and how often a product is checked. */
class PushTest {
    @Test
    fun aNewTopicAddressIsLongRandomAndOnlyUsesWhatNtfyAllows() {
        val address = newNtfyAddress()
        assertTrue(address, Regex("""https://ntfy\.sh/apt-[a-z0-9]{24}""").matches(address))
        // ntfy topic names may be at most 64 characters.
        assertTrue(address.substringAfterLast('/').length <= 64)
        assertNotEquals(address, newNtfyAddress())
    }

    @Test
    fun aTopicAddressBecomesALinkThatOpensTheNtfyApp() {
        assertEquals("ntfy://ntfy.sh/apt-abc123", ntfyAppLink("https://ntfy.sh/apt-abc123"))
        assertEquals("ntfy://ntfy.sh/apt-abc123", ntfyAppLink(" https://ntfy.sh/apt-abc123/ "))
        // A server of your own without HTTPS has to be marked as such for the app.
        assertEquals("ntfy://192.168.1.20:8080/prices?secure=false", ntfyAppLink("http://192.168.1.20:8080/prices"))
        for (notATopic in listOf("https://ntfy.sh/", "https://ntfy.sh/a/b", "ntfy.sh/topic", "ftp://ntfy.sh/topic", "not an address")) {
            assertNull(notATopic, ntfyAppLink(notATopic))
        }
    }

    @Test
    fun aPushedAlertsButtonOpensTheRightProduct() {
        assertEquals(12L, productIdFromAppLink("amepricetrack://products/12"))
        assertEquals(12L, productIdFromAppLink("amepricetrack://products/12/"))
        for (other in listOf(null, "", "amepricetrack://products/", "amepricetrack://products/abc", "amepricetrack://settings/1", "https://example.com/products/12")) {
            assertNull(other, productIdFromAppLink(other))
        }
    }

    @Test
    fun onlyPacesCloserThanTheServersOwnAreOffered() {
        assertEquals(listOf(30, 60, 120), paceChoices(usualMinutes = 240, saved = null))
        assertEquals("a server that already checks hourly", listOf(30), paceChoices(usualMinutes = 60, saved = null))
        // Whatever is saved stays choosable, even if it was set some other way.
        assertEquals(listOf(30, 45, 60, 120), paceChoices(usualMinutes = 240, saved = 45))
        assertEquals(listOf(30, 60, 120), paceChoices(usualMinutes = 240, saved = 60))

        assertEquals("30 minutes", paceText(30))
        assertEquals("hour", paceText(60))
        assertEquals("2 hours", paceText(120))
        assertEquals("4 hours", paceText(240))
        assertEquals("90 minutes", paceText(90))
    }

    @Test
    fun theServersUsualPaceIsReadAndAssumedWhenAnOlderServerDoesNotSay() {
        assertEquals(60, parseMe(JSONObject("""{"username":"sam","check_interval_minutes":60}""")).checkIntervalMinutes)
        assertEquals(240, parseMe(JSONObject("""{"username":"sam"}""")).checkIntervalMinutes)
    }
}
