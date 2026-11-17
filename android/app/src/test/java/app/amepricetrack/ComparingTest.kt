package app.amepricetrack

import app.amepricetrack.data.ApiClient
import app.amepricetrack.data.CollectEntry
import app.amepricetrack.data.expandShortBarcode
import app.amepricetrack.data.looksLikeLink
import app.amepricetrack.data.parseCollect
import app.amepricetrack.data.parseFindAnswer
import app.amepricetrack.data.parseFindStart
import app.amepricetrack.data.parseProductDetail
import app.amepricetrack.ui.matchNotes
import app.amepricetrack.work.PhoneReport
import app.amepricetrack.work.ReportWorker
import kotlinx.coroutines.runBlocking
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Finding products, comparing what they really cost, and reporting prices from the phone. */
class ComparingTest {
    private fun product(id: Int, store: String, cents: Int, extra: String = "") = """
        {"id":$id,"retailer":"$store","retailer_sku":"sku$id","url":"https://www.$store.com/p/$id","title":"Copy Paper",
         "brand":"Hammermill","model":"105007","gtin":"010199000019","image_url":null,"current_price_cents":$cents,
         "list_price_cents":null,"in_stock":true,"lowest_price_cents":$cents,"last_success_at":1794908000,"last_error":null$extra}
    """.trimIndent()

    private fun detail(otherStores: String) = JSONObject(
        """
        {"product":${product(7, "staples", 6000)},
         "watch":{"target_price_cents":null,"drop_percent":null,"notify_any_drop":true,"notify_back_in_stock":false,"note":null},
         "price":{"public_cents":6000,"member_cents":null,"discount_name":null,"discount_percent":null,"your_cents":6000,
                  "shipping_cents":null,"shipping_extra":false,"shipping_membership":null},
         "deal":{"usual_cents":null,"percent_below":null,"compared_to":null,"at_lowest":false},
         "elsewhere":null,"store_name":"Staples","history":[],"other_stores":[$otherStores]}
        """.trimIndent(),
    )

    private fun row(id: Int, store: String, fields: String, productExtra: String = "", cents: Int = 500) = """
        {"comparison":{"id":$id,"status":"found","reason":"Same model number","confirmed":false,"judged_by":"rules"},
         "store_name":"$store","product":${product(id + 100, store.lowercase(), cents, productExtra)},$fields}
    """.trimIndent()

    @Test
    fun anotherPackSizeDeliveryAndSellerAreRead() {
        val ream = row(
            1, "Walmart",
            """"your_cents":500,"shipping_cents":0,"shipping_extra":false,"equivalent_cents":5000,
               "their_amount":"500 sheets","your_amount":"5000 sheets","marketplace":false""",
        )
        val marketplace = row(
            2, "Amazon",
            """"your_cents":5200,"shipping_cents":599,"shipping_extra":false,"equivalent_cents":5799,
               "their_amount":null,"your_amount":null,"marketplace":true""",
            productExtra = ""","seller":"Paper Depot","third_party":true""",
            cents = 5200,
        )
        val parsed = parseProductDetail(detail("$ream,$marketplace"))
        val (walmart, amazon) = parsed.otherStores

        assertEquals(5000L, walmart.equivalentCents)
        assertEquals("500 sheets" to "5000 sheets", walmart.theirAmount to walmart.yourAmount)
        assertEquals("105007", walmart.product?.model)
        assertEquals("010199000019", walmart.product?.barcode)
        // A different amount is not the identical item, so there is nothing to price match.
        assertFalse(walmart.worthMatching(parsed.item.price.totalCents))

        assertTrue(amazon.marketplace)
        assertEquals("Paper Depot", amazon.product?.seller)
        assertEquals(599L, amazon.shippingCents)
        assertTrue("$57.99 with delivery beats $60.00", amazon.worthMatching(parsed.item.price.totalCents))
        assertFalse(amazon.worthMatching(5000))
        assertFalse(amazon.worthMatching(null))
    }

