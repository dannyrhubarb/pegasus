import CoreBluetooth
import Foundation
import WebKit

/// Nearby room discovery over Bluetooth LE — the iOS half of the page's
/// `window.pegNearby` module (the Android twin is NearbyBridge.kt; the
/// contract is documented in docs/multiplayer-p2p.md, "Nearby discovery").
///
/// Bluetooth carries DISCOVERY ONLY. A multiplayer host advertises its room
/// code (plus a short callsign); a guest scans, lists the rooms it hears,
/// and joining one is the ordinary room-code join — the signaling
/// WebSocket and the WebRTC DataChannel carry everything else. No GATT
/// service, no connection, no data ever crosses Bluetooth.
///
/// The page posts one JSON string per command to the `pegasusNearby` script
/// message handler and hears back through `pegNearby._on({ev, …})`
/// evaluated on the web view. Commands: advertise{text} · scan · stop.
/// Events: state{state: idle|advertising|scanning, reason?} · adv{id, text,
/// rssi} · error{msg}. Reasons on a forced idle: denied · off ·
/// unsupported · failed · stopped.
///
/// Advertisement layout (shared with Android and the page): the service
/// UUID in the primary packet, the payload text `<5-char code><callsign>`
/// as the LOCAL NAME — CoreBluetooth advertises only service UUIDs and a
/// local name (no service data), while an Android host puts the same text
/// in service data under the UUID; a scanner accepts either.
///
/// Both managers are created lazily on the first advertise/scan command —
/// creating one triggers the system Bluetooth permission prompt, and the
/// game must never prompt at launch — and the command that needed the
/// radio is parked until the manager reports `.poweredOn`.
final class NearbyBridge: NSObject {
    static let handlerName = "pegasusNearby"
    /// Document-start flag the page feature-detects (postMessage has no
    /// synchronous return, so presence is announced up front).
    static let flagScript = "window.__pegNearbyIos = true"
    /// The Pegasus nearby-room service (shared with Android and the page).
    static let serviceUUID = CBUUID(string: "7E6A5148-0000-4B1E-8F3A-000000000001")

    private weak var webView: WKWebView?
    private var state = "idle"
    private var pending: [String: Any]? // command waiting on .poweredOn
    private var peripheralManager: CBPeripheralManager?
    private var centralManager: CBCentralManager?

    init(webView: WKWebView) {
        self.webView = webView
        super.init()
    }

    // MARK: - Events to the page

    private func emit(_ ev: String, _ fields: [String: Any] = [:]) {
        var o: [String: Any] = ["ev": ev]
        fields.forEach { o[$0.key] = $0.value }
        guard let data = try? JSONSerialization.data(withJSONObject: o),
              let json = String(data: data, encoding: .utf8) else { return }
        DispatchQueue.main.async { [weak self] in
            self?.webView?.evaluateJavaScript("window.pegNearby&&pegNearby._on(\(json))", completionHandler: nil)
        }
    }

    private func setState(_ next: String, reason: String? = nil) {
        state = next
        var f: [String: Any] = ["state": next]
        if let reason = reason { f["reason"] = reason }
        emit("state", f)
    }

    private func emitError(_ msg: String) { emit("error", ["msg": msg]) }

    // MARK: - Commands from the page

    private func dispatch(_ c: [String: Any]) {
        switch c["cmd"] as? String ?? "" {
        case "advertise":
            guard ensurePeripheral(c) else { return }
            advertise(text: c["text"] as? String ?? "")
        case "scan":
            guard ensureCentral(c) else { return }
            scan()
        case "stop":
            stop(reason: "stopped")
        default:
            emitError("unknown command")
        }
    }

    /// Maps a manager state to go / park / refuse (with the page's reason code).
    private func gate(_ managerState: CBManagerState, _ c: [String: Any]) -> Bool {
        switch managerState {
        case .poweredOn: return true
        case .unknown, .resetting: pending = c; return false
        case .unauthorized: stop(reason: nil); setState("idle", reason: "denied"); return false
        case .poweredOff: stop(reason: nil); setState("idle", reason: "off"); return false
        case .unsupported: stop(reason: nil); setState("idle", reason: "unsupported"); return false
        @unknown default: stop(reason: nil); setState("idle", reason: "failed"); return false
        }
    }

    private func ensurePeripheral(_ c: [String: Any]) -> Bool {
        if peripheralManager == nil {
            peripheralManager = CBPeripheralManager(delegate: self, queue: nil)
        }
        return gate(peripheralManager!.state, c)
    }

