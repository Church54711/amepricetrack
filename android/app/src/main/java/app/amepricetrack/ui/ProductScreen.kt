@file:OptIn(ExperimentalMaterial3Api::class)

package app.amepricetrack.ui

import android.content.Intent
import android.net.Uri
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Checkbox
import androidx.compose.material3.DatePicker
import androidx.compose.material3.DatePickerDialog
import androidx.compose.material3.FilterChip
import androidx.compose.material3.SelectableDates
import androidx.compose.material3.rememberDatePickerState
import androidx.compose.material3.RadioButton
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.unit.dp
import app.amepricetrack.App
import app.amepricetrack.data.OtherStore
import app.amepricetrack.data.buyByFromPicker
import app.amepricetrack.data.buyByText
import app.amepricetrack.data.chartPoints
import app.amepricetrack.data.chartRanges
import app.amepricetrack.data.pickerMillis
import app.amepricetrack.data.PricePoint
import app.amepricetrack.data.ProductDetail
import app.amepricetrack.data.WatchSettings
import app.amepricetrack.data.dollars
import app.amepricetrack.data.paceChoices
import app.amepricetrack.data.paceText
import app.amepricetrack.data.parseDollars
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

@Composable
fun ProductScreen(app: App, productId: Long, onBack: () -> Unit, onRemoved: () -> Unit, onPriceMatch: (Long) -> Unit) {
    var state by remember(productId) { mutableStateOf<Load<ProductDetail>>(Load.Loading) }
    var confirmRemove by remember { mutableStateOf(false) }
    val snackbar = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()
    val context = LocalContext.current

    suspend fun load() {
        val result = app.attempt { app.api.product(productId) }
        result.onSuccess { state = Load.Ready(it) }
        result.onFailure { if (state is Load.Ready) snackbar.showSnackbar(result.problem()) else state = Load.Failed(result.problem()) }
    }

    /** Runs an action on the server, says what happened, and shows the result. */
    fun act(done: String, action: suspend () -> Unit) {
        scope.launch {
            val result = app.attempt { action() }
            if (result.isSuccess) load()
            snackbar.showSnackbar(if (result.isSuccess) done else result.problem())
        }
    }

    LaunchedEffect(productId) { load() }

    // While the server is still searching other stores or reading the first price, look again by itself.
    val waiting = (state as? Load.Ready)?.value?.let { d ->
        d.otherStores.any { it.status == "pending" } || (d.item.product.lastSuccessAt == null && d.item.product.lastError == null)
    } == true
    LaunchedEffect(waiting, productId) {
        while (waiting) {
            delay(6000)
            load()
        }
    }

    Scaffold(
        snackbarHost = { SnackbarHost(snackbar) },
        topBar = {
            TopAppBar(
                title = { Text((state as? Load.Ready)?.value?.item?.storeName ?: "Product") },
                navigationIcon = {
                    IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back") }
                },
                actions = {
                    IconButton(onClick = { act("Checking the price now.") { app.api.checkNow(productId) } }) {
                        Icon(Icons.Filled.Refresh, contentDescription = "Check the price now")
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = MaterialTheme.colorScheme.background),
            )
        },
    ) { padding ->
        when (val current = state) {
            Load.Loading -> LoadingBox()
            is Load.Failed -> ProblemBox(current.message) {
                state = Load.Loading
                scope.launch { load() }
            }
            is Load.Ready -> {
                val detail = current.value
                val item = detail.item
                Column(
                    Modifier.padding(padding).fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp),
                    verticalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    Card {
                        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                            Thumbnail(item.product.imageUrl, 88)
                            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                                Text(item.product.displayTitle, fontWeight = FontWeight.SemiBold)
                                item.product.brand?.let { Muted(it) }
                            }
                        }
                        val yours = item.price.yourCents
                        if (yours != null) {
                            Row(verticalAlignment = Alignment.Bottom, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                                Text(dollars(yours), style = MaterialTheme.typography.headlineMedium, fontWeight = FontWeight.Bold)
                                val listed = item.price.publicCents
                                if (listed != null && listed > yours) {
                                    Text(dollars(listed), color = LocalPalette.current.muted, textDecoration = TextDecoration.LineThrough)
                                }
                            }
                        } else {
                            Muted(item.product.lastError?.let { "Could not read the price: $it" } ?: "The first price check is running.")
                        }
                        val delivery = item.price.shippingCents
                        when {
                            delivery != null && delivery > 0 -> Muted("Plus ${dollars(delivery)} delivery.")
                            item.price.shippingExtra -> Muted("Delivery is charged on top; the store does not say how much.")
                            item.price.shippingMembership != null -> Muted("Free delivery with ${item.price.shippingMembership}.")
                        }
                        if (item.product.thirdParty == true) {
                            Muted("Sold by ${item.product.seller ?: "another seller"}, a marketplace seller, not by ${item.storeName} itself.")
                        }
                        Badges(item)
                        item.deal.usualCents?.let { Muted("Usual price over 90 days: ${dollars(it)}") }
                        item.product.lowestPriceCents?.let { Muted("Lowest recorded: ${dollars(it)}") }
                        item.product.pickup?.let { there ->
                            Muted(if (there) "At your store: ready to collect today." else "At your store: not on the shelf; delivery only.")
                        }
                        item.product.lastSuccessAt?.let { Muted("Last checked ${ago(it)}") }
                        OutlinedButton(onClick = {
                            runCatching { context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(item.product.url))) }
                        }) { Text("Open at ${item.storeName}") }
                    }

                    if (detail.history.size >= 2) {
                        Card {
                            SectionTitle("Price history")
                            PriceChart(detail.history)
                        }
                    }

                    OtherStoresCard(
                        stores = detail.otherStores,
                        trackedTotalCents = item.price.totalCents,
                        onPriceMatch = { store -> onPriceMatch(store.comparisonId) },
                        onOpenProduct = { url -> runCatching { context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url))) } },
                        onDecide = { store, same ->
                            act(if (same) "Marked as the same product." else "Marked as a different product.") {
                                app.api.decideComparison(store.comparisonId, same)
                            }
                        },
                        onSearchAgain = { act("Searching the other stores again.") { app.api.compareAgain(productId) } },
                    )

                    AlertSettingsCard(item.watch, app.session.checkIntervalMinutes) { settings ->
                        act("Saved.") { app.api.updateWatch(productId, settings) }
                    }

                    OutlinedButton(onClick = { confirmRemove = true }, modifier = Modifier.fillMaxWidth()) {
                        Text("Stop tracking", color = LocalPalette.current.bad)
                    }
                }
            }
        }
    }

    if (confirmRemove) {
        AlertDialog(
            onDismissRequest = { confirmRemove = false },
            title = { Text("Stop tracking this product?") },
            text = { Text("Its alerts stop, and it leaves your watchlist.") },
            confirmButton = {
                TextButton(onClick = {
                    confirmRemove = false
                    scope.launch {
                        val result = app.attempt { app.api.removeWatch(productId) }
                        if (result.isSuccess) onRemoved() else snackbar.showSnackbar(result.problem())
                    }
                }) { Text("Stop tracking") }
            },
            dismissButton = { TextButton(onClick = { confirmRemove = false }) { Text("Keep") } },
        )
    }
}

