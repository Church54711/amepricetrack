package app.amepricetrack.data

import org.json.JSONArray
import org.json.JSONObject
import java.text.NumberFormat
import java.util.Locale

// What the server sends, in the shapes the screens use. Field names follow
// the server's JSON (see the "Phone API" section of the README).

data class Product(
    val id: Long,
    val retailer: String,
    val url: String,
    val title: String?,
    val brand: String?,
    val imageUrl: String?,
    val currentPriceCents: Long?,
    val listPriceCents: Long?,
    val inStock: Boolean?,
    val lowestPriceCents: Long?,
    val lastSuccessAt: Long?,
    val lastError: String?,
    val model: String? = null,
    val barcode: String? = null,
    /** The store's own item number. */
    val itemNumber: String? = null,
    /** Who sells it, at stores that host other sellers. */
    val seller: String? = null,
    /** True when that seller is not the store itself. */
    val thirdParty: Boolean? = null,
) {
    val displayTitle: String get() = title ?: url
}

data class WatchSettings(
    val targetPriceCents: Long?,
    val dropPercent: Double?,
    val notifyAnyDrop: Boolean,
    val notifyBackInStock: Boolean,
    val note: String?,
    /** Check this product every so many minutes; null for the server's usual pace. */
    val checkMinutes: Int? = null,
)

data class Price(
    /** The price anyone sees. */
    val publicCents: Long?,
    /** The price this phone reported while signed in at the store. */
    val memberCents: Long?,
    val discountName: String?,
    val discountPercent: Double?,
    /** What this person would actually pay for the item itself. */
    val yourCents: Long?,
    /** What delivery adds: 0 when free or covered by a membership, null when the store does not say. */
    val shippingCents: Long? = null,
    /** Delivery is charged, but the store does not say how much. */
    val shippingExtra: Boolean = false,
    /** The membership that makes delivery free, when one applies. */
    val shippingMembership: String? = null,
) {
    /** The item and its delivery together. */
    val totalCents: Long? get() = yourCents?.let { it + (shippingCents ?: 0) }

    val isPersonal: Boolean
        get() = (yourCents != null && publicCents != null && yourCents < publicCents) || (publicCents == null && memberCents != null)
}

data class Deal(
    val usualCents: Long?,
    val percentBelow: Double?,
    /** "average" or "list": what percentBelow is measured against. */
    val comparedTo: String?,
    val atLowest: Boolean,
)

/** The cheapest confirmed match at another store. */
data class Elsewhere(val storeName: String, val productId: Long, val url: String, val cents: Long)

data class WatchItem(
    val product: Product,
    val watch: WatchSettings,
    val price: Price,
    val deal: Deal,
    val elsewhere: Elsewhere?,
    val storeName: String,
)

data class PricePoint(val ts: Long, val priceCents: Long, val member: Boolean)

/** One other store's answer for a tracked product. */
data class OtherStore(
    val comparisonId: Long,
    val storeName: String,
    /** pending, found, possible, none, failed or rejected. */
    val status: String,
    val reason: String?,
    val confirmed: Boolean,
    /** "rules" or "ai". */
    val judgedBy: String,
    val product: Product?,
    /** What the listing costs this person there, before delivery. */
    val yourCents: Long?,
    val shippingCents: Long? = null,
    val shippingExtra: Boolean = false,
    /** The figure to compare: listing plus delivery, for the same amount as the tracked product. */
    val equivalentCents: Long? = null,
    /** "500 sheets": how much the other listing holds, when that differs. */
    val theirAmount: String? = null,
    /** "5000 sheets": how much the tracked product holds, when that differs. */
    val yourAmount: String? = null,
    /** The offer comes from a marketplace seller, not the store itself. */
    val marketplace: Boolean = false,
) {
    /** A match the AI made that the person has not confirmed. */
    val aiJudged: Boolean get() = judgedBy == "ai" && !confirmed

    /** True when this store can be shown to the tracked store as a lower price for the identical item. */
    fun worthMatching(trackedTotalCents: Long?): Boolean =
        status == "found" && theirAmount == null && product != null &&
            equivalentCents != null && trackedTotalCents != null && equivalentCents < trackedTotalCents
}

data class ProductDetail(val item: WatchItem, val history: List<PricePoint>, val otherStores: List<OtherStore>)

data class Alert(
    val id: Long,
    val productId: Long?,
    val ts: Long,
    val kind: String,
    val title: String,
    val body: String,
    val read: Boolean,
)

data class AlertList(val alerts: List<Alert>, val unread: Int)

