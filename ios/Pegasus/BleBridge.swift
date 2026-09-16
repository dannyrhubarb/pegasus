import CoreBluetooth
import Foundation
import WebKit

/// BLE GATT nearby-link SPIKE — the iOS half of `window.pegBle` (see
/// docs/multiplayer-ble.md; the Android twin is BleBridge.kt).
///
/// The page posts one JSON string per command to the `pegasusBle` script
/// message handler and hears back through `pegBle._on({ev, …})` evaluated on
/// the web view. Native is a dumb PDU pipe: every `send` command is exactly
/// one GATT notification (host → guest) or write-without-response
/// (guest → host) of at most (mtu − 3) bytes, delivered in order — the page
/// frames and chunks, so the two platforms share one codec.
///
/// Roles are fixed per side: the HOST is the peripheral (CBPeripheralManager:
/// advertises the service, notifies on TX, receives writes on RX), the GUEST
/// the central (CBCentralManager: scans, connects, subscribes to TX, writes
/// RX). Both managers are created lazily on the first host/scan command —
/// creating one triggers the system Bluetooth permission prompt, and the
/// game must never prompt at launch — and the command that needed the radio
/// is parked until the manager reports `.poweredOn`.
///
/// Commands: host{name} · scan · connect{id} · send{b64} · stop.
/// Events:   state{state, role?, mtu?, reason?} · mtu{mtu} · peer{id, name,
///           rssi} · data{b64} · error{msg} · log{msg}.
/// States:   idle · advertising · scanning · connecting · connected ·
///           disconnected.
final class BleBridge: NSObject {
    static let handlerName = "pegasusBle"
    /// Document-start flag the page feature-detects (postMessage has no
    /// synchronous return, so presence is announced up front).
    static let flagScript = "window.__pegBleIos = true"

    // Custom 128-bit UUIDs (shared with the Android bridge and the page).
    static let serviceUUID = CBUUID(string: "7E6A5000-0148-4B1E-8F3A-000000000001")
    static let txUUID = CBUUID(string: "7E6A5000-0148-4B1E-8F3A-000000000002") // host → guest (notify)
    static let rxUUID = CBUUID(string: "7E6A5000-0148-4B1E-8F3A-000000000003") // guest → host (write, no response)
    static let defaultMTU = 23

    private weak var webView: WKWebView?
    private var role: String?
    private var mtu = BleBridge.defaultMTU
    private var outbox: [Data] = []
    private var pending: [String: Any]? // command waiting on .poweredOn

    // Host (peripheral) side.
    private var peripheralManager: CBPeripheralManager?
    private var txChar: CBMutableCharacteristic?
    private var subscriber: CBCentral?
    private var advertisedName = ""
    private var serviceAdded = false

