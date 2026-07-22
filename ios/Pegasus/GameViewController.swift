import UIKit
import WebKit

/// Full-screen WKWebView hosting the bundled web build. The page is served
/// from the app bundle's WebRoot/ folder through WebRootSchemeHandler because
/// fetch() does not work on file:// URLs — the game fetches its wasm, level
/// files, manifest and config at runtime, so it needs a real URL-scheme
/// origin. The custom scheme is also a stable origin for localStorage
/// (settings, pilot name, board cache).
final class GameViewController: UIViewController, WKNavigationDelegate, WKUIDelegate {
    private var webView: WKWebView!

    override var prefersStatusBarHidden: Bool { true }
    override var prefersHomeIndicatorAutoHidden: Bool { true }
    // First bottom swipe shows the home indicator, second leaves the app —
    // keeps an accidental swipe during a low pass from killing the run.
    override var preferredScreenEdgesDeferringSystemGestures: UIRectEdge { [.bottom] }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black

        let config = WKWebViewConfiguration()
        config.setURLSchemeHandler(WebRootSchemeHandler(), forURLScheme: WebRootSchemeHandler.scheme)
        config.allowsInlineMediaPlayback = true
        config.mediaTypesRequiringUserActionForPlayback = []
        // The page's syncNativeWake posts here while the canvas is live
        // (flying / watching a replay) so the screen stays on through a
        // hands-off glide; any menu screen posts false and the idle timer
        // resumes. (The controller lives for the whole app lifetime, so the
        // handler's strong reference to it is harmless.)
        config.userContentController.add(self, name: "pegasusKeepAwake")

        webView = WKWebView(frame: view.bounds, configuration: config)
        // Fill the WHOLE screen, not the safe area: the page uses
        // viewport-fit=cover and reads env(safe-area-inset-*) itself (the
        // in-game HUD and menu padding depend on the real notch insets).
        webView.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        webView.navigationDelegate = self
        webView.uiDelegate = self
        webView.isOpaque = false
        webView.backgroundColor = UIColor(red: 5 / 255, green: 6 / 255, blue: 15 / 255, alpha: 1)
        webView.scrollView.isScrollEnabled = false
        webView.scrollView.bounces = false
        webView.scrollView.contentInsetAdjustmentBehavior = .never
        // The game's UI mirrors its screen stack into session history and
        // supports the iOS edge-swipe as "back one screen" — keep that.
        webView.allowsBackForwardNavigationGestures = true
        view.addSubview(webView)

        webView.load(URLRequest(url: URL(string: "\(WebRootSchemeHandler.scheme)://app/index.html")!))
    }

    // http/https navigations (external links) leave the app for Safari;
    // everything on the bundle scheme stays in the webview.
    func webView(
        _ webView: WKWebView,
        decidePolicyFor navigationAction: WKNavigationAction,
        decisionHandler: @escaping (WKNavigationActionPolicy) -> Void
    ) {
        if let url = navigationAction.request.url,
           let scheme = url.scheme?.lowercased(),
           scheme == "http" || scheme == "https" {
            UIApplication.shared.open(url)
            decisionHandler(.cancel)
            return
        }
        decisionHandler(.allow)
    }

    // target="_blank": there are no extra windows in an app — bundled pages
    // (the third-party licenses page) load in place with swipe-back to
    // return, external URLs go to Safari.
    func webView(
        _ webView: WKWebView,
        createWebViewWith configuration: WKWebViewConfiguration,
        for navigationAction: WKNavigationAction,
        windowFeatures: WKWindowFeatures
    ) -> WKWebView? {
        if let url = navigationAction.request.url {
            if url.scheme == WebRootSchemeHandler.scheme {
                webView.load(navigationAction.request)
            } else {
                UIApplication.shared.open(url)
            }
        }
        return nil
    }
}

extension GameViewController: WKScriptMessageHandler {
    func userContentController(
        _ userContentController: WKUserContentController,
        didReceive message: WKScriptMessage
    ) {
        if message.name == "pegasusKeepAwake" {
            UIApplication.shared.isIdleTimerDisabled = (message.body as? Bool) ?? false
        }
    }
}