data class Me(
    val username: String,
    val theme: String,
    val accent: String?,
    val isAdmin: Boolean,
    val unreadAlerts: Int,
    val serverVersion: String,
    /** How often the server checks a product nobody asked to have watched closely. */
    val checkIntervalMinutes: Int = 240,
)

/** Where the server pushes this person's alerts. */
data class PushSettings(
    /** The ntfy topic address, or "" when alerts are not pushed through ntfy. */
    val ntfyUrl: String,
    /** True when a webhook is set up on the website as well. */
    val webhook: Boolean,
)

/** A product page the phone can load and report back. */
data class CollectEntry(
    val productId: Long,
    val storeName: String,
    val title: String,
    val url: String,
    /** True when the server cannot read this product itself. */
    val needed: Boolean,
    /** When any reader last got this product's price, in seconds. */
    val lastSuccessAt: Long? = null,
)

/** A store that can be searched, as the server names it. */
data class StoreRef(val id: String, val name: String)

/** A product already on the watchlist. */
data class TrackedRef(val productId: Long, val title: String, val storeName: String)

/** The start of a find: the words tidied by the server and where to look. */
data class FindStart(val query: String, val barcode: Boolean, val stores: List<StoreRef>, val tracked: List<TrackedRef>)

data class FindItem(val title: String, val url: String)

/** One store's search results. */
data class FindAnswer(val storeName: String, val results: List<FindItem>, val problem: String?)

data class IngestResult(val priceCents: Long?, val recordedAs: String)

// ------------------------------------------------------------------ reading JSON

private fun JSONObject.text(key: String): String? = if (isNull(key)) null else optString(key).takeIf { it.isNotEmpty() }
private fun JSONObject.long(key: String): Long? = if (isNull(key) || !has(key)) null else optLong(key)
private fun JSONObject.double(key: String): Double? = if (isNull(key) || !has(key)) null else optDouble(key).takeIf { !it.isNaN() }
private fun JSONObject.bool(key: String): Boolean? = if (isNull(key) || !has(key)) null else optBoolean(key)
private fun JSONObject.child(key: String): JSONObject? = if (isNull(key)) null else optJSONObject(key)

private fun <T> JSONArray?.mapObjects(read: (JSONObject) -> T): List<T> {
    if (this == null) return emptyList()
    return (0 until length()).mapNotNull { optJSONObject(it) }.map(read)
}

fun parseProduct(o: JSONObject) = Product(
    id = o.getLong("id"),
    retailer = o.optString("retailer"),
    url = o.optString("url"),
    title = o.text("title"),
    brand = o.text("brand"),
    imageUrl = o.text("image_url"),
    currentPriceCents = o.long("current_price_cents"),
    listPriceCents = o.long("list_price_cents"),
    inStock = o.bool("in_stock"),
    lowestPriceCents = o.long("lowest_price_cents"),
    lastSuccessAt = o.long("last_success_at"),
    lastError = o.text("last_error"),
    model = o.text("model"),
    barcode = o.text("gtin"),
    itemNumber = o.text("retailer_sku"),
    seller = o.text("seller"),
    thirdParty = o.bool("third_party"),
)

fun parseWatchItem(o: JSONObject): WatchItem {
    val watch = o.getJSONObject("watch")
    val price = o.getJSONObject("price")
    val deal = o.getJSONObject("deal")
    val product = parseProduct(o.getJSONObject("product"))
    return WatchItem(
        product = product,
        watch = WatchSettings(
            targetPriceCents = watch.long("target_price_cents"),
            dropPercent = watch.double("drop_percent"),
            notifyAnyDrop = watch.optBoolean("notify_any_drop", true),
            notifyBackInStock = watch.optBoolean("notify_back_in_stock", false),
            note = watch.text("note"),
            checkMinutes = watch.long("check_minutes")?.toInt(),
        ),
        price = Price(
            publicCents = price.long("public_cents"),
            memberCents = price.long("member_cents"),
            discountName = price.text("discount_name"),
            discountPercent = price.double("discount_percent"),
            yourCents = price.long("your_cents"),
            shippingCents = price.long("shipping_cents"),
            shippingExtra = price.optBoolean("shipping_extra", false),
            shippingMembership = price.text("shipping_membership"),
        ),
        deal = Deal(
            usualCents = deal.long("usual_cents"),
            percentBelow = deal.double("percent_below"),
            comparedTo = deal.text("compared_to"),
            atLowest = deal.optBoolean("at_lowest", false),
        ),
        elsewhere = o.child("elsewhere")?.let {
            Elsewhere(it.optString("store_name"), it.optLong("product_id"), it.optString("url"), it.optLong("cents"))
        },
        storeName = o.text("store_name") ?: product.retailer,
    )
}

