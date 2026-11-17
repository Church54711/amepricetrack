package app.amepricetrack

import app.amepricetrack.data.isNewerVersion
import app.amepricetrack.data.parseAppRelease
import app.amepricetrack.data.parseServerUpdate
import app.amepricetrack.data.versionNumbers
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class UpdatesTest {
    @Test
    fun versionsCompareByTheirNumbers() {
        assertTrue(isNewerVersion("0.2.0", "0.1.0"))
        assertTrue(isNewerVersion("v0.10.0", "0.9.3"))
        assertTrue(isNewerVersion("1.0", "0.9.9"))
        assertFalse(isNewerVersion("0.2.0", "0.2.0"))
        assertFalse(isNewerVersion("0.1.9", "0.2.0"))
        assertFalse(isNewerVersion("nightly", "0.2.0"))
        assertNull(versionNumbers(""))
        assertEquals(listOf(1, 2, 3), versionNumbers("v1.2.3-beta"))
    }

    @Test
    fun theAppIsFoundAmongAReleasesFiles() {
        val json = JSONObject(
            """{"tag_name":"v0.2.0","assets":[
                {"name":"amepricetrack-linux-x86_64.tar.gz","browser_download_url":"https://github.com/o/r/releases/download/v0.2.0/amepricetrack-linux-x86_64.tar.gz","size":9000000},
                {"name":"AmePriceTrack.apk","browser_download_url":"https://github.com/o/r/releases/download/v0.2.0/AmePriceTrack.apk","size":12345678}]}""",
        )
        val release = parseAppRelease(json)!!
        assertEquals("0.2.0", release.version)
        assertEquals(12345678L, release.bytes)
        assertTrue(release.apkUrl.endsWith("/AmePriceTrack.apk"))
    }

    @Test
    fun aReleaseWithoutTheAppOrWithAnOddAddressIsNotOffered() {
        assertNull(parseAppRelease(JSONObject("""{"tag_name":"v0.2.0","assets":[]}""")))
        assertNull(parseAppRelease(JSONObject("""{"message":"Not Found"}""")))
        val plain = """{"tag_name":"v0.2.0","assets":[{"name":"a.apk","browser_download_url":"http://example.com/a.apk"}]}"""
        assertNull(parseAppRelease(JSONObject(plain)))
    }

    @Test
    fun theServersAnswerIsRead() {
        val json = JSONObject(
            """{"version":"0.1.0","newer":{"version":"0.2.0","page":"https://github.com/o/r/releases/tag/v0.2.0"},
                "can_install":true,"last":{"state":"failed","message":"No space left","at":5}}""",
        )
        val update = parseServerUpdate(json)
        assertEquals("0.1.0", update.version)
        assertEquals("0.2.0", update.newer)
        assertTrue(update.canInstall)
        assertEquals("No space left", update.lastProblem)

        val current = parseServerUpdate(JSONObject("""{"version":"0.2.0","newer":null,"can_install":false,"last":null}"""))
        assertNull(current.newer)
        assertNull(current.lastProblem)
    }
}