    // Guest (central) side.
    private var centralManager: CBCentralManager?
    private var found: [String: CBPeripheral] = [:] // retain the discovered peripherals
    private var peripheral: CBPeripheral?
    private var rxChar: CBCharacteristic?

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
            self?.webView?.evaluateJavaScript("window.pegBle&&pegBle._on(\(json))", completionHandler: nil)
        }
    }

    private func emitState(_ state: String, reason: String? = nil) {
        var f: [String: Any] = ["state": state]
        if let r = role { f["role"] = r }
        if state == "connected" { f["mtu"] = mtu }
        if let reason = reason { f["reason"] = reason }
        emit("state", f)
    }

    private func emitError(_ msg: String) { emit("error", ["msg": msg]) }
    private func emitLog(_ msg: String) { emit("log", ["msg": msg]) }

    // MARK: - Commands from the page

    private func dispatch(_ c: [String: Any]) {
        switch c["cmd"] as? String ?? "" {
        case "host":
            guard ensurePeripheral(c) else { return }
            host(name: c["name"] as? String ?? "")
        case "scan":
            guard ensureCentral(c) else { return }
            scan()
        case "connect":
            connect(id: c["id"] as? String ?? "")
        case "send":
            if let b64 = c["b64"] as? String, let d = Data(base64Encoded: b64) { send(d) }
        case "stop":
            stop(reason: "stopped")
        default:
            emitError("unknown command")
        }
    }

    /// True when the peripheral manager is on; else the command is parked.
    private func ensurePeripheral(_ c: [String: Any]) -> Bool {
        if peripheralManager == nil {
            peripheralManager = CBPeripheralManager(delegate: self, queue: nil)
        }
        switch peripheralManager!.state {
        case .poweredOn: return true
        case .unknown, .resetting: pending = c; return false
        case .unauthorized: emitError("bluetooth permission denied"); return false
        case .poweredOff: emitError("bluetooth is off"); return false
        case .unsupported: emitError("no BLE hardware"); return false
        @unknown default: emitError("bluetooth unavailable"); return false
        }
    }

    private func ensureCentral(_ c: [String: Any]) -> Bool {
        if centralManager == nil {
            centralManager = CBCentralManager(delegate: self, queue: nil)
        }
        switch centralManager!.state {
        case .poweredOn: return true
        case .unknown, .resetting: pending = c; return false
        case .unauthorized: emitError("bluetooth permission denied"); return false
        case .poweredOff: emitError("bluetooth is off"); return false
        case .unsupported: emitError("no BLE hardware"); return false
        @unknown default: emitError("bluetooth unavailable"); return false
        }
    }

    // MARK: - Host (peripheral)

    private func host(name: String) {
        stop(reason: nil)
        role = "host"
        advertisedName = name
        guard let pm = peripheralManager else { return }
        if serviceAdded {
            startAdvertising()
            return
        }
        let tx = CBMutableCharacteristic(type: BleBridge.txUUID, properties: [.notify], value: nil, permissions: [.readable])
        let rx = CBMutableCharacteristic(type: BleBridge.rxUUID, properties: [.writeWithoutResponse], value: nil, permissions: [.writeable])
        let service = CBMutableService(type: BleBridge.serviceUUID, primary: true)
        service.characteristics = [tx, rx]
        txChar = tx
        pm.add(service) // → peripheralManager(_:didAdd:error:) → startAdvertising
    }

    private func startAdvertising() {
        // CoreBluetooth advertises only service UUIDs + a local name (no
        // service data), so the callsign rides the local name; an Android
        // central reads it from the scan record's device name.
        peripheralManager?.startAdvertising([
            CBAdvertisementDataServiceUUIDsKey: [BleBridge.serviceUUID],
            CBAdvertisementDataLocalNameKey: advertisedName,
        ])
    }

    // MARK: - Guest (central)

    private func scan() {
        stop(reason: nil)
        role = "guest"
        found.removeAll()
        centralManager?.scanForPeripherals(
            withServices: [BleBridge.serviceUUID],
            options: [CBCentralManagerScanOptionAllowDuplicatesKey: true] // RSSI updates
        )
        emitState("scanning")
    }

    private func connect(id: String) {
        guard let p = found[id] else { emitError("unknown peer \(id)"); return }
        centralManager?.stopScan()
        peripheral = p
        p.delegate = self
        emitState("connecting")
        centralManager?.connect(p, options: nil)
    }

    // MARK: - Sending: one PDU per command, drained as the radio allows

    private func send(_ pdu: Data) {
        guard !pdu.isEmpty else { return }
        if pdu.count > mtu - 3 { emitError("pdu \(pdu.count) > mtu-3 (\(mtu - 3))"); return }
        outbox.append(pdu)
        pump()
    }

    private func pump() {
        while let pdu = outbox.first {
            let sent: Bool
            switch role {
            case "host":
                guard let pm = peripheralManager, let tx = txChar, subscriber != nil else { outbox.removeAll(); return }
                // false = the transmit queue is full; peripheralManagerIsReady
                // (toUpdateSubscribers:) resumes the drain from this PDU.
                sent = pm.updateValue(pdu, for: tx, onSubscribedCentrals: nil)
            case "guest":
                guard let p = peripheral, let rx = rxChar else { outbox.removeAll(); return }
                if !p.canSendWriteWithoutResponse { return } // peripheralIsReady(toSendWriteWithoutResponse:) resumes
                p.writeValue(pdu, for: rx, type: .withoutResponse)
                sent = true
            default:
                outbox.removeAll(); return
            }
            if !sent { return }
            outbox.removeFirst()
        }
    }

    // MARK: - Teardown

    /// Tear everything down; `reason` nil = silent (a role switch).
    private func stop(reason: String?) {
        if let pm = peripheralManager, pm.isAdvertising { pm.stopAdvertising() }
        centralManager?.stopScan()
        if let p = peripheral { centralManager?.cancelPeripheralConnection(p) }
        peripheral = nil; rxChar = nil; subscriber = nil
        found.removeAll(); outbox.removeAll(); mtu = BleBridge.defaultMTU
        let hadRole = role != nil
        role = nil
        if let reason = reason, hadRole { emitState("idle", reason: reason) }
    }
}

