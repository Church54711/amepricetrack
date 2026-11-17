package app.amepricetrack.work

import android.annotation.SuppressLint
import android.content.Context
import android.view.ViewGroup
import android.webkit.CookieManager
import android.webkit.WebView
import android.webkit.WebViewClient
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.delay
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withTimeoutOrNull
import app.amepricetrack.data.decodeJavascriptString
import kotlin.coroutines.resume

/**
 * A browser inside the app that loads store pages the way the person would
 * see them. Its cookies stay on the phone, so a store the person signed in
 * at once is still signed in when a page is read later, on screen or in the
 * background. Everything here runs on the main thread, as Android requires.
 */
class PageReader private constructor(val view: WebView) {
    /** Completed when the page being loaded has finished. */
    private var pageDone: CompletableDeferred<Unit>? = null

    init {
        view.webViewClient = object : WebViewClient() {
            override fun onPageFinished(webView: WebView, url: String) {
                pageDone?.complete(Unit)
            }
        }
    }

    /** Loads a page, waits for it to settle, and returns its HTML as rendered. */
    suspend fun read(url: String): String {
        val done = CompletableDeferred<Unit>()
        pageDone = done
        view.loadUrl(url)
        withTimeoutOrNull(PAGE_TIMEOUT_MS) { done.await() }
        // Stores fill in the price with scripts after the page itself has loaded.
        delay(SETTLE_MS)
        return suspendCancellableCoroutine { continuation ->
            view.evaluateJavascript("document.documentElement.outerHTML") { value ->
                continuation.resume(decodeJavascriptString(value))
            }
        }
    }

    /** Leaves the last page, so nothing keeps running in it. */
    fun rest() {
        view.loadUrl("about:blank")
    }

    fun close() {
        CookieManager.getInstance().flush()
        view.destroy()
    }

    companion object {
        private const val PAGE_TIMEOUT_MS = 30_000L
        private const val SETTLE_MS = 3_500L

        @SuppressLint("SetJavaScriptEnabled") // store pages need their scripts to show a price
        fun create(context: Context): PageReader {
            val view = WebView(context)
            view.layoutParams = ViewGroup.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT)
            // A size of its own, for when it reads pages without being on screen.
            view.layout(0, 0, 1080, 1920)
            view.settings.javaScriptEnabled = true
            view.settings.domStorageEnabled = true
            CookieManager.getInstance().setAcceptCookie(true)
            return PageReader(view)
        }
    }
}
