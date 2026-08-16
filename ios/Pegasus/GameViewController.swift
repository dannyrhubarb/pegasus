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
    private var shellBridgeScript: WKUserScript!
    private var didStartLoad = false
    private var controllerView: PhoneControllerView?

    // The status bar stays visible, drawn over the game's starfield (the
    // page lays its HUD out below env(safe-area-inset-top), so nothing
    // hides behind it); light content for the dark background.
    override var preferredStatusBarStyle: UIStatusBarStyle { .lightContent }
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
        // Shell bridge for AirPlay second-screen play (see AirPlay.swift).
        // __pegExtTouch maps a phone-surface touch into canvas pixels —
        // uniform scale, letterbox-fit, centered, so circular stick motion
        // stays circular — and feeds the SAME wasm touch entry the browser's
        // real touch events go through (mq_js_bundle.js calls
        // wasm_exports.touch(type, id, x, y) in physical canvas px), so the
        // in-game TouchStick machinery works unchanged. __pegCorner clicks
        // whichever HTML corner button is currently live: restart, or for ✕
        // the replay-exit button when visible, else pause — the page keeps
        // owning the context sensitivity. Both are inert unless called and
        // fully try/caught: the plain website never sees them, and a failure
        // must never break the game.
        shellBridgeScript = WKUserScript(
            source: """
            window.__pegExtTouch = function (ph, id, x, y, w, h) {
              try {
                var c = document.getElementById("glcanvas");
                if (!c || typeof wasm_exports === "undefined" || !wasm_exports || !wasm_exports.touch) return;
                var r = c.getBoundingClientRect();
                var dp = window.devicePixelRatio || 1;
                var W = r.width * dp, H = r.height * dp;
                var s = Math.min(W / w, H / h);
                wasm_exports.touch(ph, id, (W - w * s) / 2 + x * s, (H - h * s) / 2 + y * s);
              } catch (e) {}
            };
            window.__pegCorner = function (which) {
              try {
                var vis = function (el) { return el && el.getClientRects().length > 0; };
                var el;
                if (which === "restart") {
                  el = document.getElementById("restart-btn");
                } else {
                  el = document.getElementById("exit-replay-btn");
                  if (!vis(el)) el = document.getElementById("pause-btn");
                }
                if (vis(el)) el.click();
              } catch (e) {}
            };
            """,
            injectionTime: .atDocumentStart,
            forMainFrameOnly: true
        )
        config.userContentController.addUserScript(shellBridgeScript)

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
    // The insets are read from the webview's CURRENT host: this view on the
    // phone, or the external display's container while the game is on the TV
    // (whose insets are zero — a TV has no notch, and pushing the phone's
    // values there would shove the HUD around for no reason).
    private func pushSafeAreaInsets() {
        let i = (webView.superview ?? view).safeAreaInsets
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
        ucc.addUserScript(shellBridgeScript)
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
        webView.load(URLRequest(url: URL(string: "\(WebRootSchemeHandler.scheme)://app/index.html")!))
        // Register AFTER the first load kicks off: if AirPlay is already
        // mirroring at launch, the coordinator's sync runs against a webview
        // that exists and is loading.
        AirPlayCoordinator.shared.register(game: self)
    }

    // MARK: AirPlay second-screen handoff (driven by AirPlayCoordinator)

    /// Reparent the webview: `nil` = home (this view, under the controller
    /// overlay), else the external display's container. The SAME WKWebView
    /// moves — web process, wasm state and localStorage all survive; only
    /// the hosting window changes.
    func hostWebView(in container: UIView?) {
        // Explicit type: `view` is UIView! and `container ?? view` would
        // otherwise infer UIView?.
        let target: UIView = container ?? view
        if webView.superview !== target {
            webView.frame = target.bounds
            if target === view {
                view.insertSubview(webView, at: 0)
            } else {
                target.addSubview(webView)
            }
        }
        pushSafeAreaInsets() // the new host's insets (external display: zero)
    }

    /// Show/hide the phone-side controller (dark touch surface + corner
    /// buttons) that covers this view while the game renders on the TV.
    func setControlSurfaceVisible(_ on: Bool) {
        if on {
            if controllerView == nil {
                let cv = PhoneControllerView(frame: view.bounds)
                cv.autoresizingMask = [.flexibleWidth, .flexibleHeight]
                cv.onTouch = { [weak self] phase, id, p, size in
                    // One short JS call per UIKit touch callback — WebKit
                    // handles this rate (~60–120 Hz per finger) comfortably.
                    // Whole points are plenty: 1 pt ≈ 1.6 % of the stick's
                    // 60-logical-px full travel after the TV mapping.
                    self?.webView.evaluateJavaScript(
                        "window.__pegExtTouch&&__pegExtTouch(\(phase),\(id),"
                            + "\(Int(p.x.rounded())),\(Int(p.y.rounded())),"
                            + "\(Int(size.width.rounded())),\(Int(size.height.rounded())))",
                        completionHandler: nil)
                }
                cv.onCorner = { [weak self] which in
                    self?.webView.evaluateJavaScript(
                        "window.__pegCorner&&__pegCorner(\"\(which)\")",
                        completionHandler: nil)
                }
                controllerView = cv
            }
            controllerView!.frame = view.bounds
            view.addSubview(controllerView!)
        } else {
            // Removing the surface mid-press fires touchesCancelled, which
            // the surface forwards — the in-game stick releases cleanly.
            controllerView?.removeFromSuperview()
        }
    }

    // Keep the injected values current (rotation changes which edges carry
    // the notch inset) — a stale portrait value would win the max() in
    // landscape and push the menu down for no reason.
    override func viewSafeAreaInsetsDidChange() {
        super.viewSafeAreaInsetsDidChange()
        if didStartLoad { pushSafeAreaInsets() }
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
            let on = (message.body as? Bool) ?? false
            UIApplication.shared.isIdleTimerDisabled = on
            // The wake-lock boundary is exactly "the canvas is live" (no
            // menu screen up: flight, wreck phase, replay) — the AirPlay
            // handoff keys off the same signal (see AirPlay.swift).
            AirPlayCoordinator.shared.setCanvasLive(on)
        }
    }
}
