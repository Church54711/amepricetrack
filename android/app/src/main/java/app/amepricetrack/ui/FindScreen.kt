@file:OptIn(ExperimentalMaterial3Api::class)

package app.amepricetrack.ui

import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.Button
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
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import app.amepricetrack.App
import app.amepricetrack.data.FindAnswer
import app.amepricetrack.data.FindStart
import app.amepricetrack.data.expandShortBarcode
import com.google.mlkit.vision.barcode.common.Barcode
import com.google.mlkit.vision.codescanner.GmsBarcodeScannerOptions
import com.google.mlkit.vision.codescanner.GmsBarcodeScanning
import kotlinx.coroutines.Job
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.sync.withPermit

/**
 * Opens the phone's barcode scanner. The scanner is Google's own screen, so
 * this app needs no camera permission. Gives back the product's number.
 */
fun scanBarcode(context: Context, onCode: (String) -> Unit, onProblem: (String) -> Unit) {
    val options = GmsBarcodeScannerOptions.Builder()
        .setBarcodeFormats(Barcode.FORMAT_UPC_A, Barcode.FORMAT_UPC_E, Barcode.FORMAT_EAN_13, Barcode.FORMAT_EAN_8)
        .build()
    try {
        GmsBarcodeScanning.getClient(context, options).startScan()
            .addOnSuccessListener { barcode ->
                val code = barcode.rawValue?.filter { it.isDigit() }.orEmpty()
                if (code.isEmpty()) onProblem("That barcode could not be read.") else onCode(expandShortBarcode(code))
            }
            .addOnFailureListener {
                onProblem(
                    "The barcode scanner could not start. Google Play services downloads it the first time it is used; " +
                        "try again in a moment, or type the number under the barcode.",
                )
            }
    } catch (e: Exception) {
        onProblem("The barcode scanner is not available on this phone. Type the number under the barcode instead.")
    }
}

/** Stores asked at the same time. The server reads several of them through one browser. */
private const val STORES_AT_ONCE = 3

/**
 * Looks a product up by name or barcode at every store, to pick one to
 * track. `initial` is searched for straight away.
 */
@Composable
fun FindScreen(app: App, initial: String, onBack: () -> Unit, onOpenProduct: (Long) -> Unit, onAdded: (Long) -> Unit) {
    var words by rememberSaveable { mutableStateOf(initial) }
    var start by remember { mutableStateOf<Load<FindStart>?>(null) }
    val answers = remember { mutableStateMapOf<String, Load<FindAnswer>>() }
    // The link being added, to show which button was pressed.
    var adding by remember { mutableStateOf<String?>(null) }
    var search by remember { mutableStateOf<Job?>(null) }
    val snackbar = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()
    val context = LocalContext.current

    fun find(text: String) {
        if (text.isBlank()) return
        search?.cancel()
        answers.clear()
        start = Load.Loading
        search = scope.launch {
            val begun = app.attempt { app.api.findStart(text) }
            val found = begun.getOrNull()
            if (found == null) {
                start = Load.Failed(begun.problem())
                return@launch
            }
            start = Load.Ready(found)
            found.stores.forEach { answers[it.id] = Load.Loading }
            val turns = Semaphore(STORES_AT_ONCE)
            coroutineScope {
                found.stores.forEach { store ->
                    launch {
                        turns.withPermit {
                            val answer = app.attempt { app.api.findAt(found.query, store.id) }
                            answers[store.id] = answer.fold({ Load.Ready(it) }, { Load.Failed(answer.problem()) })
                        }
                    }
                }
            }
        }
    }

    fun track(url: String) {
        if (adding != null) return
        adding = url
        scope.launch {
            val added = app.attempt { app.api.addWatch(url) }
            adding = null
            added.onSuccess { onAdded(it.product.id) }
            added.onFailure { snackbar.showSnackbar(added.problem()) }
        }
    }

    LaunchedEffect(Unit) { if (start == null) find(initial) }

    Scaffold(
        snackbarHost = { SnackbarHost(snackbar) },
        topBar = {
            TopAppBar(
                title = { Text("Find a product") },
                navigationIcon = {
                    IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back") }
                },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = MaterialTheme.colorScheme.background),
            )
        },
    ) { padding ->
        LazyColumn(
            Modifier.padding(padding).fillMaxSize().imePadding(),
            contentPadding = PaddingValues(16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            item {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    OutlinedTextField(
                        value = words,
                        onValueChange = { words = it },
                        label = { Text("Product name or barcode") },
                        singleLine = true,
                        keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
                        keyboardActions = KeyboardActions(onSearch = { find(words) }),
                        modifier = Modifier.fillMaxWidth(),
                    )
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Button(onClick = { find(words) }, enabled = words.isNotBlank()) { Text("Search") }
                        OutlinedButton(onClick = {
                            scanBarcode(
                                context,
                                onCode = {
                                    words = it
                                    find(it)
                                },
                                onProblem = { scope.launch { snackbar.showSnackbar(it) } },
                            )
                        }) { Text("Scan a barcode") }
                    }
                }
            }

            when (val begun = start) {
                null -> {}
                Load.Loading -> item { Muted("Asking your server…") }
                is Load.Failed -> item { Banner(begun.message, good = false) }
                is Load.Ready -> {
                    val found = begun.value
                    item {
                        Muted(
                            (if (found.barcode) "Stores searched for the barcode ${found.query}." else "Stores searched for “${found.query}”.") +
                                " Pick the right one to start tracking it; it is then looked up at the other stores for you.",
                        )
                    }
                    if (found.tracked.isNotEmpty()) {
                        item {
                            Card {
                                SectionTitle("Already on your list")
                                found.tracked.forEach { tracked ->
                                    Column(Modifier.fillMaxWidth().clickable { onOpenProduct(tracked.productId) }) {
                                        Text(tracked.title, color = LocalPalette.current.accent)
                                        Muted(tracked.storeName)
                                    }
                                }
                            }
                        }
                    }
                    items(found.stores, key = { it.id }) { store ->
                        Card {
                            SectionTitle(store.name)
                            when (val answer = answers[store.id]) {
                                null, Load.Loading -> Muted("Searching…")
                                is Load.Failed -> Muted(answer.message)
                                is Load.Ready -> StoreResults(
                                    answer = answer.value,
                                    adding = adding,
                                    onView = { url -> runCatching { context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url))) } },
                                    onTrack = ::track,
                                )
                            }
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun StoreResults(answer: FindAnswer, adding: String?, onView: (String) -> Unit, onTrack: (String) -> Unit) {
    when {
        answer.problem != null -> Muted("Could not search this store: ${answer.problem}")
        answer.results.isEmpty() -> Muted("Nothing found.")
        else -> answer.results.forEachIndexed { index, item ->
            if (index > 0) HorizontalDivider(color = LocalPalette.current.border)
            Row(
                Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(10.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(
                    item.title,
                    modifier = Modifier.weight(1f).clickable { onView(item.url) },
                    fontWeight = FontWeight.Medium,
                    maxLines = 3,
                )
                OutlinedButton(onClick = { onTrack(item.url) }, enabled = adding == null) {
                    Text(if (adding == item.url) "Adding…" else "Track")
                }
            }
        }
    }
}