    @Test
    fun anOlderServerWithoutTheNewFieldsStillWorks() {
        val plain = """
            {"comparison":{"id":1,"status":"found","reason":null,"confirmed":false,"judged_by":"rules"},
             "store_name":"Walmart","product":${product(101, "walmart", 5500)},"your_cents":5500}
        """.trimIndent()
        val store = parseProductDetail(detail(plain)).otherStores.single()
        assertEquals(5500L, store.equivalentCents)
        assertNull(store.theirAmount)
        assertFalse(store.marketplace)
    }

    @Test
    fun priceMatchNotesSayWhatMayGetItRefused() {
        val now = 1794908000L + 600
        val clean = parseProductDetail(detail(row(1, "Walmart", """"your_cents":500,"equivalent_cents":500"""))).otherStores.single()
        val notes = matchNotes(clean, "Walmart", now)
        assertEquals(listOf("In stock at Walmart.", "Matched by: Same model number."), notes.map { it.text })
        assertTrue(notes.none { it.caution })

        val risky = parseProductDetail(
            detail(
                row(
                    2, "Amazon",
                    """"your_cents":500,"shipping_cents":599,"equivalent_cents":1099,"marketplace":true""",
                    productExtra = ""","seller":"Paper Depot","third_party":true""",
                ),
            ),
        ).otherStores.single()
        // Read two days ago, from a marketplace seller, with a delivery charge.
        val cautions = matchNotes(risky, "Amazon", now + 2 * 86400).filter { it.caution }.map { it.text }
        assertEquals(3, cautions.size)
        assertTrue(cautions[0].startsWith("Sold by Paper Depot, a marketplace seller."))
        assertTrue(cautions[1].startsWith("Delivery adds $5.99 at Amazon."))
        assertTrue(cautions[2].startsWith("This price was read a while ago."))
    }

    @Test
    fun shortBarcodesAreWrittenOutInFull() {
        assertEquals("042100005264", expandShortBarcode("04252614"))
        assertEquals("012345000058", expandShortBarcode("01234558"))
        assertEquals("012363000064", expandShortBarcode("01236364"))
        assertEquals("012300000413", expandShortBarcode("01234133"))
        assertEquals("012340000053", expandShortBarcode("01234543"))
        // A full barcode, and an eight-digit code of another kind, are left alone.
        assertEquals("195951039305", expandShortBarcode("195951039305"))
        assertEquals("96385074", expandShortBarcode("96385074"))
    }

    @Test
    fun linksAreToldFromThingsToLookUp() {
        assertTrue(looksLikeLink("https://www.staples.com/paper/product_122374"))
        assertTrue(looksLikeLink("Check this out! https://a.co/d/abc123"))
        assertTrue(looksLikeLink("walmart.com/ip/19864722287"))
        assertFalse(looksLikeLink("hammermill copy paper"))
        assertFalse(looksLikeLink("195951039305"))
        assertFalse(looksLikeLink("MXP93LL/A"))
    }

    @Test
    fun findAnswersAreRead() {
        val start = parseFindStart(
            JSONObject(
                """{"query":"195951039305","barcode":true,"stores":[{"id":"staples","name":"Staples"},{"id":"walmart","name":"Walmart"}],
                    "tracked":[{"product_id":4,"title":"AirPods Max","store_name":"Walmart"}]}""",
            ),
        )
        assertTrue(start.barcode)
        assertEquals(listOf("staples", "walmart"), start.stores.map { it.id })
        assertEquals(4L, start.tracked.single().productId)

        val answer = parseFindAnswer(
            JSONObject("""{"store":"walmart","store_name":"Walmart","results":[{"title":"AirPods Max","url":"https://www.walmart.com/ip/1"}],"problem":null}"""),
        )
        assertEquals("AirPods Max", answer.results.single().title)
        assertNull(answer.problem)
        val refused = parseFindAnswer(JSONObject("""{"store":"target","store_name":"Target","results":[],"problem":"the store refused the request"}"""))
        assertEquals("the store refused the request", refused.problem)
    }

