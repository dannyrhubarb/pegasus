import AVFoundation
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
    private var appBuildScript: WKUserScript!
    private var didStartLoad = false

    // The status bar stays visible, drawn over the game's starfield (the
    // page lays its HUD out below env(safe-area-inset-top), so nothing
    // hides behind it); light content for the dark background.
    override var preferredStatusBarStyle: UIStatusBarStyle { .lightContent }
    override var prefersHomeIndicatorAutoHidden: Bool { true }
    // First bottom swipe shows the home indicator, second leaves the app —
    // keeps an accidental swipe during a low pass from killing the run.
    override var preferredScreenEdgesDeferringSystemGestures: UIRectEdge { [.bottom] }

    /// Query string of the Universal Link that launched the app (nil for a
    /// plain launch); appended to the bundled index.html load. Set by
    /// SceneDelegate before the view loads.
    var launchQuery: String?

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black

        let config = WKWebViewConfiguration()
        config.setURLSchemeHandler(WebRootSchemeHandler(), forURLScheme: WebRootSchemeHandler.scheme)
        config.allowsInlineMediaPlayback = true
        config.mediaTypesRequiringUserActionForPlayback = []
        // The page's syncWakeLock posts here while the canvas is live
        // (flying / watching a replay) so the screen stays on through a
        // hands-off glide; any menu screen posts false and the idle timer
        // resumes. (The controller lives for the whole app lifetime, so the
        // handler's strong reference to it is harmless.)
        config.userContentController.add(self, name: "pegasusKeepAwake")
        // Surface the INSTALLED app's version to the page for the About
        // screen's "App build" row: "1.0 (42)" — CFBundleShortVersionString
        // + CFBundleVersion (CI stamps the latter with the workflow run
        // number). Injected before the page's scripts run so the About
        // code can read it synchronously.
        let info = Bundle.main.infoDictionary
        let version = info?["CFBundleShortVersionString"] as? String ?? "?"
        let build = info?["CFBundleVersion"] as? String ?? "?"
        appBuildScript = WKUserScript(
            source: "window.__pegAppBuild = \"\(version) (\(build))\"",
            injectionTime: .atDocumentStart,
            forMainFrameOnly: true
        )
        config.userContentController.addUserScript(appBuildScript)

        webView = WKWebView(frame: view.bounds, configuration: config)
        // Fill the WHOLE screen, not the safe area: the page uses
        // viewport-fit=cover and reads env(safe-area-inset-*) itself (the
        // in-game HUD and menu padding depend on the real notch insets).
        webView.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        webView.navigationDelegate = self
        webView.uiDelegate = self
        webView.isOpaque = false
        webView.backgroundColor = UIColor(red: 5 / 255, green: 6 / 255, blue: 15 / 255, alpha: 1)
        // Scrolling is toggled per page in didCommit (syncScrollLock): OFF
        // for the game (a scrollable canvas would fight the touch stick),
        // ON for the bundled document pages (third-party licenses, LICENSE,
        // privacy) that load in place — with it left off they rendered as a
        // frozen first screen. Inset adjustment stays .never everywhere;
        // the pages pad themselves via safe-area insets.
        webView.scrollView.isScrollEnabled = false
        webView.scrollView.bounces = false
        webView.scrollView.contentInsetAdjustmentBehavior = .never
        // The game's UI mirrors its screen stack into session history and
        // supports the iOS edge-swipe as "back one screen" — keep that.
        webView.allowsBackForwardNavigationGestures = true
        view.addSubview(webView)
        // load() happens in viewDidLayoutSubviews, once the safe-area
        // insets are known — see pushSafeAreaInsets.
    }

    // env(safe-area-inset-*) reaches the web content only AFTER its first
    // paint (WKWebView propagates the insets asynchronously, a couple of
    // frames later), so the menu briefly painted with its fallback padding
    // and then visibly jumped down ~31 pt during launch. The shell knows
    // the real insets before the page even exists, so it injects them as
    // the --app-inset-* CSS variables (index.html folds them in via
    // max(env(...), var(--app-inset-*, 0px)) — the plain website, where the
    // vars stay unset, is untouched). The first load is deferred to the
    // first layout pass because view.safeAreaInsets is still zero in
    // viewDidLoad (the view isn't in a window yet).
    private func pushSafeAreaInsets() {
        let i = view.safeAreaInsets
        let js = """
        (function (s) {
          s.setProperty("--app-inset-top", "\(Int(i.top.rounded()))px");
          s.setProperty("--app-inset-right", "\(Int(i.right.rounded()))px");
          s.setProperty("--app-inset-bottom", "\(Int(i.bottom.rounded()))px");
          s.setProperty("--app-inset-left", "\(Int(i.left.rounded()))px");
        })(document.documentElement.style);
        """
        // Re-register the document-start scripts so any future navigation
        // (the bundled licenses page and back) also boots with the CURRENT
        // insets, not the launch-time ones…
        let ucc = webView.configuration.userContentController
        ucc.removeAllUserScripts()
        ucc.addUserScript(appBuildScript)
        ucc.addUserScript(WKUserScript(
            source: js, injectionTime: .atDocumentStart, forMainFrameOnly: true))
        // …and update the live page directly (rotation happens mid-session).
        if didStartLoad {
            webView.evaluateJavaScript(js, completionHandler: nil)
        }
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        guard !didStartLoad else { return }
        pushSafeAreaInsets()
        didStartLoad = true
        var page = "\(WebRootSchemeHandler.scheme)://app/index.html"
        if let q = launchQuery { page += "?" + q }
        // WebRootSchemeHandler strips the query when resolving the file, so a
        // forwarded query can't miss the bundle; a malformed one just falls
        // back to the plain page.
        let url = URL(string: page) ?? URL(string: "\(WebRootSchemeHandler.scheme)://app/index.html")!
        webView.load(URLRequest(url: url))
    }

    // Keep the injected values current (rotation changes which edges carry
    // the notch inset) — a stale portrait value would win the max() in
    // landscape and push the menu down for no reason.
    override func viewSafeAreaInsetsDidChange() {
        super.viewSafeAreaInsetsDidChange()
        if didStartLoad { pushSafeAreaInsets() }
    }

    // The game page must not scroll (the canvas IS the touch surface), but
    // the bundled document pages that navigate in place — the third-party
    // licenses page, the LICENSE text it links, privacy.html — are ordinary
    // long documents and need the scroll view live. Keyed on the committed
    // URL so the swipe-back to the game re-freezes it.
    private func syncScrollLock() {
        let isGame = webView.url?.path.hasSuffix("index.html") ?? true
        webView.scrollView.isScrollEnabled = !isGame
        webView.scrollView.bounces = !isGame
    }

    func webView(_ webView: WKWebView, didCommit navigation: WKNavigation!) {
        syncScrollLock()
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

// Camera/microphone permission for getUserMedia (iOS 15+). Without this
// delegate WebKit shows its OWN per-origin sheet ("pegasus://app" would
// like to access the camera) on top of the iOS system prompt — two
// dialogs, seen live on the first #200 camera probe (2026-09). Answer from
// the system authorization instead: iOS asks once per media type (the
// NS*UsageDescription prompts; instant once decided), and WebKit is
// granted or denied accordingly, so the player sees one dialog naming
// Pegasus per media type.
extension GameViewController {
    func webView(
        _ webView: WKWebView,
        requestMediaCapturePermissionFor origin: WKSecurityOrigin,
        initiatedByFrame frame: WKFrameInfo,
        type: WKMediaCaptureType,
        decisionHandler: @escaping (WKPermissionDecision) -> Void
    ) {
        // Only the bundled page (our own scheme) may capture; anything else
        // that ever loads in here gets WebKit's default prompt.
        guard origin.protocol == WebRootSchemeHandler.scheme else {
            return decisionHandler(.prompt)
        }
        // Both usage strings live in Info.plist (requesting access without
        // the matching key is a crash, not a denial — keep them paired).
        let media: [AVMediaType]
        switch type {
        case .camera: media = [.video]
        case .microphone: media = [.audio]
        case .cameraAndMicrophone: media = [.video, .audio]
        @unknown default: return decisionHandler(.prompt)
        }
        requestAccess(media) { granted in
            DispatchQueue.main.async { decisionHandler(granted ? .grant : .deny) }
        }
    }

    /// Sequential system authorization for each media type; false as soon
    /// as one is refused. iOS shows one prompt per type, once per install.
    private func requestAccess(_ media: [AVMediaType], _ completion: @escaping (Bool) -> Void) {
        guard let first = media.first else { return completion(true) }
        AVCaptureDevice.requestAccess(for: first) { granted in
            guard granted else { return completion(false) }
            self.requestAccess(Array(media.dropFirst()), completion)
        }
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
