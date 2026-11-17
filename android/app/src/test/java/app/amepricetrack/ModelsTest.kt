package app.amepricetrack

import app.amepricetrack.data.Alert
import app.amepricetrack.data.decodeJavascriptString
import app.amepricetrack.data.dollars
import app.amepricetrack.data.normaliseServerAddress
import app.amepricetrack.data.parseAlerts
import app.amepricetrack.data.parseCollect
import app.amepricetrack.data.parseDollars
import app.amepricetrack.data.parseMe
import app.amepricetrack.data.parseProductDetail
import app.amepricetrack.data.parseWatchList
import app.amepricetrack.work.AlertWorker
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The server's answers, as it sends them, read into what the screens show. */
class ModelsTest {
    private val watch = """
        {"product":{"id":7,"retailer":"staples","retailer_sku":"555","url":"https://www.staples.com/chair/product_555",
                    "title":"Office Chair","brand":"TRU RED","model":null,"gtin":null,"image_url":"https://img.example/chair.jpg",
                    "current_price_cents":10000,"list_price_cents":12999,"in_stock":true,"promo_text":null,
                    "lowest_price_cents":9500,"lowest_price_at":1794900000,"fetch_mode":"browser","created_at":1794800000,
                    "last_checked_at":1794908000,"last_success_at":1794908000,"next_check_at":1794922400,"fail_count":0,
                    "last_error":null,"details":"Colour: Black"},
         "watch":{"id":3,"user_id":1,"product_id":7,"target_price_cents":8000,"drop_percent":10.0,"notify_any_drop":true,
                  "notify_back_in_stock":false,"note":"for the office","created_at":1794800000,"last_seen_price_cents":7500,
                  "last_seen_in_stock":true,"last_alert_price_cents":null,"last_alert_at":null},
         "price":{"public_cents":10000,"member_cents":null,"discount_name":"Associate discount","discount_percent":25.0,"your_cents":7500},
         "deal":{"usual_cents":11000,"percent_below":31.8,"compared_to":"average","at_lowest":false},
         "elsewhere":{"store_name":"Walmart","product_id":9,"url":"https://www.walmart.com/ip/1","cents":7000},
         "store_name":"Staples"}
    """.trimIndent()

    @Test
    fun watchlistIsRead() {
        val items = parseWatchList(JSONObject("""{"watches":[$watch]}"""))
        val item = items.single()
        assertEquals(7L, item.product.id)
        assertEquals("Office Chair", item.product.displayTitle)
        assertEquals("Staples", item.storeName)
        assertEquals(7500L, item.price.yourCents)
        assertTrue("a discount makes it a personal price", item.price.isPersonal)
        assertEquals(8000L, item.watch.targetPriceCents)
        assertEquals(10.0, item.watch.dropPercent!!, 0.0)
        assertEquals("for the office", item.watch.note)
        assertEquals("average", item.deal.comparedTo)
        assertEquals("Walmart", item.elsewhere!!.storeName)
        assertEquals(7000L, item.elsewhere!!.cents)
    }

    @Test
    fun missingAndNullFieldsAreAbsentNotZero() {
        val bare = """
            {"product":{"id":1,"retailer":"shop.example","url":"https://shop.example/p/1","title":null,"current_price_cents":null,
                        "in_stock":null,"last_success_at":null,"last_error":null},
             "watch":{"target_price_cents":null,"drop_percent":null},
             "price":{"public_cents":null,"member_cents":null,"your_cents":null},
             "deal":{"usual_cents":null,"percent_below":null,"compared_to":null,"at_lowest":false},
             "elsewhere":null}
        """.trimIndent()
        val item = parseWatchList(JSONObject("""{"watches":[$bare]}""")).single()
        assertEquals("https://shop.example/p/1", item.product.displayTitle)
        assertNull(item.product.currentPriceCents)
        assertNull(item.product.inStock)
        assertNull(item.price.yourCents)
        assertNull(item.watch.targetPriceCents)
        assertNull(item.elsewhere)
        assertTrue("alerts on any drop unless turned off", item.watch.notifyAnyDrop)
        assertEquals("the site name stands in for a store name", "shop.example", item.storeName)
    }

