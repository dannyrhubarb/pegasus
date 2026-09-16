package se.danielfalk.pegasus

import android.Manifest
import android.annotation.SuppressLint
import android.app.Activity
import android.bluetooth.BluetoothAdapter
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothGatt
import android.bluetooth.BluetoothGattCallback
import android.bluetooth.BluetoothGattCharacteristic
import android.bluetooth.BluetoothGattDescriptor
import android.bluetooth.BluetoothGattServer
import android.bluetooth.BluetoothGattServerCallback
import android.bluetooth.BluetoothGattService
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothProfile
import android.bluetooth.le.AdvertiseCallback
import android.bluetooth.le.AdvertiseData
import android.bluetooth.le.AdvertiseSettings
import android.bluetooth.le.BluetoothLeAdvertiser
import android.bluetooth.le.BluetoothLeScanner
import android.bluetooth.le.ScanCallback
import android.bluetooth.le.ScanFilter
import android.bluetooth.le.ScanResult
import android.bluetooth.le.ScanSettings
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.ParcelUuid
import android.util.Base64
import android.webkit.JavascriptInterface
import android.webkit.WebView
import org.json.JSONObject
import java.util.ArrayDeque
import java.util.UUID

/**
 * BLE GATT nearby-link SPIKE — the Android half of `window.pegBle`
 * (see docs/multiplayer-ble.md; the iOS twin is ios/Pegasus/BleBridge.swift).
 *
 * The page talks to this through ONE JS interface method, `cmd(json)`, and
 * hears back through `pegBle._on({ev, …})` evaluated on the WebView. Native
 * is deliberately a dumb PDU pipe: every `send` command is exactly one GATT
 * notification (host → guest) or write-without-response (guest → host) of at
 * most (mtu − 3) bytes, delivered in order — the page does the message
 * framing and the chunking so both platforms share a single codec.
 *
 * Roles: the HOST is the GATT peripheral (advertises the service, answers
 * with notifications on TX, receives writes on RX); the GUEST is the central
 * (scans, connects, subscribes to TX, writes RX). Fixed per side so the two
 * phones never race for the same role — and it maps onto #148's host/guest
 * split.
 *
 * Commands: host{name} · scan · connect{id} · send{b64} · stop.
 * Events:   state{state, role?, mtu?, reason?} · mtu{mtu} · peer{id, name,
 *           rssi} · data{b64} · error{msg} · log{msg}.
 * States:   idle · advertising · scanning · connecting · connected ·
 *           disconnected.
 *
 * Everything runs on the main thread (GATT callbacks arrive on binder
 * threads and are posted over) so the outbound queue and the event order
 * the page sees are both serialized. Permissions are requested lazily on
 * the first host/scan command — the game must never prompt for Bluetooth
 * at launch — and the interrupted command re-runs when they are granted.
 */
@SuppressLint("MissingPermission") // every entry point checks first (hasPermissions)
class BleBridge(private val activity: Activity, private val webView: WebView) {
    companion object {
        // Custom 128-bit UUIDs (shared with the iOS bridge and the page).
        val SERVICE_UUID: UUID = UUID.fromString("7e6a5000-0148-4b1e-8f3a-000000000001")
        val TX_UUID: UUID = UUID.fromString("7e6a5000-0148-4b1e-8f3a-000000000002") // host → guest (notify)
        val RX_UUID: UUID = UUID.fromString("7e6a5000-0148-4b1e-8f3a-000000000003") // guest → host (write, no response)
        val CCCD_UUID: UUID = UUID.fromString("00002902-0000-1000-8000-00805f9b34fb")
        const val PERMISSION_REQUEST = 0x8B1E
        const val DEFAULT_MTU = 23
        const val WANT_MTU = 517
        // Scan-response budget: 31 − (2 + 16 for a 128-bit service-data AD).
        const val ADV_NAME_MAX = 12
    }

    private val main = Handler(Looper.getMainLooper())
    private val manager: BluetoothManager?
        get() = activity.getSystemService(Context.BLUETOOTH_SERVICE) as? BluetoothManager
    private val adapter: BluetoothAdapter? get() = manager?.adapter

