import UIKit

// AirPlay second-screen support: game on the TV, controls on the phone.
//
// There is no API to START AirPlay programmatically — the user begins screen
// mirroring from Control Center. Once mirroring is active, the system offers
// the mirrored display to the foreground app as a NON-INTERACTIVE
// external-display scene (this happens even with
// UIApplicationSupportsMultipleScenes = false; AppDelegate answers the role
// with ExternalSceneDelegate). Putting our own UIWindow on that scene
// replaces the letterboxed mirror image with a true full-screen 16:9 view.
//
// The coordinator below then swaps the app's ONE WKWebView between screens
// (reparenting keeps the web process, wasm state and localStorage — only the
// hosting window changes):
//
//   canvas live (flight / wreck / replay)  → webview on the TV; the phone
//                                            shows PhoneControllerView (dark
//                                            touch surface + ⟳/✕ corner
//                                            buttons).
//   any menu screen up                     → webview back on the phone, so
//                                            every HTML menu/dialog (level
//                                            picker, settings, game-over,
//                                            score submit) stays fully
//                                            usable; the TV shows the idle
//                                            card.
//
// "Canvas live" is the page's own pegasusKeepAwake signal — the wake-lock
// boundary IS the handoff boundary (see GameViewController's message
// handler and the note in index.html's syncWakeLock). Phone touches reach
// the game through the __pegExtTouch JS shim (GameViewController's shell
// bridge script): they are mapped into canvas pixels and fed to the SAME
// wasm touch entry point the browser's real touch events use, so the whole
// in-game TouchStick — floating spawn under the finger, thrust gating,
// input recording — works unchanged, and the stick is drawn ON THE TV under
// the mapped finger, exactly like the replay's ghost stick.

final class AirPlayCoordinator {
    static let shared = AirPlayCoordinator()

    private weak var game: GameViewController?
    private var externalWindow: UIWindow?
    private var externalVC: ExternalDisplayViewController?
    private var canvasLive = false

    func register(game: GameViewController) {
        self.game = game
        sync()
    }

    func externalConnected(_ scene: UIWindowScene) {
        // One external display: a second connect (rare) replaces the first.
        let vc = ExternalDisplayViewController()
        let win = UIWindow(windowScene: scene)
        win.rootViewController = vc
        win.isHidden = false
        externalWindow = win
        externalVC = vc
        sync()
    }

    func externalDisconnected() {
        externalWindow?.isHidden = true
        externalWindow = nil
        externalVC = nil
        sync()
    }

    /// Fed from the page's pegasusKeepAwake message (true = no menu screen
    /// up: flight, the wreck phase, or replay playback).
    func setCanvasLive(_ live: Bool) {
        guard live != canvasLive else { return }
        canvasLive = live
        sync()
    }

    private func sync() {
        guard let game else { return }
        if let ext = externalVC, canvasLive {
            ext.loadViewIfNeeded()
            // Move the page first, then cover the phone — never leave a live
            // canvas under the player's finger while it's being reparented.
            game.hostWebView(in: ext.gameContainer)
            ext.setShowingGame(true)
            game.setControlSurfaceVisible(true)
        } else {
            // Uncover first, then bring the page home (the reverse order).
            game.setControlSurfaceVisible(false)
            game.hostWebView(in: nil)
            externalVC?.setShowingGame(false)
        }
    }
}

/// Delegate for the external-display scene (see AppDelegate). The scene
/// connects when AirPlay mirroring starts while the app is frontmost, and
/// disconnects when mirroring stops.
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

/// Root view controller of the TV window: hosts the webview while the canvas
/// is live, and an idle card (title + "menus are on your phone") while a
/// menu screen has the webview back on the phone.
final class ExternalDisplayViewController: UIViewController {
    let gameContainer = UIView()
    private let idleCard = UIStackView()

