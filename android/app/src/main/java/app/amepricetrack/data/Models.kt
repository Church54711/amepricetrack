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
) {
    val displayTitle: String get() = title ?: url
}

data class WatchSettings(
    val targetPriceCents: Long?,
    val dropPercent: Double?,
    val notifyAnyDrop: Boolean,
    val notifyBackInStock: Boolean,
    val note: String?,
)

data class Price(
    /** The price anyone sees. */
    val publicCents: Long?,
    /** The price this phone reported while signed in at the store. */
    val memberCents: Long?,
    val discountName: String?,
    val discountPercent: Double?,
    /** What this person would actually pay. */
    val yourCents: Long?,
) {
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
    val yourCents: Long?,
) {
    /** A match the AI made that the person has not confirmed. */
    val aiJudged: Boolean get() = judgedBy == "ai" && !confirmed
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
)

/** A product page the phone can load and report back. */
data class CollectEntry(
    val productId: Long,
    val storeName: String,
    val title: String,
    val url: String,
    /** True when the server cannot read this product itself. */
    val needed: Boolean,
)

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
        ),
        price = Price(
            publicCents = price.long("public_cents"),
            memberCents = price.long("member_cents"),
            discountName = price.text("discount_name"),
            discountPercent = price.double("discount_percent"),
            yourCents = price.long("your_cents"),
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
)

fun parseCollect(o: JSONObject): List<CollectEntry> {
    fun entries(key: String, needed: Boolean) = o.optJSONArray(key).mapObjects {
        CollectEntry(it.optLong("product_id"), it.optString("store_name"), it.text("title") ?: it.optString("url"), it.optString("url"), needed)
    }
    return entries("needed", true) + entries("optional", false)
}

// ------------------------------------------------------------------ small helpers

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