/** The public price over time as a stepped line: a price holds until it changes. */
@Composable
private fun PriceChart(history: List<PricePoint>) {
    val now = System.currentTimeMillis() / 1000
    val oldest = history.filter { !it.member }.minOfOrNull { it.ts } ?: now
    // A stretch of time is offered once the history reaches back further than it.
    val offered = chartRanges.filter { (_, days) -> now - oldest > days * 86_400L }
    var days by rememberSaveable { mutableStateOf<Int?>(null) }
    if (offered.isNotEmpty()) {
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            (offered + ("All" to null)).forEach { (label, range) ->
                FilterChip(selected = days == range, onClick = { days = range }, label = { Text(label) })
            }
        }
    }
    val since = days?.let { now - it * 86_400L }
    val points = chartPoints(history, since)
    if (points.isEmpty() || (points.size < 2 && since == null)) {
        Muted("Not enough price changes yet.")
        return
    }
    val accent = LocalPalette.current.accent
    val grid = LocalPalette.current.border
    val low = points.minOf { it.priceCents }
    val high = points.maxOf { it.priceCents }
    val start = since ?: points.first().ts
    val end = maxOf(now, points.last().ts + 1)
    Canvas(Modifier.fillMaxWidth().height(140.dp)) {
        val pad = 6.dp.toPx()
        val width = size.width - 2 * pad
        val height = size.height - 2 * pad
        val span = (high - low).coerceAtLeast(1)
        fun x(ts: Long) = pad + width * (ts - start).toFloat() / (end - start).coerceAtLeast(1).toFloat()
        // A flat history sits in the middle rather than on the bottom edge.
        fun y(cents: Long) = if (high == low) pad + height / 2 else pad + height * (1f - (cents - low).toFloat() / span.toFloat())
        drawLine(grid, Offset(pad, pad), Offset(size.width - pad, pad))
        drawLine(grid, Offset(pad, size.height - pad), Offset(size.width - pad, size.height - pad))
        val path = Path()
        path.moveTo(x(points.first().ts), y(points.first().priceCents))
        for (i in 1 until points.size) {
            path.lineTo(x(points[i].ts), y(points[i - 1].priceCents))
            path.lineTo(x(points[i].ts), y(points[i].priceCents))
        }
        path.lineTo(x(end), y(points.last().priceCents))
        drawPath(path, accent, style = Stroke(width = 2.dp.toPx()))
        drawCircle(accent, radius = 3.5.dp.toPx(), center = Offset(x(end), y(points.last().priceCents)))
    }
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
        Muted("Low ${dollars(low)}")
        Muted("High ${dollars(high)}")
    }
}

