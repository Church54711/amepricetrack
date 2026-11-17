package app.amepricetrack.work

import android.annotation.SuppressLint
import android.content.Context
import android.view.ViewGroup
import android.webkit.CookieManager
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebView
import android.webkit.WebViewClient
import app.amepricetrack.data.decodeJavascriptString
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.delay
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withTimeoutOrNull
import java.io.ByteArrayInputStream
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

    /**
     * While true, pictures, video and fonts are not downloaded. A price is
     * text, so reading one needs none of them, and they are most of what a
     * store page costs in data and battery. Turned off while a person browses.
     */
    @Volatile
    var lean: Boolean = false
        set(value) {
            field = value
            view.settings.blockNetworkImage = value
            view.settings.loadsImagesAutomatically = !value
        }

    init {
        view.webViewClient = object : WebViewClient() {
            override fun onPageFinished(webView: WebView, url: String) {
                pageDone?.complete(Unit)
            }

            // Called off the main thread for everything the page asks for.
            override fun shouldInterceptRequest(webView: WebView, request: WebResourceRequest): WebResourceResponse? =
                if (lean && isHeavy(request.url?.path)) nothing() else null
        }
    }

    /** Loads a page, waits for it to settle, and returns its HTML as rendered. */
    suspend fun read(url: String): String {
        val done = CompletableDeferred<Unit>()
        pageDone = done
        view.loadUrl(url)
        withTimeoutOrNull(PAGE_TIMEOUT_MS) { done.await() }
        settle()
        return script("document.documentElement.outerHTML")
    }

    /**
     * Stores fill in the price with scripts after the page itself has loaded.
     * Rather than always waiting the longest a slow store needs, this waits
     * until the page has stopped growing, within sensible limits.
     */
    private suspend fun settle() {
        var waited = 0L
        var lastSize = ""
        while (waited < SETTLE_MAX_MS) {
            delay(SETTLE_STEP_MS)
            waited += SETTLE_STEP_MS
            val size = script("document.documentElement.outerHTML.length")
            if (waited >= SETTLE_MIN_MS && size == lastSize) return
            lastSize = size
        }
    }

    private suspend fun script(code: String): String = suspendCancellableCoroutine { continuation ->
        view.evaluateJavascript(code) { value -> continuation.resume(decodeJavascriptString(value)) }
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
        private const val PAGE_TIMEOUT_MS = 25_000L
        private const val SETTLE_STEP_MS = 700L
        private const val SETTLE_MIN_MS = 2_100L
        private const val SETTLE_MAX_MS = 5_600L

        private val HEAVY = setOf(
            "png", "jpg", "jpeg", "gif", "webp", "avif", "svg", "ico", "bmp",
            "woff", "woff2", "ttf", "otf", "eot",
            "mp4", "webm", "m4s", "m3u8", "mp3", "mov",
        )

        /** True for an address that is a picture, a font or a video, going by its ending. */
        fun isHeavy(path: String?): Boolean {
            val ending = path?.substringAfterLast('/')?.substringAfterLast('.', "")?.lowercase() ?: return false
            return ending in HEAVY
        }

        private fun nothing() = WebResourceResponse("text/plain", "utf-8", ByteArrayInputStream(ByteArray(0)))

        @SuppressLint("SetJavaScriptEnabled") // store pages need their scripts to show a price
        fun create(context: Context): PageReader {
            val view = WebView(context)
            view.layoutParams = ViewGroup.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT)
            // A size of its own, for when it reads pages without being on screen.
            view.layout(0, 0, 1080, 1920)
            view.settings.javaScriptEnabled = true
            view.settings.domStorageEnabled = true
            view.settings.mediaPlaybackRequiresUserGesture = true
            CookieManager.getInstance().setAcceptCookie(true)
            return PageReader(view)
        }
    }
}