    private func ensureCentral(_ c: [String: Any]) -> Bool {
        if centralManager == nil {
            centralManager = CBCentralManager(delegate: self, queue: nil)
        }
        return gate(centralManager!.state, c)
    }

    // MARK: - Host: advertise the room

    private func advertise(text: String) {
        stop(reason: nil)
        // No service to add — the room code IS the advertisement. With a
        // 128-bit UUID filling the primary packet, CoreBluetooth moves the
        // local name into the scan response by itself.
        peripheralManager?.startAdvertising([
            CBAdvertisementDataServiceUUIDsKey: [NearbyBridge.serviceUUID],
            CBAdvertisementDataLocalNameKey: text,
        ])
    }

    // MARK: - Guest: scan for rooms

    private func scan() {
        stop(reason: nil)
        centralManager?.scanForPeripherals(
            withServices: [NearbyBridge.serviceUUID],
            // Repeated reports keep the page's "last seen" fresh, so a host
            // that stops advertising drops off the list.
            options: [CBCentralManagerScanOptionAllowDuplicatesKey: true]
        )
        setState("scanning")
    }

    // MARK: - Teardown

    /// Stop everything; `reason` nil = silent (a role switch or an error path).
    private func stop(reason: String?) {
        if let pm = peripheralManager, pm.isAdvertising { pm.stopAdvertising() }
        if let cm = centralManager, cm.isScanning { cm.stopScan() }
        let wasActive = state != "idle"
        state = "idle"
        if let reason = reason, wasActive { setState("idle", reason: reason) }
    }
}

// MARK: - WKScriptMessageHandler

extension NearbyBridge: WKScriptMessageHandler {
    func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
        guard message.name == NearbyBridge.handlerName else { return }
        guard let json = message.body as? String,
              let data = json.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            emitError("bad command")
            return
        }
        dispatch(obj)
    }
}

// MARK: - CBPeripheralManagerDelegate (host)

extension NearbyBridge: CBPeripheralManagerDelegate {
    func peripheralManagerDidUpdateState(_ pm: CBPeripheralManager) {
        if let c = pending, (c["cmd"] as? String) == "advertise" {
            if pm.state == .poweredOn {
                pending = nil
                dispatch(c)
            } else if pm.state != .unknown && pm.state != .resetting {
                pending = nil
                _ = gate(pm.state, c) // reports denied / off / unsupported
            }
        } else if pm.state != .poweredOn, state == "advertising" {
            stop(reason: nil); setState("idle", reason: "off")
        }
    }

    func peripheralManagerDidStartAdvertising(_ pm: CBPeripheralManager, error: Error?) {
        if let e = error {
            emitError("advertise failed: \(e.localizedDescription)")
            stop(reason: nil); setState("idle", reason: "failed")
            return
        }
        setState("advertising")
    }
}

// MARK: - CBCentralManagerDelegate (guest)

extension NearbyBridge: CBCentralManagerDelegate {
    func centralManagerDidUpdateState(_ cm: CBCentralManager) {
        if let c = pending, (c["cmd"] as? String) == "scan" {
            if cm.state == .poweredOn {
                pending = nil
                dispatch(c)
            } else if cm.state != .unknown && cm.state != .resetting {
                pending = nil
                _ = gate(cm.state, c)
            }
        } else if cm.state != .poweredOn, state == "scanning" {
            stop(reason: nil); setState("idle", reason: "off")
        }
    }

    func centralManager(_ cm: CBCentralManager, didDiscover p: CBPeripheral, advertisementData: [String: Any], rssi RSSI: NSNumber) {
        // An Android host carries the text as service data, an iOS host as
        // its local name — take whichever is there; the page validates.
        // Only the LIVE advertisement counts: `p.name` is CoreBluetooth's
        // cached name for that radio, which could be a room code from an
        // earlier session and would list a room that no longer exists.
        var text: String?
        if let sd = advertisementData[CBAdvertisementDataServiceDataKey] as? [CBUUID: Data],
           let d = sd[NearbyBridge.serviceUUID], let s = String(data: d, encoding: .utf8) {
            text = s
        } else if let s = advertisementData[CBAdvertisementDataLocalNameKey] as? String {
            text = s
        }
        guard let t = text else { return }
        emit("adv", ["id": p.identifier.uuidString, "text": t, "rssi": RSSI.intValue])
    }
}
