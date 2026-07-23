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
        // The page's shell bridge (wake lock + app version — see
        // PegasusBridge). Only bundled content runs in this WebView
        // (external links leave for the browser), so exposing the
        // interface is safe.
        // Belt-and-braces against gesture claiming: never let the WebView's
        // long-press (text selection / drag) or overscroll machinery steal
        // an in-progress touch from the game.
        webView.overScrollMode = View.OVER_SCROLL_NEVER
        webView.isLongClickable = false
        webView.setOnLongClickListener { true }
        webView.isHapticFeedbackEnabled = false
        webView.addJavascriptInterface(PegasusBridge(), "PegasusApp")
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
        // AFTER setContentView: window.insetsController needs the DecorView,
        // which doesn't exist yet at the top of onCreate — accessing it there
        // NPEs inside PhoneWindow on Android 11+ (crash-at-boot, found by the
        // emulator smoke test; the pre-11 fallback path masked it by creating
        // the decor as a side effect).
        enterEdgeToEdge()
        webView.loadUrl("https://${WebViewAssetLoader.DEFAULT_DOMAIN}/index.html")
    }

    /**
     * Edge-to-edge, NO hidden bars: the game draws under the cutout and the
     * transparent status/navigation bars (the page uses viewport-fit=cover
     * and lays itself out via safe-area insets) — the same configuration as
     * the game running in Chrome, which is the proven-good touch baseline.
     *
     * Do NOT hide the navigation bar with
     * BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE: any upward swipe near the
     * bottom (= where the touch stick lives) transiently reveals the bars,
     * and while they are showing the NEXT touch is consumed to dismiss
     * them — in the field that read as "only every few touches goes
     * through" (tester report, 2026-07).
     */
    private fun enterEdgeToEdge() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            window.attributes.layoutInDisplayCutoutMode =
                WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            window.setDecorFitsSystemWindows(false)
        } else {
            @Suppress("DEPRECATION")
            window.decorView.systemUiVisibility = (
                View.SYSTEM_UI_FLAG_LAYOUT_STABLE
                    or View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN
                    or View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION
                )
        }
    }

    /**
     * JS bridge behind `window.PegasusApp`.
     *
     * setKeepAwake: the page's syncWakeLock holds the screen on while the
     * canvas is live (flying / watching a replay) — a View flag (not a
     * WakeLock permission) that only holds while this window is visible,
     * so backgrounding the app always releases it.
     *
     * appBuild: the INSTALLED app's version for the About screen's "App
     * build" row — "1.0 (42)", versionName + the versionCode CI stamps
     * with the workflow run number.
     */
    private inner class PegasusBridge {
        @JavascriptInterface
        fun setKeepAwake(on: Boolean) {
            runOnUiThread { webView.keepScreenOn = on }
        }

        @JavascriptInterface
        fun appBuild(): String = try {
            val info = packageManager.getPackageInfo(packageName, 0)
            val code = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                info.longVersionCode
            } else {
                @Suppress("DEPRECATION")
                info.versionCode.toLong()
            }
            "${info.versionName} ($code)"
        } catch (e: Exception) {
            ""
        }
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        // Re-assert the legacy layout flags on < R (the system can clear
        // them); on R+ the decor-fits setting is sticky.
        if (hasFocus) enterEdgeToEdge()
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