fun parseWatchList(o: JSONObject): List<WatchItem> = o.optJSONArray("watches").mapObjects(::parseWatchItem)

fun parseProductDetail(o: JSONObject) = ProductDetail(
    item = parseWatchItem(o),
    history = o.optJSONArray("history").mapObjects {
        PricePoint(it.optLong("ts"), it.optLong("price_cents"), it.optBoolean("member", false))
    },
    otherStores = o.optJSONArray("other_stores").mapObjects { row ->
        val comparison = row.getJSONObject("comparison")
        OtherStore(
            comparisonId = comparison.getLong("id"),
            storeName = row.optString("store_name"),
            status = comparison.optString("status"),
            reason = comparison.text("reason"),
            confirmed = comparison.optBoolean("confirmed", false),
            judgedBy = comparison.optString("judged_by", "rules"),
            product = row.child("product")?.let(::parseProduct),
            yourCents = row.long("your_cents"),
            shippingCents = row.long("shipping_cents"),
            shippingExtra = row.optBoolean("shipping_extra", false),
            // Older servers do not send this; the listing's own price is the best stand-in.
            equivalentCents = row.long("equivalent_cents") ?: row.long("your_cents"),
            theirAmount = row.text("their_amount"),
            yourAmount = row.text("your_amount"),
            marketplace = row.optBoolean("marketplace", false),
        )
    },
)

fun parseAlerts(o: JSONObject) = AlertList(
    alerts = o.optJSONArray("notifications").mapObjects {
        Alert(
            id = it.getLong("id"),
            productId = it.long("product_id"),
            ts = it.optLong("ts"),
            kind = it.optString("kind"),
            title = it.optString("title"),
            body = it.optString("body"),
            read = !it.isNull("read_at"),
        )
    },
    unread = o.optInt("unread", 0),
)

fun parseMe(o: JSONObject) = Me(
    username = o.optString("username"),
    theme = o.optString("theme", "oled-gold"),
    accent = o.text("accent"),
    isAdmin = o.optBoolean("is_admin", false),
    unreadAlerts = o.optInt("unread_alerts", 0),
    serverVersion = o.optString("server_version"),
    checkIntervalMinutes = o.optInt("check_interval_minutes", 240),
)

fun parsePushSettings(o: JSONObject) = PushSettings(ntfyUrl = o.optString("ntfy_url"), webhook = o.optBoolean("webhook", false))

fun parseCollect(o: JSONObject): List<CollectEntry> {
    fun entries(key: String, needed: Boolean) = o.optJSONArray(key).mapObjects {
        CollectEntry(
            productId = it.optLong("product_id"),
            storeName = it.optString("store_name"),
            title = it.text("title") ?: it.optString("url"),
            url = it.optString("url"),
            needed = needed,
            lastSuccessAt = it.long("last_success_at"),
        )
    }
    return entries("needed", true) + entries("optional", false)
}

fun parseFindStart(o: JSONObject) = FindStart(
    query = o.optString("query"),
    barcode = o.optBoolean("barcode", false),
    stores = o.optJSONArray("stores").mapObjects { StoreRef(it.optString("id"), it.optString("name")) },
    tracked = o.optJSONArray("tracked").mapObjects {
        TrackedRef(it.optLong("product_id"), it.optString("title"), it.optString("store_name"))
    },
)

fun parseFindAnswer(o: JSONObject) = FindAnswer(
    storeName = o.optString("store_name"),
    results = o.optJSONArray("results").mapObjects { FindItem(it.optString("title"), it.optString("url")) },
    problem = o.text("problem"),
)

// ------------------------------------------------------------------ small helpers

/** How long between checks, in words that follow "every": 30 -> "30 minutes", 60 -> "hour", 240 -> "4 hours". */
fun paceText(minutes: Int): String = when {
    minutes == 60 -> "hour"
    minutes >= 120 && minutes % 60 == 0 -> "${minutes / 60} hours"
    else -> "$minutes minutes"
}

/**
 * The closer paces a person can choose for one product, in minutes: the ones
 * shorter than the server's usual pace, and whatever is saved already.
 */
fun paceChoices(usualMinutes: Int, saved: Int?): List<Int> =
    (listOf(30, 60, 120).filter { it < usualMinutes } + listOfNotNull(saved)).distinct().sorted()

