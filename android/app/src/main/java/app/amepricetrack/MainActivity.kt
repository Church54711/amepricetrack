package app.amepricetrack

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.amepricetrack.ui.AppRoot
import app.amepricetrack.ui.AppTheme
import app.amepricetrack.ui.paletteFor
import app.amepricetrack.work.AlertWorker

/** Something that arrived from outside the app and is waiting to be acted on. */
sealed interface Incoming {
    /** A product link (or the text a store app shares) to start tracking. */
    data class SharedText(val text: String) : Incoming

    /** A notification was tapped. */
    data class OpenProduct(val id: Long) : Incoming
}

class MainActivity : ComponentActivity() {
    private var incoming by mutableStateOf<Incoming?>(null)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val app = application as App
        if (savedInstanceState == null) incoming = read(intent)
        setContent {
            val session by app.session.state.collectAsStateWithLifecycle()
            val dark = paletteFor(session.theme, session.accent, isSystemInDarkTheme()).dark
            // Status and navigation bar icons that can be seen on the theme's background.
            LaunchedEffect(dark) {
                val transparent = android.graphics.Color.TRANSPARENT
                val style = if (dark) SystemBarStyle.dark(transparent) else SystemBarStyle.light(transparent, transparent)
                enableEdgeToEdge(statusBarStyle = style, navigationBarStyle = style)
            }
            AppTheme(session.theme, session.accent) {
                AppRoot(app = app, session = session, incoming = incoming, onIncomingHandled = { incoming = null })
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        read(intent)?.let { incoming = it }
    }

    private fun read(intent: Intent?): Incoming? {
        if (intent == null) return null
        if (intent.action == Intent.ACTION_SEND && intent.type == "text/plain") {
            val text = intent.getStringExtra(Intent.EXTRA_TEXT)?.trim().orEmpty()
            return if (text.isEmpty()) null else Incoming.SharedText(text)
        }
        val productId = intent.getLongExtra(AlertWorker.EXTRA_PRODUCT_ID, -1L)
        return if (productId > 0) Incoming.OpenProduct(productId) else null
    }
}
