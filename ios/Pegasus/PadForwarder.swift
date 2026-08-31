import GameController
import WebKit

/// Native Bluetooth/USB game-controller bridge.
///
/// The page's own Web Gamepad API poll works in a plain browser, but WebKit
/// exposes gamepads only to a page it considers visible, focused and
/// recently interacted with — gating that is brittle inside an app shell,
/// especially with AirPlay mirroring in play. The shell therefore reads the
/// controller natively (GCController has no such gating) and forwards to
/// the SAME wasm exports the web poll uses; the `__pegNativePad` flag
/// injected by GameViewController makes the page's poll stand down so the
/// two paths never double-drive.
///
/// Mapping (matches index.html's web poll) — mirrors the split on-screen
/// scheme: left hand burns, right thumb steers:
///   throttle: L2 trigger, ANALOG (travel = burn); L1 and D-pad up are
///             digital full burn                        → set_pad_throttle
///   heading : RIGHT stick, both axes                   → set_pad_stick
///             (commanded nose direction through the touch stick's PD;
///             GCController's y is up-positive, the game wants screen
///             convention up = −y, so y is negated here)
///   reset   : Menu or Y, edge-triggered                → set_pad_reset
/// No pad rate-rotation (set_pad_torque is never driven): D-pad left/right
/// was a keyboard-legacy override of the heading PD, dropped as redundant
/// next to the stick.
final class PadForwarder: NSObject {
    private weak var webView: WKWebView?
    private var link: CADisplayLink?
    private var lastThrottle: Float = -1
    private var lastStickX: Float = 0
    private var lastStickY: Float = 0
    private var lastReset = false

    init(webView: WKWebView) {
        self.webView = webView
        super.init()
        let nc = NotificationCenter.default
        nc.addObserver(
            self, selector: #selector(padsChanged), name: .GCControllerDidConnect, object: nil)
        nc.addObserver(
            self, selector: #selector(padsChanged), name: .GCControllerDidDisconnect, object: nil)
        padsChanged()
    }

    @objc private func padsChanged() {
        let hasPad = GCController.controllers().contains { $0.extendedGamepad != nil }
        if hasPad && link == nil {
            // 60 Hz poll with change-dedup below — same cadence as the web
            // poll, bounded JS traffic. (The link retains self; both live
            // for the app lifetime alongside the game view controller.)
            let l = CADisplayLink(target: self, selector: #selector(tick))
            l.add(to: .main, forMode: .common)
            link = l
        } else if !hasPad, let l = link {
            l.invalidate()
            link = nil
            // Release anything held so a mid-burn disconnect doesn't wedge
            // the engine on.
            send("E.set_pad_throttle(0);E.set_pad_stick(0,0);")
            lastThrottle = -1
            lastStickX = 0
            lastStickY = 0
            lastReset = false
        }
    }

    @objc private func tick() {
        guard let pad = GCController.controllers().compactMap({ $0.extendedGamepad }).first
        else { return }
        // L1 / D-pad up: digital full burn. Analog throttle is the MAX of
        // the L2 trigger's travel and the (otherwise unused) LEFT stick
        // pushed up, HOTAS-style — some pads' triggers are digital click
        // switches behind an analog-shaped API (Switch-style ZL jumps
        // 0 → 1), while a stick axis is genuinely analog on every
        // controller. Stick dead-zone 0.15, rescaled; the game applies the
        // expo curve on top of whichever source wins.
        let ly = pad.leftThumbstick.yAxis.value // GCController: up-positive
        let stickThrottle = max(0, (ly - 0.15) / 0.85)
        let throttle: Float =
            (pad.leftShoulder.isPressed || pad.dpad.up.isPressed)
            ? 1.0 : max(pad.leftTrigger.value, stickThrottle)
        let sx = pad.rightThumbstick.xAxis.value
        let sy = -pad.rightThumbstick.yAxis.value
        let reset = pad.buttonMenu.isPressed || pad.buttonY.isPressed

        var js = ""
        if abs(throttle - lastThrottle) > 0.004 {
            js += "E.set_pad_throttle(\(throttle));"
            lastThrottle = throttle
        }
        if abs(sx - lastStickX) > 0.004 || abs(sy - lastStickY) > 0.004 {
            js += "E.set_pad_stick(\(sx),\(sy));"
            lastStickX = sx
            lastStickY = sy
        }
        if reset && !lastReset {
            js += "E.set_pad_reset();"
        }
        lastReset = reset
        if !js.isEmpty { send(js) }
    }

    private func send(_ body: String) {
        // set_pad_throttle is the newest of the exports — its presence
        // means they all exist. Everything try/caught: pad input must
        // never break the page.
        webView?.evaluateJavaScript(
            "try{var E=wasm_exports;if(typeof E!==\"undefined\"&&E.set_pad_throttle){\(body)}}catch(e){}",
            completionHandler: nil)
    }
}