/** A new ntfy topic address on the public ntfy server, with a name nobody could guess. */
fun newNtfyAddress(random: java.util.Random = java.security.SecureRandom()): String {
    val letters = "abcdefghijklmnopqrstuvwxyz0123456789"
    val name = (1..24).map { letters[random.nextInt(letters.length)] }.joinToString("")
    return "https://ntfy.sh/apt-$name"
}

/**
 * The link that opens a topic in the ntfy app and subscribes to it:
 * "https://ntfy.sh/my-topic" -> "ntfy://ntfy.sh/my-topic". A server reached
 * without HTTPS is marked as such. Null when the address is not a topic address.
 */
fun ntfyAppLink(address: String): String? {
    val uri = try {
        java.net.URI(address.trim())
    } catch (e: Exception) {
        return null
    }
    val scheme = uri.scheme?.lowercase()
    val server = uri.rawAuthority
    val topic = uri.rawPath.orEmpty().trim('/')
    if (scheme !in setOf("http", "https") || server.isNullOrEmpty() || topic.isEmpty() || topic.contains('/')) return null
    return "ntfy://$server/$topic" + if (scheme == "http") "?secure=false" else ""
}

/** The product a link such as "amepricetrack://products/12" points at, when it is one. */
fun productIdFromAppLink(link: String?): Long? =
    link?.let { Regex("""^amepricetrack://products/(\d{1,18})/?$""").matchEntire(it.trim()) }?.groupValues?.get(1)?.toLongOrNull()

/**
 * A browser hands back the result of a script as JSON text: a page's HTML
 * arrives as one quoted string with its special characters escaped. This
 * returns the plain text, or "" when the script produced nothing.
 */
fun decodeJavascriptString(value: String?): String {
    if (value == null || value == "null") return ""
    return try {
        JSONArray("[$value]").optString(0)
    } catch (e: Exception) {
        ""
    }
}

/**
 * Small packages carry a short barcode (UPC-E). Stores list the full
 * twelve-digit one, so a scanned short code is written out in full:
 * "04252614" -> "042100005264". Anything else is returned as it came.
 */
fun expandShortBarcode(code: String): String {
    if (code.length != 8 || !code.all { it.isDigit() } || code[0] !in "01") return code
    val system = code[0]
    val d = code.substring(1, 7)
    val check = code[7]
    val body = when (d[5]) {
        '0', '1', '2' -> d.substring(0, 2) + d[5] + "0000" + d.substring(2, 5)
        '3' -> d.substring(0, 3) + "00000" + d.substring(3, 5)
        '4' -> d.substring(0, 4) + "00000" + d[4]
        else -> d.substring(0, 5) + "0000" + d[5]
    }
    return "$system$body$check"
}

/** True when the text holds a web link, as opposed to a name or barcode to look up. */
fun looksLikeLink(text: String): Boolean =
    text.length < 2000 && (Regex("""https?://\S+""").containsMatchIn(text) || Regex("""^[A-Za-z0-9][A-Za-z0-9.-]*\.[A-Za-z]{2,}(/\S*)?$""").matches(text.trim()))

/** 123456 -> "$1,234.56" */
fun dollars(cents: Long): String = NumberFormat.getCurrencyInstance(Locale.US).format(cents / 100.0)

/** "12.50", "$12.5" or "1,299" -> cents, or null when it is not an amount. */
fun parseDollars(text: String): Long? {
    val cleaned = text.trim().removePrefix("$").replace(",", "").trim()
    if (cleaned.isEmpty() || !Regex("""\d+(\.\d{0,2})?""").matches(cleaned)) return null
    return Math.round(cleaned.toDouble() * 100).takeIf { it > 0 }
}

/**
 * Turns what a person types as the server's address into a full address:
 * "192.168.1.20:8787" -> "http://192.168.1.20:8787". Returns null when it
 * cannot be an address.
 */
fun normaliseServerAddress(typed: String): String? {
    val text = typed.trim()
    if (text.isEmpty() || text.any { it.isWhitespace() }) return null
    val scheme = when {
        text.startsWith("https://", ignoreCase = true) -> "https://"
        text.startsWith("http://", ignoreCase = true) -> "http://"
        else -> ""
    }
    val rest = text.substring(scheme.length).trimEnd('/')
    val host = rest.substringBefore('/').substringBefore(':')
    if (host.isEmpty() || !host.all { it.isLetterOrDigit() || it == '.' || it == '-' }) return null
    return scheme.ifEmpty { "http://" }.lowercase() + rest
}