@Composable
private fun OtherStoresCard(
    stores: List<OtherStore>,
    trackedTotalCents: Long?,
    onPriceMatch: (OtherStore) -> Unit,
    onOpenProduct: (String) -> Unit,
    onDecide: (OtherStore, Boolean) -> Unit,
    onSearchAgain: () -> Unit,
) {
    Card {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.CenterVertically) {
            SectionTitle("Other stores")
            TextButton(onClick = onSearchAgain) { Text(if (stores.isEmpty()) "Compare" else "Search again") }
        }
        if (stores.isEmpty()) {
            Muted("Look for this same product at the other stores and compare prices.")
        }
        stores.forEachIndexed { index, store ->
            if (index > 0) HorizontalDivider(color = LocalPalette.current.border)
            Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                    Text(store.storeName, fontWeight = FontWeight.Medium)
                    // The figure that can be compared: delivery included, for the same amount as the tracked product.
                    store.equivalentCents?.takeIf { store.product != null }?.let { Text(dollars(it), fontWeight = FontWeight.Bold) }
                }
                val found = store.product
                when {
                    found != null && (store.status == "found" || store.status == "possible") -> {
                        Text(
                            found.displayTitle,
                            color = LocalPalette.current.accent,
                            modifier = Modifier.clickable { onOpenProduct(found.url) },
                        )
                        Row(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
                            if (store.status == "found") Tag("Same product", TagKind.Good) else Tag("Possibly the same", TagKind.Warn)
                            if (store.aiJudged) Tag("Judged by AI", TagKind.Plain) else if (store.confirmed) Tag("Confirmed by you", TagKind.Plain)
                            if (found.inStock == false) Tag("Out of stock", TagKind.Bad)
                        }
                        if (store.marketplace) Tag("Marketplace seller" + (found.seller?.let { ": $it" } ?: ""), TagKind.Warn)
                        // What the figure is made of, when it is not simply the listed price.
                        val listed = store.yourCents
                        if (store.theirAmount != null && store.yourAmount != null && listed != null) {
                            Muted("Sold as ${store.theirAmount} for ${dollars(listed)}. The price shown is for ${store.yourAmount}, the amount you track.")
                        }
                        val delivery = store.shippingCents
                        if (delivery != null && delivery > 0) Muted("Includes ${dollars(delivery)} delivery.") else if (store.shippingExtra) Muted("Delivery is charged on top.")
                        store.reason?.let { Muted(it) }
                        Row {
                            if (store.worthMatching(trackedTotalCents)) {
                                TextButton(onClick = { onPriceMatch(store) }) { Text("Price match card") }
                            }
                            if (store.status == "possible" || store.aiJudged) {
                                TextButton(onClick = { onDecide(store, true) }) { Text("Yes, same product") }
                            }
                            TextButton(onClick = { onDecide(store, false) }) { Text("Not the same") }
                        }
                    }
                    store.status == "pending" -> Muted("Searching…")
                    store.status == "failed" -> Muted("Could not search: ${store.reason ?: "unknown problem"}")
                    store.status == "rejected" -> Muted("You said the suggested product was not the same.")
                    else -> Muted(store.reason ?: "Not found")
                }
            }
        }
    }
}

