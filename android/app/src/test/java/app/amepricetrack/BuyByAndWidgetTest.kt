package app.amepricetrack

import app.amepricetrack.data.PricePoint
import app.amepricetrack.data.buyByFromPicker
import app.amepricetrack.data.buyByText
import app.amepricetrack.data.chartPoints
import app.amepricetrack.data.dayText
import app.amepricetrack.data.daysUntil
import app.amepricetrack.data.dealRows
import app.amepricetrack.data.dealRowsFromJson
import app.amepricetrack.data.dealRowsToJson
import app.amepricetrack.data.parseWatchItem
import app.amepricetrack.data.pickerMillis
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class BuyByAndWidgetTest {
    // 2026-11-28 at noon UTC, which is how the server keeps a buy-by day.
    private val day = 1_795_867_200L

    @Test
    fun aBuyByDayIsOneCalendarDayHoweverItIsWritten() {
        assertEquals("2026-11-28", dayText(day))
        // The date picker hands over the start of the day in UTC; the server gets the same day back.
        assertEquals(day, buyByFromPicker(pickerMillis(day)))
        assertEquals(1_795_824_000_000L, pickerMillis(day))
    }

    @Test
    fun daysAreCountedWholeAndTheLabelSaysHowClose() {
        assertEquals(0, daysUntil(day, day))
        assertEquals(0, daysUntil(day, day - 11 * 3600))
        assertEquals(1, daysUntil(day, day - 86_400))
        assertEquals(-1, daysUntil(day, day + 86_400))
        assertEquals("Buy by Nov 28 (today)", buyByText(day, day))
        assertEquals("Buy by Nov 28 (tomorrow)", buyByText(day, day - 86_400))
        assertEquals("Buy by Nov 28 (5 days)", buyByText(day, day - 5 * 86_400))
        assertEquals("Buy by Nov 28", buyByText(day, day - 30 * 86_400))
        assertEquals("Buy by Nov 28 (passed)", buyByText(day, day + 3 * 86_400))
    }

    @Test
    fun aChartRangeStartsFromThePriceThatAlreadyHeld() {
        val history = listOf(
            PricePoint(100, 5000, false),
            PricePoint(200, 4500, false),
            PricePoint(250, 4000, true),
            PricePoint(400, 4800, false),
        )
        assertEquals(listOf(100L, 200L, 400L), chartPoints(history, null).map { it.ts })
        val recent = chartPoints(history, 300)
        assertEquals(listOf(300L to 4500L, 400L to 4800L), recent.map { it.ts to it.priceCents })
        // Nothing changed inside the range: the price that held is the whole chart.
        assertEquals(listOf(500L to 4800L), chartPoints(history, 500).map { it.ts to it.priceCents })
        assertTrue(chartPoints(emptyList(), 300).isEmpty())
    }

    private fun item(id: Long, yours: Long?, target: Long? = null, below: Double? = null, lowest: Boolean = false, inStock: Boolean = true) =
        parseWatchItem(
            JSONObject(
                """{"product":{"id":$id,"retailer":"staples","url":"https://www.staples.com/product_$id","title":"Item $id","in_stock":$inStock},
                "watch":{"target_price_cents":${target ?: "null"},"notify_any_drop":true},
                "price":{"your_cents":${yours ?: "null"}},
                "deal":{"percent_below":${below ?: "null"},"compared_to":"average","at_lowest":$lowest},
                "store_name":"Staples"}""",
            ),
        )

    @Test
    fun theWidgetPutsTheBestPricesFirst() {
        val rows = dealRows(
            listOf(
                item(1, 5000),
                item(2, 4000, below = 12.4),
                item(3, 3000, lowest = true),
                item(4, 2000, target = 2500),
                item(5, null),
                item(6, 1000, below = 30.0),
                item(7, 900, below = 50.0, inStock = false),
            ),
        )
        assertEquals(listOf(4L, 3L, 6L, 2L, 1L, 7L), rows.map { it.productId }, )
        assertEquals("Target reached", rows[0].note)
        assertEquals("Lowest recorded", rows[1].note)
        assertEquals("30% below usual", rows[2].note)
        assertEquals("$20.00", rows[0].price)
        assertTrue(rows[0].good)
        assertNull(rows[4].note)
        assertEquals("Out of stock", rows[5].note)
    }

    @Test
    fun widgetRowsSurviveBeingStored() {
        val rows = dealRows(listOf(item(4, 2000, target = 2500), item(1, 5000)))
        assertEquals(rows, dealRowsFromJson(dealRowsToJson(rows)))
        assertTrue(dealRowsFromJson(null).isEmpty())
        assertTrue(dealRowsFromJson("not json").isEmpty())
    }
}
