@file:OptIn(ExperimentalMaterial3Api::class)

package app.amepricetrack.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.material3.OutlinedButton
import androidx.compose.ui.platform.LocalContext
import app.amepricetrack.App
import app.amepricetrack.data.WatchItem
import app.amepricetrack.data.dollars
import app.amepricetrack.data.looksLikeLink
import coil.compose.AsyncImage
import kotlinx.coroutines.launch

@Composable
fun WatchlistScreen(app: App, version: Int, onOpen: (Long) -> Unit, onFind: (String) -> Unit, onMessage: (String) -> Unit) {
    var state by remember { mutableStateOf<Load<List<WatchItem>>>(Load.Loading) }
    var refreshing by remember { mutableStateOf(false) }
    var adding by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()

    suspend fun load() {
        val result = app.attempt { app.api.watches() }
        result.onSuccess { state = Load.Ready(it) }
        // Keep showing the list when a refresh fails.
        result.onFailure { if (state is Load.Ready) onMessage(result.problem()) else state = Load.Failed(result.problem()) }
    }

    LaunchedEffect(version) { load() }

    Box(Modifier.fillMaxSize()) {
        when (val current = state) {
            Load.Loading -> LoadingBox()
            is Load.Failed -> ProblemBox(current.message) {
                state = Load.Loading
                scope.launch { load() }
            }
            is Load.Ready -> PullToRefreshBox(
                isRefreshing = refreshing,
                onRefresh = {
                    refreshing = true
                    scope.launch {
                        load()
                        refreshing = false
                    }
                },
            ) {
                LazyColumn(
                    Modifier.fillMaxSize(),
                    contentPadding = PaddingValues(start = 16.dp, end = 16.dp, top = 16.dp, bottom = 96.dp),
                    verticalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    item { Text("Watchlist", style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.Bold) }
                    if (current.value.isEmpty()) {
                        item {
                            Card {
                                SectionTitle("Nothing tracked yet")
                                Muted(
                                    "Tap + to paste a product link, type a product's name, or scan its barcode. " +
                                        "You can also use Share in a store's app or your browser and choose “Track price”.",
                                )
                            }
                        }
                    }
                    items(current.value, key = { it.product.id }) { item -> WatchRow(item) { onOpen(item.product.id) } }
                }
            }
        }
        FloatingActionButton(
            onClick = { adding = true },
            containerColor = LocalPalette.current.accent,
            contentColor = LocalPalette.current.accentText,
            modifier = Modifier.align(Alignment.BottomEnd).padding(20.dp),
        ) { Icon(Icons.Filled.Add, contentDescription = "Add a product") }
    }

    if (adding) {
        AddDialog(
            app = app,
            onDismiss = { adding = false },
            onAdded = {
                adding = false
                scope.launch { load() }
                onOpen(it)
            },
            onFind = {
                adding = false
                onFind(it)
            },
        )
    }
}

@Composable
private fun AddDialog(app: App, onDismiss: () -> Unit, onAdded: (Long) -> Unit, onFind: (String) -> Unit) {
    val clipboard = LocalClipboardManager.current
    val context = LocalContext.current
    // A link already on the clipboard is offered, since that is the usual way here.
    var link by remember { mutableStateOf(clipboard.getText()?.text?.trim()?.takeIf(::looksLikeLink).orEmpty()) }
    var problem by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()

    AlertDialog(
        onDismissRequest = { if (!busy) onDismiss() },
        title = { Text("Track a product") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                OutlinedTextField(
                    value = link,
                    onValueChange = { link = it },
                    label = { Text("Product link, name or barcode") },
                    maxLines = 4,
                    modifier = Modifier.fillMaxWidth(),
                )
                problem?.let { Text(it, color = LocalPalette.current.bad) }
                Muted("Paste the link of a product page, or type a name or the number under a barcode to look it up at each store.")
                OutlinedButton(
                    enabled = !busy,
                    onClick = { scanBarcode(context, onCode = onFind, onProblem = { problem = it }) },
                ) { Text("Scan a barcode") }
            }
        },
        confirmButton = {
            TextButton(
                enabled = !busy && link.isNotBlank(),
                onClick = {
                    // A name or a barcode rather than a link: look it up at the stores.
                    if (!looksLikeLink(link)) {
                        onFind(link.trim())
                        return@TextButton
                    }
                    busy = true
                    problem = null
                    scope.launch {
                        val result = app.attempt { app.api.addWatch(link) }
                        busy = false
                        result.onSuccess { onAdded(it.product.id) }
                        result.onFailure { problem = result.problem() }
                    }
                },
            ) { Text(if (busy) "Adding…" else if (link.isBlank() || looksLikeLink(link)) "Track" else "Find") }
        },
        dismissButton = { TextButton(onClick = onDismiss, enabled = !busy) { Text("Cancel") } },
    )
}

