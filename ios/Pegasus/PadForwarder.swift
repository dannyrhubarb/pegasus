import GameController
import WebKit

/// Native Bluetooth/USB game-controller bridge.
///
/// The page's own Web Gamepad API poll works in a normal webview, but WebKit
/// only exposes gamepads to a page that is VISIBLE AND FOCUSED — and during
/// AirPlay second-screen play the webview lives in the non-interactive TV
/// window, which is never the key window, so navigator.getGamepads() goes
/// quiet exactly when playing on the TV is the point. The shell therefore
/// reads the controller natively (GCController is immune to webview focus)
/// and forwards to the SAME wasm exports the web poll uses; the
/// `__pegNativePad` flag in GameViewController's shell bridge script makes
/// the page's poll stand down so the two paths never double-drive.
///
/// Mapping (matches index.html's web poll):
///   thrust  : A, right trigger > 0.3, or D-pad up      → set_pad_thrust
///   heading : left stick, both axes                    → set_pad_stick
///             (commanded nose direction through the touch stick's PD;
///             GCController's y is up-positive, the game wants screen
///             convention up = −y, so y is negated here)
///   rate    : D-pad left/right (overrides the PD while held)
///                                                      → set_pad_torque
///   reset   : Menu or Y, edge-triggered                → set_pad_reset
final class PadForwarder: NSObject {
    private weak var webView: WKWebView?
    private var link: CADisplayLink?
    private var lastThrust = -1
    private var lastStickX: Float = 0
    private var lastStickY: Float = 0
    private var lastRate: Float = 0
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
            send("E.set_pad_thrust(0);E.set_pad_torque(0);E.set_pad_stick(0,0);")
            lastThrust = -1
            lastStickX = 0
            lastStickY = 0
            lastRate = 0
            lastReset = false
        }
    }

    @objc private func tick() {
        guard let pad = GCController.controllers().compactMap({ $0.extendedGamepad }).first
        else { return }
        let thrust =
            (pad.buttonA.isPressed || pad.rightTrigger.value > 0.3 || pad.dpad.up.isPressed)
            ? 1 : 0
        let sx = pad.leftThumbstick.xAxis.value
        let sy = -pad.leftThumbstick.yAxis.value
        var rate: Float = 0
        if pad.dpad.left.isPressed { rate = -1 } else if pad.dpad.right.isPressed { rate = 1 }
        let reset = pad.buttonMenu.isPressed || pad.buttonY.isPressed

        var js = ""
        if thrust != lastThrust {
            js += "E.set_pad_thrust(\(thrust));"
            lastThrust = thrust
        }
        if abs(sx - lastStickX) > 0.004 || abs(sy - lastStickY) > 0.004 {
            js += "E.set_pad_stick(\(sx),\(sy));"
            lastStickX = sx
            lastStickY = sy
        }
        if rate != lastRate {
            js += "E.set_pad_torque(\(rate));"
            lastRate = rate
        }
        if reset && !lastReset {
            js += "E.set_pad_reset();"
        }
        lastReset = reset
        if !js.isEmpty { send(js) }
    }

    private func send(_ body: String) {
        // set_pad_stick is the newest of the exports — its presence means
        // they all exist. Everything try/caught: pad input must never break
        // the page.
        webView?.evaluateJavaScript(
            "try{var E=wasm_exports;if(typeof E!==\"undefined\"&&E.set_pad_stick){\(body)}}catch(e){}",
            completionHandler: nil)
    }
}
