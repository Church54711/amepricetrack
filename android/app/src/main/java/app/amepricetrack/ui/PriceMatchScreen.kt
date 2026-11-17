@file:OptIn(ExperimentalMaterial3Api::class)

package app.amepricetrack.ui

import android.content.Intent
import android.net.Uri
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.amepricetrack.App
import app.amepricetrack.data.OtherStore
import app.amepricetrack.data.Product
import app.amepricetrack.data.ProductDetail
import app.amepricetrack.data.dollars
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** A price read longer ago than this is worth checking again before asking for a match. */
private const val STALE_AFTER_SECONDS = 6 * 3600L

/** Something to check before asking for a price match. `caution` marks the ones that may get it refused. */
data class MatchNote(val text: String, val caution: Boolean)

/** What the person should know about the other store's offer before asking their store to match it. */
fun matchNotes(other: OtherStore, theirStore: String, nowSeconds: Long): List<MatchNote> {
    val listing = other.product ?: return emptyList()
    val notes = mutableListOf<MatchNote>()
    val seller = listing.seller
    if (other.marketplace) {
        notes += MatchNote(
            "Sold by ${seller ?: "another seller"}, a marketplace seller. Stores usually match only what the other store sells itself.",
            caution = true,
        )
    } else if (seller != null) {
        notes += MatchNote("Sold by $seller.", caution = false)
    }
    when (listing.inStock) {
        false -> notes += MatchNote("Out of stock at $theirStore. Stores usually match only items that can be bought now.", caution = true)
        true -> notes += MatchNote("In stock at $theirStore.", caution = false)
        null -> {}
    }
    val shipping = other.shippingCents
    if (shipping != null && shipping > 0) {
        notes += MatchNote("Delivery adds ${dollars(shipping)} at $theirStore. Stores usually match the item's price, not delivery.", caution = true)
    } else if (other.shippingExtra) {
        notes += MatchNote("Delivery is charged on top at $theirStore.", caution = true)
    }
    if (other.aiJudged) {
        notes += MatchNote("Your AI judged these to be the same product. Check that the model or barcode matches.", caution = true)
    } else if (other.confirmed) {
        notes += MatchNote("You confirmed these are the same product.", caution = false)
    } else {
        other.reason?.let { notes += MatchNote("Matched by: $it.", caution = false) }
    }
    val read = listing.lastSuccessAt
    if (read == null || nowSeconds - read > STALE_AFTER_SECONDS) {
        notes += MatchNote("This price was read a while ago. Check it again before you ask.", caution = true)
    }
    return notes
}

/** A screen to show at the register when asking a store to match another store's price. */
@Composable
fun PriceMatchScreen(app: App, productId: Long, comparisonId: Long, onBack: () -> Unit) {
    var state by remember { mutableStateOf<Load<ProductDetail>>(Load.Loading) }
    var checking by remember { mutableStateOf(false) }
    val snackbar = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()
    val context = LocalContext.current

    suspend fun load() {
        val result = app.attempt { app.api.product(productId) }
        result.onSuccess { state = Load.Ready(it) }
        result.onFailure { if (state is Load.Ready) snackbar.showSnackbar(result.problem()) else state = Load.Failed(result.problem()) }
    }

    LaunchedEffect(productId) { load() }

    Scaffold(
        snackbarHost = { SnackbarHost(snackbar) },
        topBar = {
            TopAppBar(
                title = { Text("Price match") },
                navigationIcon = {
                    IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back") }
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
                val other = detail.otherStores.firstOrNull { it.comparisonId == comparisonId }
                val listing = other?.product
                Column(
                    Modifier.padding(padding).fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp),
                    verticalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    if (other == null || listing == null || other.status != "found") {
                        Banner("This match is no longer there. Go back and search the other stores again.", good = false)
                        return@Column
                    }
                    val mine = detail.item.product
                    val myStore = detail.item.storeName
                    val theirStore = other.storeName
                    Muted("Show this at $myStore when asking them to match the price at $theirStore.")

                    Side(myStore, mine, highlight = false)
                    Side(theirStore, listing, highlight = true)
                    Muted("Read ${listing.lastSuccessAt?.let(::ago) ?: "never"}")

                    val here = mine.currentPriceCents
                    val there = listing.currentPriceCents
                    if (here != null && there != null && there < here) {
                        Text("${dollars(here - there)} lower at $theirStore.", fontWeight = FontWeight.Bold)
                    } else {
                        Banner("The listed price at $theirStore is not lower than at $myStore right now.", good = false)
                    }

                    Button(
                        onClick = { runCatching { context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(listing.url))) } },
                        modifier = Modifier.fillMaxWidth(),
                    ) { Text("Open the listing at $theirStore") }

                    val notes = matchNotes(other, theirStore, System.currentTimeMillis() / 1000)
                    if (notes.isNotEmpty()) {
                        Card {
                            SectionTitle("Before you ask")
                            notes.forEach { note ->
                                Text("• ${note.text}", color = if (note.caution) LocalPalette.current.warn else LocalPalette.current.text)
                            }
                        }
                    }

                    OutlinedButton(
                        enabled = !checking,
                        modifier = Modifier.fillMaxWidth(),
                        onClick = {
                            checking = true
                            scope.launch {
                                val asked = app.attempt { app.api.refreshComparison(comparisonId) }
                                if (asked.isFailure) {
                                    checking = false
                                    snackbar.showSnackbar(asked.problem())
                                    return@launch
                                }
                                // The server reads both pages in the background; look again as they come in.
                                repeat(3) {
                                    delay(8000)
                                    load()
                                }
                                checking = false
                            }
                        },
                    ) { Text(if (checking) "Checking both prices…" else "Check both prices again") }

                    Muted(
                        "Price-match rules differ from store to store and change over time. Most ask for the identical item, in stock, " +
                            "sold and shipped by the other store itself. Check the current policy at $myStore before relying on this.",
                    )
                }
            }
        }
    }
}

/** One store's side of the comparison: what it is, what it costs, and the numbers that identify it. */
@Composable
private fun Side(store: String, product: Product, highlight: Boolean) {
    val p = LocalPalette.current
    Surface(
        modifier = Modifier.fillMaxWidth(),
        color = p.surface,
        contentColor = p.text,
        shape = RoundedCornerShape(12.dp),
        border = BorderStroke(1.dp, if (highlight) p.good else p.border),
    ) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text(store.uppercase(), color = p.muted, style = MaterialTheme.typography.labelMedium, fontWeight = FontWeight.SemiBold)
            Text(product.displayTitle, fontWeight = FontWeight.SemiBold)
            val price = product.currentPriceCents
            if (price != null) {
                Text(
                    dollars(price),
                    style = MaterialTheme.typography.headlineMedium,
                    fontWeight = FontWeight.Bold,
                    color = if (highlight) p.good else Color.Unspecified,
                )
            } else {
                Muted("No price read yet")
            }
            product.model?.let { Muted("Model $it") }
            product.barcode?.let { Muted("Barcode $it") }
            product.itemNumber?.let { Muted("Item number $it") }
        }
    }
}
