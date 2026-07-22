package se.danielfalk.pegasus

import android.annotation.SuppressLint
import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Intent
import android.content.pm.ApplicationInfo
import android.os.Build
import android.os.Bundle
import android.view.View
import android.view.WindowManager
import android.webkit.JavascriptInterface
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.webkit.WebViewAssetLoader
import java.io.ByteArrayInputStream
import java.io.IOException

/**
 * Full-screen WebView hosting the bundled web build. Assets are served
 * through WebViewAssetLoader on https://appassets.androidplatform.net —
 * the Android twin of the iOS pegasus:// scheme handler, and needed for
 * the same reason: the game fetches its wasm, level files, manifest and
 * config at runtime, which requires a real secure origin (file:// URLs
 * break fetch and give localStorage no stable home).
 */
class MainActivity : Activity() {
    private lateinit var webView: WebView

    @SuppressLint("SetJavaScriptEnabled")
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enterImmersiveMode()

        val assetLoader = WebViewAssetLoader.Builder()
            .addPathHandler("/", WebRootPathHandler(this))
            .build()

        webView = WebView(this)
        webView.setBackgroundColor(0xFF05060F.toInt())
        webView.settings.apply {
            javaScriptEnabled = true
            // localStorage: settings, pilot name, board cache.
            domStorageEnabled = true
            mediaPlaybackRequiresUserGesture = false
            allowFileAccess = false
            allowContentAccess = false
        }
        // The page's syncWakeLock calls this while the canvas is live
        // (flying / watching a replay) so a hands-off glide can't hit the
        // screen timeout; any open menu screen toggles it back off. Only
        // bundled content runs in this WebView (external links leave for
        // the browser), so exposing the interface is safe.
        webView.addJavascriptInterface(KeepAwakeBridge(), "PegasusApp")
        WebView.setWebContentsDebuggingEnabled(
            applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE != 0
        )

        webView.webViewClient = object : WebViewClient() {
            override fun shouldInterceptRequest(
                view: WebView,
                request: WebResourceRequest
            ): WebResourceResponse? =
                // Bundle requests are answered from assets; anything else
                // (the score/analytics API, CloudFront replays) returns null
                // here and goes over the network as usual.
                assetLoader.shouldInterceptRequest(request.url)

            override fun shouldOverrideUrlLoading(
                view: WebView,
                request: WebResourceRequest
            ): Boolean {
                // Top-level navigations only (fetches never come through
                // here): external links leave for the browser, bundle pages
                // (the third-party licenses page) load in place and the
                // back gesture returns.
                if (request.url.host == WebViewAssetLoader.DEFAULT_DOMAIN) return false
                return try {
                    startActivity(Intent(Intent.ACTION_VIEW, request.url))
                    true
                } catch (e: ActivityNotFoundException) {
                    true
                }
            }
        }

        setContentView(webView)
        webView.loadUrl("https://${WebViewAssetLoader.DEFAULT_DOMAIN}/index.html")
    }

    /**
     * Edge-to-edge: the game draws under the cutout and system bars (the
     * page uses viewport-fit=cover and lays itself out via safe-area
     * insets). The STATUS bar stays visible — transparent (theme), drawn
     * over the game's starfield; only the navigation bar is hidden and
     * reappears on a swipe.
     */
    private fun enterImmersiveMode() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            window.attributes.layoutInDisplayCutoutMode =
                WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            window.setDecorFitsSystemWindows(false)
            window.insetsController?.let {
                it.hide(android.view.WindowInsets.Type.navigationBars())
                it.systemBarsBehavior =
                    android.view.WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
            }
        } else {
            @Suppress("DEPRECATION")
            window.decorView.systemUiVisibility = (
                View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY
                    or View.SYSTEM_UI_FLAG_HIDE_NAVIGATION
                    or View.SYSTEM_UI_FLAG_LAYOUT_STABLE
                    or View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN
                    or View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION
                )
        }
    }

    /**
     * JS bridge behind `window.PegasusApp`: the wake lock rides the View
     * flag (not a WakeLock permission) and only holds while this window is
     * visible, so backgrounding the app always releases it.
     */
    private inner class KeepAwakeBridge {
        @JavascriptInterface
        fun setKeepAwake(on: Boolean) {
            runOnUiThread { webView.keepScreenOn = on }
        }
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus) enterImmersiveMode()
    }

    // The game mirrors its screen stack into session history, so system
    // back = "back one screen in the game UI" — exactly the behavior the
    // site already implements for Android browsers.
    @Deprecated("Deprecated in Java")
    override fun onBackPressed() {
        if (webView.canGoBack()) webView.goBack() else super.onBackPressed()
    }

    override fun onPause() {
        webView.onPause()
        super.onPause()
    }

    override fun onResume() {
        super.onResume()
        webView.onResume()
    }

    /**
     * Serves assets/webroot/ with the same contract as the iOS handler:
     * queries are already stripped (the loader hands us the path only),
     * "/" means index.html, and a missing file is a real 404 — the page
     * probes for optional files (config.json, version.json,
     * whats-new.json) and treats a miss as "feature off". Returning null
     * instead would send the request to the network, where the reserved
     * domain fails DNS and surfaces as a fetch error.
     */
    private class WebRootPathHandler(private val activity: Activity) :
        WebViewAssetLoader.PathHandler {
        override fun handle(path: String): WebResourceResponse {
            val clean = if (path.isEmpty() || path.endsWith("/")) path + "index.html" else path
            return try {
                val stream = activity.assets.open("webroot/$clean")
                WebResourceResponse(mimeFor(clean), null, stream)
            } catch (e: IOException) {
                WebResourceResponse(
                    "text/plain", "utf-8", 404, "Not Found",
                    emptyMap(), ByteArrayInputStream(ByteArray(0))
                )
            }
        }

        private fun mimeFor(path: String): String = when (path.substringAfterLast('.', "")) {
            "html" -> "text/html"
            "js" -> "text/javascript"
            "wasm" -> "application/wasm"
            "json" -> "application/json"
            "png" -> "image/png"
            "svg" -> "image/svg+xml"
            "ico" -> "image/x-icon"
            "css" -> "text/css"
            // .level files, LICENSE (no extension) and anything else texty.
            else -> "text/plain"
        }
    }
}