// MARK: - WKScriptMessageHandler

extension BleBridge: WKScriptMessageHandler {
    func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
        guard message.name == BleBridge.handlerName else { return }
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

extension BleBridge: CBPeripheralManagerDelegate {
    func peripheralManagerDidUpdateState(_ pm: CBPeripheralManager) {
        if pm.state == .poweredOn, let c = pending, (c["cmd"] as? String) == "host" {
            pending = nil
            dispatch(c)
        } else if pm.state != .poweredOn, role == "host" {
            stop(reason: "bluetooth went away")
        }
    }

    func peripheralManager(_ pm: CBPeripheralManager, didAdd service: CBService, error: Error?) {
        if let e = error { emitError("addService failed: \(e.localizedDescription)"); stop(reason: "addService failed"); return }
        serviceAdded = true
        startAdvertising()
    }

    func peripheralManagerDidStartAdvertising(_ pm: CBPeripheralManager, error: Error?) {
        if let e = error { emitError("advertise failed: \(e.localizedDescription)"); stop(reason: "advertise failed"); return }
        emitState("advertising")
    }

    func peripheralManager(_ pm: CBPeripheralManager, central: CBCentral, didSubscribeTo characteristic: CBCharacteristic) {
        guard characteristic.uuid == BleBridge.txUUID else { return }
        subscriber = central
        // maximumUpdateValueLength is the notification PAYLOAD cap (ATT MTU − 3).
        mtu = central.maximumUpdateValueLength + 3
        pm.stopAdvertising()
        emitState("connected")
    }

    func peripheralManager(_ pm: CBPeripheralManager, central: CBCentral, didUnsubscribeFrom characteristic: CBCharacteristic) {
        guard characteristic.uuid == BleBridge.txUUID, subscriber?.identifier == central.identifier else { return }
        subscriber = nil; outbox.removeAll(); mtu = BleBridge.defaultMTU
        emitState("disconnected", reason: "peer left")
    }

    func peripheralManager(_ pm: CBPeripheralManager, didReceiveWrite requests: [CBATTRequest]) {
        for r in requests where r.characteristic.uuid == BleBridge.rxUUID {
            if let v = r.value, !v.isEmpty { emit("data", ["b64": v.base64EncodedString()]) }
        }
        // RX is write-without-response only; a response is neither expected
        // nor harmful, but the docs ask for one per request batch.
        if let first = requests.first, first.characteristic.properties.contains(.write) {
            pm.respond(to: first, withResult: .success)
        }
    }

