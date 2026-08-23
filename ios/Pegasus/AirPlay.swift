import UIKit
import WebKit

// AirPlay second screen: the game stays on the PHONE — completely normal in
// portrait or landscape, every menu touchable, nothing moves — while the TV
// shows a full-screen 16:9 render of the same run, live.
//
// There is no API to START AirPlay programmatically — the user begins
// screen mirroring from Control Center. Once mirroring is active, the
// system offers the mirrored display to the app as a NON-INTERACTIVE
// external-display scene (this happens even with
// UIApplicationSupportsMultipleScenes = false; AppDelegate answers the role
// with ExternalSceneDelegate). Instead of moving anything there, the
// coordinator puts a SECOND WKWebView on the TV window running
// spectator.html — a second instance of the game in spectator mode that
// re-simulates the phone's live recording in lockstep. The sim being a
// deterministic pure function of the quantized input stream (the same
// guarantee behind replays, the racing ghost and backend score
// verification) is what makes the TV's independent 16:9 render
// bit-identical to the phone's run.
//
// The pipe: the phone page drains the wasm's outbound sync frames each rAF
// and posts them here as base64 (pegasusSpecData message, see index.html's
// __pegSpecSetSync block); the coordinator relays each frame into the
// spectator webview (__pegSpecRecv). A 1 Hz poll of __pegSpecNeedFull
// covers resync (spectator joined mid-run or missed frames → the phone
// resends the full recording). Latency on the TV = the relay (~a frame) +
// AirPlay's own ~100–200 ms; the phone, which the player is holding, has
// zero added latency.

final class AirPlayCoordinator {
    static let shared = AirPlayCoordinator()

    private weak var game: GameViewController?
    private var externalWindow: UIWindow?
    private var spectatorView: WKWebView?
    private var resyncTimer: Timer?

    func register(game: GameViewController) {
        self.game = game
        // If mirroring was already active at app launch, the scene
        // connected before the game page existed — start publishing now.
        if externalWindow != nil {
            game.setSpectatorSync(enabled: true)
        }
    }

    func externalConnected(_ scene: UIWindowScene) {
        // One external display: a second connect (rare) replaces the first.
        teardown()

        let config = WKWebViewConfiguration()
        config.setURLSchemeHandler(WebRootSchemeHandler(), forURLScheme: WebRootSchemeHandler.scheme)
        config.allowsInlineMediaPlayback = true
        let web = WKWebView(frame: .zero, configuration: config)
        web.isOpaque = false
        web.backgroundColor = UIColor(red: 5 / 255, green: 6 / 255, blue: 15 / 255, alpha: 1)
        web.scrollView.isScrollEnabled = false
        web.scrollView.bounces = false
        web.load(URLRequest(url: URL(string: "\(WebRootSchemeHandler.scheme)://app/spectator.html")!))

        let vc = UIViewController()
        vc.view.backgroundColor = web.backgroundColor
        web.frame = vc.view.bounds
        web.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        vc.view.addSubview(web)

        let win = UIWindow(windowScene: scene)
        win.rootViewController = vc
        win.isHidden = false
        externalWindow = win
        spectatorView = web

        game?.setSpectatorSync(enabled: true)
        // Resync: the spectator raises a flag when a delta didn't join
        // (joined mid-run, missed frames); poll it and have the phone
        // resend the full recording. 1 Hz is plenty — the flag is rare.
        resyncTimer = Timer.scheduledTimer(withTimeInterval: 1.0, repeats: true) { [weak self] _ in
            self?.pollResync()
        }
    }

    func externalDisconnected() {
        teardown()
        game?.setSpectatorSync(enabled: false)
    }

    /// One base64-encoded buffer of sync frames, phone page → TV page.
    func relay(_ base64: String) {
        // Base64's alphabet is JS-string safe verbatim.
        spectatorView?.evaluateJavaScript(
            "window.__pegSpecRecv&&__pegSpecRecv(\"\(base64)\")",
            completionHandler: nil)
    }

    private func pollResync() {
        spectatorView?.evaluateJavaScript(
            "window.__pegSpecNeedFull?__pegSpecNeedFull():0"
        ) { [weak self] result, _ in
            if (result as? Int ?? 0) != 0 {
                self?.game?.requestSpectatorFullSync()
            }
        }
    }

    private func teardown() {
        resyncTimer?.invalidate()
        resyncTimer = nil
        externalWindow?.isHidden = true
        externalWindow = nil
        spectatorView = nil
    }
}

/// Delegate for the external-display scene (see AppDelegate). Connects when
/// AirPlay mirroring starts while the app is frontmost, disconnects when it
/// stops.
final class ExternalSceneDelegate: UIResponder, UIWindowSceneDelegate {
    func scene(
        _ scene: UIScene,
        willConnectTo session: UISceneSession,
        options connectionOptions: UIScene.ConnectionOptions
    ) {
        guard let windowScene = scene as? UIWindowScene else { return }
        AirPlayCoordinator.shared.externalConnected(windowScene)
    }

    func sceneDidDisconnect(_ scene: UIScene) {
        AirPlayCoordinator.shared.externalDisconnected()
    }
}