    override func viewDidLoad() {
        super.viewDidLoad()
        // The page's own background color, so handoffs don't flash.
        view.backgroundColor = UIColor(red: 5 / 255, green: 6 / 255, blue: 15 / 255, alpha: 1)

        gameContainer.frame = view.bounds
        gameContainer.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        view.addSubview(gameContainer)

        let title = UILabel()
        title.text = "PEGASUS"
        title.font = .monospacedSystemFont(ofSize: 96, weight: .black)
        title.textColor = UIColor(red: 0.43, green: 0.94, blue: 1.0, alpha: 1) // menu cyan
        title.layer.shadowColor = title.textColor.cgColor
        title.layer.shadowRadius = 18
        title.layer.shadowOpacity = 0.8
        title.layer.shadowOffset = .zero

        let hint = UILabel()
        hint.text = "flight controls & menus are on your phone"
        hint.font = .monospacedSystemFont(ofSize: 24, weight: .medium)
        hint.textColor = UIColor(white: 1, alpha: 0.45)

        idleCard.axis = .vertical
        idleCard.alignment = .center
        idleCard.spacing = 24
        idleCard.addArrangedSubview(title)
        idleCard.addArrangedSubview(hint)
        idleCard.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(idleCard)
        NSLayoutConstraint.activate([
            idleCard.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            idleCard.centerYAnchor.constraint(equalTo: view.centerYAnchor),
        ])
    }

    func setShowingGame(_ showing: Bool) {
        loadViewIfNeeded()
        idleCard.isHidden = showing
    }
}

/// The phone-side controller shown while the game is on the TV: a full-screen
/// touch surface (every touch is forwarded to the game — the canvas-anywhere
/// floating-stick behavior carries over 1:1), the two corner buttons the HTML
/// hud-btns provide in normal play, and a faint ring/knob echo under the
/// finger for tactile confidence (the real stick is drawn on the TV).
final class PhoneControllerView: UIView {
    /// (sappPhase, touchId, location in view points, view size in points)
    var onTouch: ((Int, Int, CGPoint, CGSize) -> Void)?
    /// "menu" (✕) or "restart" (⟳) — routed to __pegCorner, which clicks
    /// whichever HTML corner button is currently live (pause in flight,
    /// exit-replay in a replay).
    var onCorner: ((String) -> Void)?

    // SAPP touch phases — the values mq_js_bundle.js passes to the wasm
    // touch entry (SAPP_EVENTTYPE_TOUCHES_*).
    private static let began = 10, moved = 11, ended = 12, cancelled = 13

    // Ids must never collide with the browser's own touch identifiers
    // (WebKit starts at 0) — real canvas touches can't happen while the
    // webview is on the TV, but transitions overlap; start far above.
    private var touchIds: [ObjectIdentifier: Int] = [:]
    private var nextTouchId = 1001

    private let ring = CAShapeLayer()
    private let knob = CAShapeLayer()
    private var ringTouch: ObjectIdentifier?
    private var ringCenter = CGPoint.zero

