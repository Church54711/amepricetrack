package app.amepricetrack

import app.amepricetrack.data.ApiClient
import app.amepricetrack.data.ApiException
import app.amepricetrack.data.WatchSettings
import kotlinx.coroutines.runBlocking
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import org.json.JSONObject
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test

/** The app's requests, checked against a stand-in server on this machine. */
class ApiClientTest {
    private lateinit var server: MockWebServer
    private var token: String? = "tok123"
    private lateinit var api: ApiClient

    @Before
    fun start() {
        server = MockWebServer()
        server.start()
        api = ApiClient(serverUrl = { server.url("").toString().trimEnd('/') }, token = { token })
    }

    @After
    fun stop() {
        server.shutdown()
    }

    private fun problemOf(block: suspend () -> Unit): ApiException {
        try {
            runBlocking { block() }
        } catch (e: ApiException) {
            return e
        }
        fail("expected the call to fail")
        throw IllegalStateException()
    }

    @Test
    fun signInSendsTheDeviceNameAndReturnsTheToken() = runBlocking {
        token = null
        server.enqueue(MockResponse().setBody("""{"token":"abc","username":"sam"}"""))
        val address = server.url("").toString().trimEnd('/')
        assertEquals("abc", api.login(address, "sam", "longenough", "Pixel 9"))
        val request = server.takeRequest()
        assertEquals("/api/v1/login", request.path)
        assertEquals(null, request.getHeader("Authorization"))
        val body = JSONObject(request.body.readUtf8())
        assertEquals("sam", body.getString("username"))
        assertEquals("Pixel 9", body.getString("device_name"))
    }

    @Test
    fun signedInCallsCarryTheToken() = runBlocking {
        server.enqueue(MockResponse().setBody("""{"watches":[]}"""))
        assertTrue(api.watches().isEmpty())
        val request = server.takeRequest()
        assertEquals("GET", request.method)
        assertEquals("/api/v1/watches", request.path)
        assertEquals("Bearer tok123", request.getHeader("Authorization"))
    }

    @Test
    fun alertSettingsAreSentWithEmptyFieldsAsNull() = runBlocking {
        server.enqueue(
            MockResponse().setBody(
                """{"product":{"id":7,"retailer":"staples","url":"u"},"watch":{},"price":{},"deal":{},"elsewhere":null,"store_name":"Staples"}""",
            ),
        )
        api.updateWatch(7, WatchSettings(targetPriceCents = 4999, dropPercent = null, notifyAnyDrop = false, notifyBackInStock = true, note = null))
        val request = server.takeRequest()
        assertEquals("/api/v1/products/7/watch", request.path)
        val body = JSONObject(request.body.readUtf8())
        assertEquals(4999, body.getInt("target_price_cents"))
        assertTrue(body.isNull("drop_percent"))
        assertTrue(body.isNull("note"))
        assertFalse(body.getBoolean("notify_any_drop"))
        assertTrue(body.getBoolean("notify_back_in_stock"))
    }

    @Test
    fun aPageFromThePhoneIsSentWhole() = runBlocking {
        server.enqueue(MockResponse().setBody("""{"product_id":7,"price_cents":7500,"recorded_as":"member"}"""))
        val html = "<html>" + "x".repeat(200_000) + "</html>"
        val result = api.ingest("https://www.staples.com/chair/product_555", html, signedIn = true)
        assertEquals(7500L, result.priceCents)
        assertEquals("member", result.recordedAs)
        val body = JSONObject(server.takeRequest().body.readUtf8())
        assertEquals(html, body.getString("html"))
        assertTrue(body.getBoolean("signed_in"))
    }

    @Test
    fun theServersOwnWordsAreShownForAProblem() {
        server.enqueue(MockResponse().setResponseCode(400).setBody("""{"error":"That Staples link is not a product page."}"""))
        val problem = problemOf { api.addWatch("https://www.staples.com/") }
        assertEquals("That Staples link is not a product page.", problem.message)
        assertFalse(problem.signedOut)
    }

    @Test
    fun aRevokedDeviceIsToldItIsSignedOut() {
        server.enqueue(MockResponse().setResponseCode(401).setBody("""{"error":"Sign in first."}"""))
        assertTrue(problemOf { api.watches() }.signedOut)
    }

    @Test
    fun somethingThatIsNotTheServerIsExplained() {
        server.enqueue(MockResponse().setBody("<html>Router login</html>"))
        assertEquals("That address answered, but not like an AmePriceTrack server.", problemOf { api.me() }.message)

        server.shutdown()
        val unreachable = problemOf { api.me() }.message!!
        assertTrue(unreachable, unreachable.startsWith("Could not reach the server."))
    }
}