    @Test
    fun findRequestsCarryTheWordsSafely() = runBlocking {
        val server = MockWebServer()
        server.start()
        try {
            val api = ApiClient(serverUrl = { server.url("").toString().trimEnd('/') }, token = { "tok" })
            server.enqueue(MockResponse().setBody("""{"query":"8.5 x 11 paper & pens","barcode":false,"stores":[],"tracked":[]}"""))
            api.findStart("8.5 x 11 paper & pens")
            assertEquals("/api/v1/find?q=8.5+x+11+paper+%26+pens", server.takeRequest().path)

            server.enqueue(MockResponse().setBody("""{"store":"boxlunch.com","store_name":"BoxLunch","results":[],"problem":null}"""))
            api.findAt("copy paper", "boxlunch.com")
            assertEquals("/api/v1/find?q=copy+paper&store=boxlunch.com", server.takeRequest().path)

            server.enqueue(MockResponse().setBody("""{"ok":true}"""))
            api.refreshComparison(12)
            val refresh = server.takeRequest()
            assertEquals("POST" to "/api/v1/comparisons/12/refresh", refresh.method to refresh.path)
        } finally {
            server.shutdown()
        }
    }

    private fun entry(id: Long, url: String, needed: Boolean) = CollectEntry(id, "Store", "Thing $id", url, needed)

    @Test
    fun automaticReportsCoverWhatTheServerCannotReadAndStoresSignedInAt() {
        val entries = listOf(
            entry(1, "https://www.amazon.com/dp/B000000001", needed = false),
            entry(2, "https://www.staples.com/chair/product_555", needed = true),
            entry(3, "https://www.walmart.com/ip/777", needed = false),
            entry(4, "https://www.bestbuy.com/site/1.p?skuId=1", needed = true),
        )
        // The ones the server needs come first; Walmart follows because it was opened in the app.
        assertEquals(listOf(2L, 4L, 3L), PhoneReport.automatic(entries, setOf("walmart.com")).map { it.productId })
        assertEquals(listOf(2L, 4L), PhoneReport.automatic(entries, emptySet()).map { it.productId })
        assertEquals("walmart.com", PhoneReport.hostOf("https://www.walmart.com/ip/777"))
        assertEquals("", PhoneReport.hostOf("not a link"))

        val many = (1L..60L).map { entry(it, "https://www.staples.com/x/product_$it", needed = true) }
        assertEquals(PhoneReport.MAX_AUTOMATIC, PhoneReport.automatic(many, emptySet()).size)
    }

    @Test
    fun anAutomaticReportIsSummedUpInWords() {
        assertEquals("Nothing needed reporting.", ReportWorker.summarise(0, 0, 0))
        assertEquals("Reported 1 price.", ReportWorker.summarise(1, 0, 1))
        assertEquals("Reported 5 prices. 1 could not be read.", ReportWorker.summarise(5, 1, 6))
        assertEquals("Reported 3 prices. 2 could not be read. 4 are left for next time.", ReportWorker.summarise(3, 2, 9))
    }

    @Test
    fun collectEntriesSayWhenTheyWereLastRead() {
        val entries = parseCollect(
            JSONObject(
                """{"needed":[{"product_id":1,"store_name":"Staples","title":"Chair","url":"https://www.staples.com/c/product_1","last_success_at":null}],
                    "optional":[{"product_id":2,"store_name":"Walmart","title":"Desk","url":"https://www.walmart.com/ip/2","last_success_at":1794908000}]}""",
            ),
        )
        assertNull(entries[0].lastSuccessAt)
        assertEquals(1794908000L, entries[1].lastSuccessAt)
    }
}