    func peripheralManagerIsReady(toUpdateSubscribers pm: CBPeripheralManager) {
        pump()
    }
}

// MARK: - CBCentralManagerDelegate + CBPeripheralDelegate (guest)

extension BleBridge: CBCentralManagerDelegate, CBPeripheralDelegate {
    func centralManagerDidUpdateState(_ cm: CBCentralManager) {
        if cm.state == .poweredOn, let c = pending, (c["cmd"] as? String) == "scan" {
            pending = nil
            dispatch(c)
        } else if cm.state != .poweredOn, role == "guest" {
            stop(reason: "bluetooth went away")
        }
    }

    func centralManager(_ cm: CBCentralManager, didDiscover p: CBPeripheral, advertisementData: [String: Any], rssi RSSI: NSNumber) {
        let id = p.identifier.uuidString
        found[id] = p
        // An Android host puts the callsign in service data; an iOS host
        // can only advertise a local name. Take whichever is there.
        var name = ""
        if let sd = advertisementData[CBAdvertisementDataServiceDataKey] as? [CBUUID: Data],
           let d = sd[BleBridge.serviceUUID], let s = String(data: d, encoding: .utf8) {
            name = s
        } else if let s = advertisementData[CBAdvertisementDataLocalNameKey] as? String {
            name = s
        } else if let s = p.name {
            name = s
        }
        emit("peer", ["id": id, "name": name, "rssi": RSSI.intValue])
    }

    func centralManager(_ cm: CBCentralManager, didConnect p: CBPeripheral) {
        emitLog("connected, discovering")
        p.discoverServices([BleBridge.serviceUUID])
    }

    func centralManager(_ cm: CBCentralManager, didFailToConnect p: CBPeripheral, error: Error?) {
        emitError("connect failed: \(error?.localizedDescription ?? "?")")
        stop(reason: "connect failed")
    }

    func centralManager(_ cm: CBCentralManager, didDisconnectPeripheral p: CBPeripheral, error: Error?) {
        guard p === peripheral else { return }
        peripheral = nil; rxChar = nil; outbox.removeAll(); mtu = BleBridge.defaultMTU
        emitState("disconnected", reason: error?.localizedDescription ?? "peer left")
    }

    func peripheral(_ p: CBPeripheral, didDiscoverServices error: Error?) {
        guard let svc = p.services?.first(where: { $0.uuid == BleBridge.serviceUUID }) else {
            emitError("service not found"); stop(reason: "no service"); return
        }
        p.discoverCharacteristics([BleBridge.txUUID, BleBridge.rxUUID], for: svc)
    }

    func peripheral(_ p: CBPeripheral, didDiscoverCharacteristicsFor service: CBService, error: Error?) {
        guard let chars = service.characteristics,
              let tx = chars.first(where: { $0.uuid == BleBridge.txUUID }),
              let rx = chars.first(where: { $0.uuid == BleBridge.rxUUID }) else {
            emitError("characteristics not found"); stop(reason: "no characteristics"); return
        }
        rxChar = rx
        p.setNotifyValue(true, for: tx)
    }

    func peripheral(_ p: CBPeripheral, didUpdateNotificationStateFor characteristic: CBCharacteristic, error: Error?) {
        guard characteristic.uuid == BleBridge.txUUID else { return }
        if let e = error { emitError("subscribe failed: \(e.localizedDescription)"); stop(reason: "subscribe failed"); return }
        guard characteristic.isNotifying else { return }
        // maximumWriteValueLength is the write PAYLOAD cap (ATT MTU − 3).
        mtu = p.maximumWriteValueLength(for: .withoutResponse) + 3
        emitState("connected")
    }

    func peripheral(_ p: CBPeripheral, didUpdateValueFor characteristic: CBCharacteristic, error: Error?) {
        guard characteristic.uuid == BleBridge.txUUID, let v = characteristic.value, !v.isEmpty else { return }
        emit("data", ["b64": v.base64EncodedString()])
    }

    func peripheralIsReady(toSendWriteWithoutResponse p: CBPeripheral) {
        pump()
    }
}