@Composable
fun Thumbnail(url: String?, size: Int) {
    val shape = RoundedCornerShape(8.dp)
    if (url == null) {
        Box(Modifier.size(size.dp).clip(shape).background(LocalPalette.current.border))
    } else {
        AsyncImage(
            model = url,
            contentDescription = null,
            contentScale = ContentScale.Fit,
            // Product photos are shot on white; a white tile keeps them tidy on a dark theme.
            modifier = Modifier.size(size.dp).clip(shape).background(Color.White).padding(3.dp),
        )
    }
}

/** The labels under a product's name: deal, personal price, stock, other stores. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun Badges(item: WatchItem) {
    val p = item.product
    FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        if (p.inStock == false) Tag("Out of stock", TagKind.Bad)
        item.deal.percentBelow?.takeIf { it >= 1 }?.let {
            val against = if (item.deal.comparedTo == "average") "usual price" else "list price"
            Tag("${it.toInt()}% below $against", TagKind.Good)
        }
        if (item.deal.atLowest) Tag("Lowest recorded", TagKind.Good)
        if (item.price.memberCents != null) Tag("Your signed-in price", TagKind.Member)
        item.price.discountName?.let { Tag(it, TagKind.Member) }
        val yours = item.price.yourCents
        // Compared with delivery included on both sides.
        val total = item.price.totalCents
        item.elsewhere?.takeIf { total != null && it.cents < total }?.let { Tag("${dollars(it.cents)} at ${it.storeName}", TagKind.Warn) }
        if (p.thirdParty == true) Tag("Marketplace seller", TagKind.Warn)
        if (item.watch.checkMinutes != null) Tag("Watched closely", TagKind.Plain)
        val target = item.watch.targetPriceCents
        if (target != null && yours != null && yours <= target) Tag("Target reached", TagKind.Good)
        if (p.currentPriceCents == null && p.lastError != null) Tag("Could not read price", TagKind.Bad)
    }
}

@Composable
private fun WatchRow(item: WatchItem, onClick: () -> Unit) {
    val p = item.product
    Card(Modifier.clickable(onClick = onClick)) {
        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Thumbnail(p.imageUrl, 64)
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Text(p.displayTitle, maxLines = 2, overflow = TextOverflow.Ellipsis, fontWeight = FontWeight.Medium)
                Muted(item.storeName)
                Badges(item)
            }
            Column(horizontalAlignment = Alignment.End) {
                val yours = item.price.yourCents
                if (yours != null) {
                    Text(dollars(yours), fontWeight = FontWeight.Bold, style = MaterialTheme.typography.titleMedium)
                    val listed = item.price.publicCents
                    if (listed != null && listed > yours) {
                        Text(dollars(listed), color = LocalPalette.current.muted, textDecoration = TextDecoration.LineThrough)
                    }
                } else {
                    Muted(if (p.lastSuccessAt == null && p.lastError == null) "Checking…" else "No price")
                }
            }
        }
    }
}