    @Test
    fun productPageWithHistoryAndOtherStores() {
        val json = JSONObject(watch)
        json.put("history", org.json.JSONArray("""[{"ts":1794800000,"price_cents":12000,"list_price_cents":null,"in_stock":true,"source":"server","member":false},
                                                  {"ts":1794900000,"price_cents":10000,"list_price_cents":null,"in_stock":true,"source":"device","member":true}]"""))
        json.put("other_stores", org.json.JSONArray("""[
            {"comparison":{"id":11,"user_id":1,"source_product_id":7,"retailer":"walmart","status":"found","product_id":9,"score":90,
                           "reason":"Same model and colour","confirmed":false,"last_notified_cents":null,"created_at":1,"updated_at":2,"judged_by":"ai"},
             "store_name":"Walmart","product":{"id":9,"retailer":"walmart","url":"https://www.walmart.com/ip/1","title":"Chair","in_stock":true},
             "your_cents":7000},
            {"comparison":{"id":12,"retailer":"target","status":"none","product_id":null,"reason":null,"confirmed":false,"judged_by":"rules"},
             "store_name":"Target","product":null,"your_cents":null}]"""))
        val detail = parseProductDetail(json)
        assertEquals(2, detail.history.size)
        assertTrue(detail.history[1].member)
        val (walmart, target) = detail.otherStores
        assertEquals("found", walmart.status)
        assertTrue("judged by the AI and not yet confirmed", walmart.aiJudged)
        assertEquals("Chair", walmart.product!!.title)
        assertEquals(7000L, walmart.yourCents)
        assertEquals("none", target.status)
        assertNull(target.product)
        assertFalse(target.aiJudged)
    }

    @Test
    fun alertsAccountAndPagesToCollect() {
        val alerts = parseAlerts(JSONObject("""{"notifications":[
            {"id":5,"user_id":1,"product_id":7,"ts":1794900000,"kind":"drop","title":"Chair is $75.00","body":"Down from $100.00","price_cents":7500,"read_at":null},
            {"id":4,"user_id":1,"product_id":null,"ts":1794800000,"kind":"test","title":"Test","body":"It works","price_cents":null,"read_at":1794800100}],"unread":1}"""))
        assertEquals(1, alerts.unread)
        assertFalse(alerts.alerts[0].read)
        assertTrue(alerts.alerts[1].read)
        assertNull(alerts.alerts[1].productId)

        val me = parseMe(JSONObject("""{"username":"sam","theme":"midnight","accent":"#ff8800","is_admin":true,"unread_alerts":2,"server_version":"0.1.0"}"""))
        assertEquals("midnight", me.theme)
        assertEquals("#ff8800", me.accent)
        assertNull(parseMe(JSONObject("""{"username":"sam","accent":null}""")).accent)
        assertEquals("the default look", "oled-gold", parseMe(JSONObject("""{"username":"sam"}""")).theme)

        val pages = parseCollect(JSONObject("""{"needed":[{"product_id":7,"store":"staples","store_name":"Staples","url":"https://www.staples.com/x/product_555","title":"Office Chair"}],
            "optional":[{"product_id":9,"store":"walmart","store_name":"Walmart","url":"https://www.walmart.com/ip/1"}]}"""))
        assertEquals(listOf(true, false), pages.map { it.needed })
        assertEquals("Office Chair", pages[0].title)
        assertEquals("an older server sends no name; the link is shown", "https://www.walmart.com/ip/1", pages[1].title)
    }

    @Test
    fun onlyNewUnreadAlertsBecomeNotifications() {
        fun alert(id: Long, read: Boolean) = Alert(id, null, 0, "drop", "t", "b", read)
        val fresh = listOf(alert(9, false), alert(8, true), alert(7, false), alert(5, false))
        assertEquals(listOf(7L, 9L), AlertWorker.newAlerts(fresh, lastShown = 5).map { it.id })
        assertTrue(AlertWorker.newAlerts(fresh, lastShown = 9).isEmpty())
    }

    @Test
    fun moneyIsShownAndRead() {
        assertEquals("$1,234.56", dollars(123456))
        assertEquals("$0.99", dollars(99))
        assertEquals(4999L, parseDollars("49.99"))
        assertEquals(129900L, parseDollars(" $1,299 "))
        assertEquals(1250L, parseDollars("12.5"))
        for (bad in listOf("", "abc", "12.345", "-5", "0", "1e3")) assertNull(bad, parseDollars(bad))
    }

    @Test
    fun serverAddressAsTyped() {
        assertEquals("http://192.168.1.20:8787", normaliseServerAddress(" 192.168.1.20:8787/ "))
        assertEquals("https://prices.example.com", normaliseServerAddress("https://prices.example.com/"))
        assertEquals("http://nas.local:8787", normaliseServerAddress("nas.local:8787"))
        for (bad in listOf("", "   ", "my server", "http://", "exa mple.com")) assertNull(bad, normaliseServerAddress(bad))
    }

    @Test
    fun pageHtmlComesBackFromTheBrowserAsQuotedText() {
        assertEquals("<html><p class=\"a\">Tom's é</p>\n</html>", decodeJavascriptString("\"\\u003Chtml>\\u003Cp class=\\\"a\\\">Tom's é\\u003C/p>\\n\\u003C/html>\""))
        assertEquals("", decodeJavascriptString("null"))
        assertEquals("", decodeJavascriptString(null))
    }
}