    private var role: String? = null
    private var mtu = DEFAULT_MTU
    private val outbox = ArrayDeque<ByteArray>()
    private var inflight = false
    private var pending: JSONObject? = null // command waiting on a permission grant

    // Host (peripheral) side.
    private var server: BluetoothGattServer? = null
    private var txChar: BluetoothGattCharacteristic? = null
    private var subscriber: BluetoothDevice? = null
    private var advertiser: BluetoothLeAdvertiser? = null
    private var advCallback: AdvertiseCallback? = null

    // Guest (central) side.
    private var scanner: BluetoothLeScanner? = null
    private var scanCallback: ScanCallback? = null
    private val found = HashMap<String, BluetoothDevice>()
    private var gatt: BluetoothGatt? = null
    private var rxChar: BluetoothGattCharacteristic? = null

    // ---- JS surface ------------------------------------------------------

    @JavascriptInterface
    fun cmd(json: String) {
        main.post {
            try {
                dispatch(JSONObject(json))
            } catch (e: Exception) {
                emitError("bad command: $e")
            }
        }
    }

    private fun dispatch(c: JSONObject) {
        when (c.optString("cmd")) {
            "host" -> if (ensureReady(c)) host(c.optString("name"))
            "scan" -> if (ensureReady(c)) scan()
            "connect" -> connect(c.optString("id"))
            "send" -> send(Base64.decode(c.optString("b64"), Base64.DEFAULT))
            "stop" -> stop("stopped")
            else -> emitError("unknown command ${c.optString("cmd")}")
        }
    }

    private fun emit(ev: String, fill: (JSONObject.() -> Unit)? = null) {
        val o = JSONObject().put("ev", ev)
        fill?.invoke(o)
        val js = "window.pegBle&&pegBle._on(${JSONObject.quote(o.toString())})"
        main.post { webView.evaluateJavascript(js, null) }
    }

    private fun emitState(state: String, reason: String? = null) = emit("state") {
        put("state", state)
        role?.let { put("role", it) }
        if (state == "connected") put("mtu", mtu)
        reason?.let { put("reason", it) }
    }

    private fun emitError(msg: String) = emit("error") { put("msg", msg) }
    private fun emitLog(msg: String) = emit("log") { put("msg", msg) }

    // ---- Permissions / adapter ------------------------------------------

