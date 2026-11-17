package app.amepricetrack.data

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody
import okhttp3.RequestBody.Companion.toRequestBody
import okio.Buffer
import okio.GzipSink
import okio.buffer
import org.json.JSONObject
import java.io.IOException
import java.net.URLEncoder
import java.util.concurrent.TimeUnit

/** A problem worth showing to the person, in words they can act on. */
class ApiException(message: String, val status: Int? = null) : Exception(message) {
    /** The server no longer accepts this device's sign-in. */
    val signedOut: Boolean get() = status == 401
}

/** Talks to the AmePriceTrack server's phone API. */
class ApiClient(
    private val serverUrl: () -> String?,
    private val token: () -> String?,
    private val http: OkHttpClient = defaultHttp,
) {
    companion object {
        private val JSON = "application/json; charset=utf-8".toMediaType()
        val defaultHttp: OkHttpClient = OkHttpClient.Builder()
            .connectTimeout(10, TimeUnit.SECONDS)
            // The assistant can take a while to answer, most of all a local model.
            .readTimeout(310, TimeUnit.SECONDS)
            .writeTimeout(60, TimeUnit.SECONDS)
            .build()
    }

    /** The same text compressed. A product page shrinks to a fraction of its size. */
    private fun compressed(text: String): RequestBody {
        val packed = Buffer()
        GzipSink(packed).buffer().use { it.writeUtf8(text) }
        return packed.readByteString().toRequestBody(JSON)
    }

    private suspend fun call(
        method: String,
        path: String,
        body: JSONObject? = null,
        server: String? = null,
        compress: Boolean = false,
    ): JSONObject =
        withContext(Dispatchers.IO) {
            val base = server ?: serverUrl() ?: throw ApiException("Not signed in to a server.")
            val request = Request.Builder().url("$base/api/v1/$path").apply {
                token()?.let { header("Authorization", "Bearer $it") }
                when (method) {
                    "GET" -> get()
                    "DELETE" -> delete()
                    else -> {
                        val text = (body ?: JSONObject()).toString()
                        if (compress) {
                            header("Content-Encoding", "gzip")
                            post(compressed(text))
                        } else {
                            post(text.toRequestBody(JSON))
                        }
                    }
                }
            }.build()
            val response = try {
                http.newCall(request).execute()
            } catch (e: IOException) {
                throw ApiException("Could not reach the server. Check the address and that this phone is on the same network.")
            } catch (e: IllegalArgumentException) {
                throw ApiException("That server address does not look right.")
            }
            response.use {
                val text = it.body?.string().orEmpty()
                val json = try {
                    JSONObject(text)
                } catch (e: Exception) {
                    null
                }
                if (!it.isSuccessful) {
                    val reason = json?.optString("error")?.takeIf { message -> message.isNotEmpty() }
                    throw ApiException(reason ?: "The server answered with an error (${it.code}).", it.code)
                }
                json ?: throw ApiException("That address answered, but not like an AmePriceTrack server.")
            }
        }

    /** Signs in and returns the token for this device. */
    suspend fun login(server: String, username: String, password: String, deviceName: String): String {
        val body = JSONObject().put("username", username).put("password", password).put("device_name", deviceName)
        val token = call("POST", "login", body, server).optString("token")
        if (token.isEmpty()) throw ApiException("That address answered, but not like an AmePriceTrack server.")
        return token
    }

    suspend fun logout() {
        call("POST", "logout")
    }

    suspend fun me(): Me = parseMe(call("GET", "me"))

    suspend fun watches(): List<WatchItem> = parseWatchList(call("GET", "watches"))

    /** Starts tracking a product from a link, or from the text a store app shares. */
    suspend fun addWatch(linkOrSharedText: String): WatchItem =
        parseWatchItem(call("POST", "watches", JSONObject().put("url", linkOrSharedText)))

    suspend fun product(id: Long): ProductDetail = parseProductDetail(call("GET", "products/$id"))

    suspend fun updateWatch(id: Long, settings: WatchSettings): WatchItem {
        val body = JSONObject()
            .put("target_price_cents", settings.targetPriceCents ?: JSONObject.NULL)
            .put("drop_percent", settings.dropPercent ?: JSONObject.NULL)
            .put("notify_any_drop", settings.notifyAnyDrop)
            .put("notify_back_in_stock", settings.notifyBackInStock)
            .put("note", settings.note ?: JSONObject.NULL)
            .put("check_minutes", settings.checkMinutes ?: JSONObject.NULL)
            // An empty day means none; leaving the field out would keep the old one.
            .put("buy_by", settings.buyBy?.let(::dayText).orEmpty())
        return parseWatchItem(call("POST", "products/$id/watch", body))
    }

    suspend fun removeWatch(id: Long) {
        call("DELETE", "products/$id/watch")
    }

    suspend fun checkNow(id: Long) {
        call("POST", "products/$id/check")
    }

    suspend fun compareAgain(id: Long) {
        call("POST", "products/$id/compare")
    }

    /** Says whether a suggested match at another store is the same product. */
    suspend fun decideComparison(comparisonId: Long, same: Boolean) {
        call("POST", "comparisons/$comparisonId", JSONObject().put("same", same))
    }

    suspend fun alerts(after: Long = 0): AlertList = parseAlerts(call("GET", "notifications?after=$after"))

    suspend fun markAlertsRead() {
        call("POST", "notifications/read")
    }

    suspend fun collect(): List<CollectEntry> = parseCollect(call("GET", "collect"))

    /** Sends a product page this phone loaded, so the server can read the price on it. */
    suspend fun ingest(url: String, html: String, signedIn: Boolean): IngestResult {
        val body = JSONObject().put("url", url).put("html", html).put("signed_in", signedIn)
        val result = call("POST", "ingest", body, compress = true)
        return IngestResult(
            priceCents = if (result.isNull("price_cents")) null else result.optLong("price_cents"),
            recordedAs = result.optString("recorded_as"),
        )
    }

    /** Starts a find: the server tidies the words and says which stores to ask. */
    suspend fun findStart(words: String): FindStart = parseFindStart(call("GET", "find?q=${encode(words)}"))

    /** One store's first results for a name or barcode. */
    suspend fun findAt(words: String, storeId: String): FindAnswer =
        parseFindAnswer(call("GET", "find?q=${encode(words)}&store=${encode(storeId)}"))

    /** Has the server read both sides of a comparison again now. */
    suspend fun refreshComparison(comparisonId: Long) {
        call("POST", "comparisons/$comparisonId/refresh")
    }

    /** Where the server pushes this person's alerts. */
    suspend fun pushSettings(): PushSettings = parsePushSettings(call("GET", "push"))

    /** Has the server push alerts to this ntfy topic address; an empty one turns that off. */
    suspend fun setNtfy(address: String): PushSettings = parsePushSettings(call("POST", "push", JSONObject().put("ntfy_url", address)))

    /** Has the server push a test alert now. */
    suspend fun testPush() {
        call("POST", "push/test")
    }

    private fun encode(text: String): String = URLEncoder.encode(text, "UTF-8")

    /** The server's version, and whether a newer one can be installed from here. */
    suspend fun serverUpdate(): ServerUpdate = parseServerUpdate(call("GET", "update"))

    /** Has the server install the latest release. It restarts about a minute later. */
    suspend fun updateServer() {
        call("POST", "update")
    }

    suspend fun chat(message: String): String = call("POST", "chat", JSONObject().put("message", message)).optString("reply")
}