    override init(frame: CGRect) {
        super.init(frame: frame)
        isMultipleTouchEnabled = true
        backgroundColor = UIColor(red: 5 / 255, green: 6 / 255, blue: 15 / 255, alpha: 1)

        // Touch echo: amber like the in-game stick's held state.
        let amber = UIColor(red: 1.0, green: 0.71, blue: 0.33, alpha: 1)
        ring.fillColor = nil
        ring.strokeColor = amber.withAlphaComponent(0.35).cgColor
        ring.lineWidth = 2
        ring.isHidden = true
        layer.addSublayer(ring)
        knob.fillColor = amber.withAlphaComponent(0.45).cgColor
        knob.isHidden = true
        layer.addSublayer(knob)

        let onTv = makeCaption("GAME ON TV", size: 13, alpha: 0.5)
        addSubview(onTv)
        let hint = makeCaption("touch anywhere · hold to thrust · drag to steer", size: 11, alpha: 0.3)
        addSubview(hint)

        let menuBtn = makeCornerButton(systemName: "xmark")
        menuBtn.addTarget(self, action: #selector(tapMenu), for: .touchUpInside)
        addSubview(menuBtn)
        let restartBtn = makeCornerButton(systemName: "arrow.counterclockwise")
        restartBtn.addTarget(self, action: #selector(tapRestart), for: .touchUpInside)
        addSubview(restartBtn)

        // Same corner as the HTML hud-btns (top-right, ✕ outermost).
        NSLayoutConstraint.activate([
            menuBtn.topAnchor.constraint(equalTo: safeAreaLayoutGuide.topAnchor, constant: 10),
            menuBtn.trailingAnchor.constraint(equalTo: safeAreaLayoutGuide.trailingAnchor, constant: -12),
            restartBtn.topAnchor.constraint(equalTo: menuBtn.topAnchor),
            restartBtn.trailingAnchor.constraint(equalTo: menuBtn.leadingAnchor, constant: -12),
            onTv.topAnchor.constraint(equalTo: safeAreaLayoutGuide.topAnchor, constant: 22),
            onTv.centerXAnchor.constraint(equalTo: centerXAnchor),
            hint.bottomAnchor.constraint(equalTo: safeAreaLayoutGuide.bottomAnchor, constant: -14),
            hint.centerXAnchor.constraint(equalTo: centerXAnchor),
        ])
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    private func makeCaption(_ text: String, size: CGFloat, alpha: CGFloat) -> UILabel {
        let l = UILabel()
        l.text = text
        l.font = .monospacedSystemFont(ofSize: size, weight: .semibold)
        l.textColor = UIColor(white: 1, alpha: alpha)
        l.translatesAutoresizingMaskIntoConstraints = false
        return l
    }

    private func makeCornerButton(systemName: String) -> UIButton {
        // Mirrors the HTML .cbtn look: translucent dark circle, cyan glyph.
        let b = UIButton(type: .system)
        let cfg = UIImage.SymbolConfiguration(pointSize: 17, weight: .bold)
        b.setImage(UIImage(systemName: systemName, withConfiguration: cfg), for: .normal)
        b.tintColor = UIColor(red: 0.43, green: 0.94, blue: 1.0, alpha: 1)
        b.backgroundColor = UIColor(white: 0.08, alpha: 0.8)
        b.layer.cornerRadius = 22
        b.layer.borderWidth = 1
        b.layer.borderColor = UIColor(red: 0.43, green: 0.94, blue: 1.0, alpha: 0.35).cgColor
        b.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            b.widthAnchor.constraint(equalToConstant: 44),
            b.heightAnchor.constraint(equalToConstant: 44),
        ])
        return b
    }

    @objc private func tapMenu() { onCorner?("menu") }
    @objc private func tapRestart() { onCorner?("restart") }

    // MARK: touch forwarding

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
        forward(Self.began, touches)
    }
    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent?) {
        forward(Self.moved, touches)
    }
    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent?) {
        forward(Self.ended, touches)
    }
    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent?) {
        // Also fires when the surface is removed mid-press (menu handoff) —
        // forwarding it releases the in-game stick instead of wedging it.
        forward(Self.cancelled, touches)
    }

    private func forward(_ phase: Int, _ touches: Set<UITouch>) {
        for t in touches {
            let key = ObjectIdentifier(t)
            let id: Int
            if phase == Self.began {
                id = nextTouchId
                nextTouchId += 1
                touchIds[key] = id
            } else {
                guard let known = touchIds[key] else { continue }
                id = known
            }
            if phase == Self.ended || phase == Self.cancelled {
                touchIds.removeValue(forKey: key)
            }
            let p = t.location(in: self)
            onTouch?(phase, id, p, bounds.size)
            updateEcho(phase, key, p)
        }
    }

    // MARK: touch echo (ring at touch-down, knob under the finger)

    private func updateEcho(_ phase: Int, _ key: ObjectIdentifier, _ p: CGPoint) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        defer { CATransaction.commit() }
        if phase == Self.began && ringTouch == nil {
            ringTouch = key
            ringCenter = p
            ring.path = UIBezierPath(
                arcCenter: p, radius: 64, startAngle: 0, endAngle: 2 * .pi, clockwise: true
            ).cgPath
            ring.isHidden = false
            knob.isHidden = false
        }
        guard key == ringTouch else { return }
        if phase == Self.ended || phase == Self.cancelled {
            ringTouch = nil
            ring.isHidden = true
            knob.isHidden = true
            return
        }
        knob.path = UIBezierPath(
            arcCenter: p, radius: 14, startAngle: 0, endAngle: 2 * .pi, clockwise: true
        ).cgPath
    }
}