    private fun requiredPermissions(): Array<String> =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) arrayOf(
            Manifest.permission.BLUETOOTH_SCAN,
            Manifest.permission.BLUETOOTH_ADVERTISE,
            Manifest.permission.BLUETOOTH_CONNECT,
        ) else arrayOf(
            // Pre-12 scanning is location-gated; the classic BLUETOOTH /
            // BLUETOOTH_ADMIN permissions are install-time.
            Manifest.permission.ACCESS_FINE_LOCATION,
        )

    private fun hasPermissions() = requiredPermissions().all {
        activity.checkSelfPermission(it) == PackageManager.PERMISSION_GRANTED
    }

    /** True when the command may proceed; otherwise it is parked and re-run on grant. */
    private fun ensureReady(c: JSONObject): Boolean {
        if (!activity.packageManager.hasSystemFeature(PackageManager.FEATURE_BLUETOOTH_LE)) {
            emitError("no BLE hardware"); return false
        }
        if (!hasPermissions()) {
            pending = c
            activity.requestPermissions(requiredPermissions(), PERMISSION_REQUEST)
            return false
        }
        val a = adapter
        if (a == null) { emitError("no bluetooth adapter"); return false }
        if (!a.isEnabled) {
            // Best-effort: ask the system to switch it on; the user retries.
            try { activity.startActivity(Intent(BluetoothAdapter.ACTION_REQUEST_ENABLE)) } catch (_: Exception) {}
            emitError("bluetooth is off"); return false
        }
        return true
    }

    /** Called from MainActivity.onRequestPermissionsResult. */
    fun onPermissionResult(requestCode: Int, grantResults: IntArray) {
        if (requestCode != PERMISSION_REQUEST) return
        val c = pending ?: return
        pending = null
        if (grantResults.isNotEmpty() && grantResults.all { it == PackageManager.PERMISSION_GRANTED }) {
            main.post { dispatch(c) }
        } else {
            emitError("bluetooth permission denied")
        }
    }

    // ---- Host (GATT server + advertiser) --------------------------------

    private fun host(name: String) {
        stop(null)
        role = "host"
        val m = manager ?: run { emitError("no bluetooth manager"); return }
        val srv = m.openGattServer(activity, serverCallback) ?: run { emitError("openGattServer failed"); return }
        server = srv
        val service = BluetoothGattService(SERVICE_UUID, BluetoothGattService.SERVICE_TYPE_PRIMARY)
        val tx = BluetoothGattCharacteristic(
            TX_UUID, BluetoothGattCharacteristic.PROPERTY_NOTIFY, BluetoothGattCharacteristic.PERMISSION_READ
        )
        tx.addDescriptor(BluetoothGattDescriptor(
            CCCD_UUID, BluetoothGattDescriptor.PERMISSION_READ or BluetoothGattDescriptor.PERMISSION_WRITE
        ))
        val rx = BluetoothGattCharacteristic(
            RX_UUID, BluetoothGattCharacteristic.PROPERTY_WRITE_NO_RESPONSE, BluetoothGattCharacteristic.PERMISSION_WRITE
        )
        service.addCharacteristic(tx)
        service.addCharacteristic(rx)
        txChar = tx
        if (!srv.addService(service)) { emitError("addService failed"); stop("addService failed"); return }

        val adv = adapter?.bluetoothLeAdvertiser ?: run {
            emitError("this device cannot advertise (no BLE peripheral support)"); stop("no advertiser"); return
        }
        advertiser = adv
        val settings = AdvertiseSettings.Builder()
            .setAdvertiseMode(AdvertiseSettings.ADVERTISE_MODE_LOW_LATENCY)
            .setTxPowerLevel(AdvertiseSettings.ADVERTISE_TX_POWER_HIGH)
            .setConnectable(true)
            .setTimeout(0)
            .build()
        // The service UUID goes in the advertisement; the callsign rides the
        // scan response as service data (the adapter's device name is the
        // phone's Bluetooth name, not ours — and changing it is global).
        val advData = AdvertiseData.Builder()
            .addServiceUuid(ParcelUuid(SERVICE_UUID))
            .setIncludeDeviceName(false)
            .setIncludeTxPowerLevel(false)
            .build()
        val nameBytes = name.toByteArray(Charsets.UTF_8).let { if (it.size > ADV_NAME_MAX) it.copyOf(ADV_NAME_MAX) else it }
        val scanResp = AdvertiseData.Builder()
            .addServiceData(ParcelUuid(SERVICE_UUID), nameBytes)
            .build()
        val cb = object : AdvertiseCallback() {
            override fun onStartSuccess(settingsInEffect: AdvertiseSettings?) {
                emitState("advertising")
            }
            override fun onStartFailure(errorCode: Int) {
                emitError("advertise failed: $errorCode"); stop("advertise failed")
            }
        }
        advCallback = cb
        adv.startAdvertising(settings, advData, scanResp, cb)
    }

    private val serverCallback = object : BluetoothGattServerCallback() {
        override fun onConnectionStateChange(device: BluetoothDevice, status: Int, newState: Int) {
            main.post {
                if (newState == BluetoothProfile.STATE_CONNECTED) {
                    emitLog("central connected ${device.address}")
                    // Keep advertising until the guest subscribes — a second
                    // central would just fail to subscribe (one subscriber).
                } else if (newState == BluetoothProfile.STATE_DISCONNECTED && device == subscriber) {
                    subscriber = null; outbox.clear(); inflight = false; mtu = DEFAULT_MTU
                    emitState("disconnected", "peer left")
                }
            }
        }

        override fun onMtuChanged(device: BluetoothDevice, newMtu: Int) {
            main.post { mtu = newMtu; if (device == subscriber) emit("mtu") { put("mtu", newMtu) } }
        }

        override fun onDescriptorWriteRequest(
            device: BluetoothDevice, requestId: Int, descriptor: BluetoothGattDescriptor,
            preparedWrite: Boolean, responseNeeded: Boolean, offset: Int, value: ByteArray?
        ) {
            main.post {
                if (responseNeeded) server?.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, 0, value)
                if (descriptor.uuid == CCCD_UUID && value != null &&
                    value.contentEquals(BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE)) {
                    subscriber = device
                    try { advertiser?.stopAdvertising(advCallback) } catch (_: Exception) {}
                    emitState("connected")
                }
            }
        }

        override fun onCharacteristicWriteRequest(
            device: BluetoothDevice, requestId: Int, characteristic: BluetoothGattCharacteristic,
            preparedWrite: Boolean, responseNeeded: Boolean, offset: Int, value: ByteArray?
        ) {
            main.post {
                if (responseNeeded) server?.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, 0, value)
                if (characteristic.uuid == RX_UUID && value != null && value.isNotEmpty()) {
                    emit("data") { put("b64", Base64.encodeToString(value, Base64.NO_WRAP)) }
                }
            }
        }

        override fun onNotificationSent(device: BluetoothDevice, status: Int) {
            main.post { inflight = false; pump() }
        }
    }

    // ---- Guest (scanner + GATT client) ----------------------------------

    private fun scan() {
        stop(null)
        role = "guest"
        val sc = adapter?.bluetoothLeScanner ?: run { emitError("no scanner"); return }
        scanner = sc
        val cb = object : ScanCallback() {
            override fun onScanResult(callbackType: Int, result: ScanResult) { main.post { onPeer(result) } }
            override fun onBatchScanResults(results: MutableList<ScanResult>) { main.post { results.forEach { onPeer(it) } } }
            override fun onScanFailed(errorCode: Int) { main.post { emitError("scan failed: $errorCode"); stop("scan failed") } }
        }
        scanCallback = cb
        sc.startScan(
            listOf(ScanFilter.Builder().setServiceUuid(ParcelUuid(SERVICE_UUID)).build()),
            ScanSettings.Builder().setScanMode(ScanSettings.SCAN_MODE_LOW_LATENCY).build(),
            cb,
        )
        emitState("scanning")
    }

    private fun onPeer(result: ScanResult) {
        val dev = result.device
        found[dev.address] = dev
        val rec = result.scanRecord
        // An Android host puts the callsign in service data; an iOS host can
        // only advertise a local name. Take whichever is there.
        val name = rec?.getServiceData(ParcelUuid(SERVICE_UUID))?.toString(Charsets.UTF_8)
            ?: rec?.deviceName ?: ""
        emit("peer") { put("id", dev.address); put("name", name); put("rssi", result.rssi) }
    }

    private fun connect(id: String) {
        val dev = found[id] ?: run { emitError("unknown peer $id"); return }
        try { scanner?.stopScan(scanCallback) } catch (_: Exception) {}
        emitState("connecting")
        gatt = dev.connectGatt(activity, false, clientCallback, BluetoothDevice.TRANSPORT_LE)
    }

    private val clientCallback = object : BluetoothGattCallback() {
        override fun onConnectionStateChange(g: BluetoothGatt, status: Int, newState: Int) {
            main.post {
                if (newState == BluetoothProfile.STATE_CONNECTED) {
                    emitLog("connected, negotiating mtu")
                    if (!g.requestMtu(WANT_MTU)) g.discoverServices()
                } else if (newState == BluetoothProfile.STATE_DISCONNECTED) {
                    outbox.clear(); inflight = false; rxChar = null; mtu = DEFAULT_MTU
                    try { g.close() } catch (_: Exception) {}
                    if (gatt === g) gatt = null
                    emitState("disconnected", "status $status")
                }
            }
        }

        override fun onMtuChanged(g: BluetoothGatt, newMtu: Int, status: Int) {
            main.post {
                if (status == BluetoothGatt.GATT_SUCCESS) mtu = newMtu
                g.discoverServices()
            }
        }

        override fun onServicesDiscovered(g: BluetoothGatt, status: Int) {
            main.post {
                val svc = g.getService(SERVICE_UUID)
                val tx = svc?.getCharacteristic(TX_UUID)
                val rx = svc?.getCharacteristic(RX_UUID)
                if (svc == null || tx == null || rx == null) { emitError("service not found"); stop("no service"); return@post }
                rxChar = rx
                g.setCharacteristicNotification(tx, true)
                val cccd = tx.getDescriptor(CCCD_UUID) ?: run { emitError("no CCCD"); stop("no cccd"); return@post }
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                    g.writeDescriptor(cccd, BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE)
                } else {
                    @Suppress("DEPRECATION")
                    cccd.value = BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE
                    @Suppress("DEPRECATION")
                    g.writeDescriptor(cccd)
                }
            }
        }

        override fun onDescriptorWrite(g: BluetoothGatt, descriptor: BluetoothGattDescriptor, status: Int) {
            main.post {
                if (descriptor.uuid == CCCD_UUID) {
                    if (status == BluetoothGatt.GATT_SUCCESS) emitState("connected")
                    else { emitError("subscribe failed: $status"); stop("subscribe failed") }
                }
            }
        }

        override fun onCharacteristicWrite(g: BluetoothGatt, characteristic: BluetoothGattCharacteristic, status: Int) {
            main.post { inflight = false; pump() }
        }

        // API 33+ delivers the value here…
        override fun onCharacteristicChanged(g: BluetoothGatt, characteristic: BluetoothGattCharacteristic, value: ByteArray) {
            main.post { if (characteristic.uuid == TX_UUID) emit("data") { put("b64", Base64.encodeToString(value, Base64.NO_WRAP)) } }
        }

        // …and older releases through the deprecated overload.
        @Deprecated("Deprecated in Java")
        override fun onCharacteristicChanged(g: BluetoothGatt, characteristic: BluetoothGattCharacteristic) {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) return
            @Suppress("DEPRECATION")
            val v = characteristic.value ?: return
            main.post { if (characteristic.uuid == TX_UUID) emit("data") { put("b64", Base64.encodeToString(v, Base64.NO_WRAP)) } }
        }
    }

    // ---- Sending: one PDU per command, strictly one in flight -------------

    private fun send(pdu: ByteArray) {
        if (pdu.isEmpty()) return
        if (pdu.size > mtu - 3) { emitError("pdu ${pdu.size} > mtu-3 (${mtu - 3})"); return }
        outbox.add(pdu)
        pump()
    }

    private fun pump() {
        if (inflight) return
        val pdu = outbox.poll() ?: return
        inflight = true
        val ok = try {
            when (role) {
                "host" -> {
                    val dev = subscriber; val tx = txChar; val srv = server
                    if (dev == null || tx == null || srv == null) false
                    else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                        srv.notifyCharacteristicChanged(dev, tx, false, pdu) == BluetoothGatt.GATT_SUCCESS
                    } else {
                        @Suppress("DEPRECATION")
                        tx.value = pdu
                        @Suppress("DEPRECATION")
                        srv.notifyCharacteristicChanged(dev, tx, false)
                    }
                }
                "guest" -> {
                    val g = gatt; val rx = rxChar
                    if (g == null || rx == null) false
                    else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                        g.writeCharacteristic(rx, pdu, BluetoothGattCharacteristic.WRITE_TYPE_NO_RESPONSE) == BluetoothGatt.GATT_SUCCESS
                    } else {
                        @Suppress("DEPRECATION")
                        rx.writeType = BluetoothGattCharacteristic.WRITE_TYPE_NO_RESPONSE
                        @Suppress("DEPRECATION")
                        rx.value = pdu
                        @Suppress("DEPRECATION")
                        g.writeCharacteristic(rx)
                    }
                }
                else -> false
            }
        } catch (e: Exception) { emitError("send failed: $e"); false }
        if (!ok) { inflight = false; emitError("send dropped (${pdu.size} B)") }
    }

    // ---- Teardown ---------------------------------------------------------

    /** Tear everything down; `reason` null = silent (a role switch). */
    private fun stop(reason: String?) {
        try { advertiser?.stopAdvertising(advCallback) } catch (_: Exception) {}
        try { scanner?.stopScan(scanCallback) } catch (_: Exception) {}
        try { gatt?.disconnect(); gatt?.close() } catch (_: Exception) {}
        try { server?.close() } catch (_: Exception) {}
        advertiser = null; advCallback = null; scanner = null; scanCallback = null
        gatt = null; rxChar = null; server = null; txChar = null; subscriber = null
        found.clear(); outbox.clear(); inflight = false; mtu = DEFAULT_MTU
        val hadRole = role != null
        role = null
        if (reason != null && hadRole) emitState("idle", reason)
    }

    fun destroy() = stop(null)
}