@Composable
private fun AlertSettingsCard(watch: WatchSettings, usualMinutes: Int, onSave: (WatchSettings) -> Unit) {
    var target by remember(watch) { mutableStateOf(watch.targetPriceCents?.let { "%.2f".format(it / 100.0) }.orEmpty()) }
    var drop by remember(watch) { mutableStateOf(watch.dropPercent?.let { if (it % 1.0 == 0.0) it.toInt().toString() else it.toString() }.orEmpty()) }
    var anyDrop by remember(watch) { mutableStateOf(watch.notifyAnyDrop) }
    var backInStock by remember(watch) { mutableStateOf(watch.notifyBackInStock) }
    var note by remember(watch) { mutableStateOf(watch.note.orEmpty()) }
    var checkMinutes by remember(watch) { mutableStateOf(watch.checkMinutes) }
    var buyBy by remember(watch) { mutableStateOf(watch.buyBy) }
    var choosingDay by remember { mutableStateOf(false) }
    var problem by remember { mutableStateOf<String?>(null) }

    Card {
        SectionTitle("Alert me when")
        OutlinedTextField(
            value = target,
            onValueChange = { target = it },
            label = { Text("The price reaches") },
            placeholder = { Text("No target") },
            prefix = { Text("$") },
            singleLine = true,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Decimal),
            modifier = Modifier.fillMaxWidth(),
        )
        Muted("Compared with your own price, after your discounts.")
        OutlinedTextField(
            value = drop,
            onValueChange = { drop = it },
            label = { Text("It drops by at least") },
            placeholder = { Text("Any amount") },
            suffix = { Text("%") },
            singleLine = true,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Decimal),
            modifier = Modifier.fillMaxWidth(),
        )
        CheckRow("Any price drop (when no minimum is set above)", anyDrop) { anyDrop = it }
        CheckRow("It comes back in stock", backInStock) { backInStock = it }
        OutlinedTextField(
            value = note,
            onValueChange = { note = it.take(200) },
            label = { Text("Note") },
            singleLine = true,
            modifier = Modifier.fillMaxWidth(),
        )
        Text("I want to buy by", fontWeight = FontWeight.Medium)
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
            OutlinedButton(onClick = { choosingDay = true }) {
                Text(buyBy?.let { buyByText(it, System.currentTimeMillis() / 1000).removePrefix("Buy by ") } ?: "No day chosen")
            }
            if (buyBy != null) TextButton(onClick = { buyBy = null }) { Text("Clear") }
        }
        Muted(
            "Optional. In the last week before that day every price drop is announced and the price is checked more often. " +
                "You are reminded a few days ahead, the day before and on the day, with how the price compares.",
        )
        Text("Check the price", fontWeight = FontWeight.Medium)
        val paces = listOf<Int?>(null) + paceChoices(usualMinutes, watch.checkMinutes)
        paces.forEach { pace ->
            Row(
                Modifier.fillMaxWidth().clickable { checkMinutes = pace },
                verticalAlignment = Alignment.CenterVertically,
            ) {
                RadioButton(selected = checkMinutes == pace, onClick = { checkMinutes = pace })
                Text(if (pace == null) "At the usual pace (about every ${paceText(usualMinutes)})" else "Every ${paceText(pace)}")
            }
        }
        Muted(
            "Closer checks catch a sale that only lasts a few hours. The same product at the other stores is checked as often. " +
                "Stores notice frequent visits, so keep this for the few things you are waiting on.",
        )
        problem?.let { Text(it, color = LocalPalette.current.bad) }
        Button(onClick = {
            val targetCents = if (target.isBlank()) null else parseDollars(target)
            val dropPercent = if (drop.isBlank()) null else drop.trim().toDoubleOrNull()
            problem = when {
                target.isNotBlank() && targetCents == null -> "Type the target as an amount, such as 49.99."
                drop.isNotBlank() && (dropPercent == null || dropPercent <= 0 || dropPercent > 100) -> "The drop is a percentage between 1 and 100."
                else -> null
            }
            if (problem == null) {
                onSave(WatchSettings(targetCents, dropPercent, anyDrop, backInStock, note.trim().ifEmpty { null }, checkMinutes, buyBy))
            }
        }) { Text("Save") }
    }

    if (choosingDay) {
        // Today in UTC, as the picker counts days; earlier days cannot be chosen.
        val today = System.currentTimeMillis() / 86_400_000L * 86_400_000L
        val picker = rememberDatePickerState(
            initialSelectedDateMillis = buyBy?.let(::pickerMillis),
            selectableDates = object : SelectableDates {
                override fun isSelectableDate(utcTimeMillis: Long) = utcTimeMillis >= today
            },
        )
        DatePickerDialog(
            onDismissRequest = { choosingDay = false },
            confirmButton = {
                TextButton(onClick = {
                    picker.selectedDateMillis?.let { buyBy = buyByFromPicker(it) }
                    choosingDay = false
                }) { Text("Choose") }
            },
            dismissButton = { TextButton(onClick = { choosingDay = false }) { Text("Cancel") } },
        ) { DatePicker(state = picker) }
    }
}

@Composable
private fun CheckRow(label: String, checked: Boolean, onChange: (Boolean) -> Unit) {
    Row(
        Modifier.fillMaxWidth().clickable { onChange(!checked) },
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Checkbox(checked = checked, onCheckedChange = onChange)
        Text(label)
    }
}
