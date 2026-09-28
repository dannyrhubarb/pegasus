package se.danielfalk.pegasus

import android.Manifest
import android.annotation.SuppressLint
import android.app.Activity
import android.bluetooth.BluetoothAdapter
import android.bluetooth.BluetoothManager
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
import android.content.pm.PackageManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.ParcelUuid
import android.webkit.JavascriptInterface
import android.webkit.WebView
import org.json.JSONObject
import java.util.UUID

/**
 * Nearby room discovery over Bluetooth LE — the Android half of the page's
 * `window.pegNearby` module (the iOS twin is ios/Pegasus/NearbyBridge.swift;
 * the contract is documented in docs/multiplayer-p2p.md, "Nearby
 * discovery").
 *
 * Bluetooth carries DISCOVERY ONLY. A multiplayer host advertises its room
 * code (plus a short callsign) in a BLE advertisement; a guest scans, lists
 * the rooms it hears, and joining one is the ordinary room-code join — the
 * signaling WebSocket and the WebRTC DataChannel carry everything else.
 * No GATT service, no connection, no data ever crosses Bluetooth, which is
 * what keeps this bridge small and both platforms identical.
 *
 * Commands (one JSON string per `cmd` call): advertise{text} · scan · stop.
 * Events (evaluated on the WebView as `pegNearby._on({ev, …})`):
 *   state{state: idle|advertising|scanning, reason?} · adv{id, text, rssi}
 *   · error{msg}.
 * Reasons on a forced idle: denied · off · unsupported · failed · stopped.
 *
 * Advertisement layout (shared with iOS and the page): the primary packet
 * carries SERVICE_UUID; the scan response carries the payload text —
 * `<5-char room code><callsign>` — as SERVICE DATA under that UUID (an
 * iOS host can only advertise a local NAME, so it puts the same text
 * there; a scanner accepts either). A 31-byte scan response minus the
 * 18-byte 128-bit service-data header leaves PAYLOAD_MAX = 13 bytes: the
 * code plus up to 8 bytes of callsign.
 *
 * Permissions are requested lazily on the first advertise/scan command
 * and the command re-runs on grant — the game never prompts at launch.
 * Only the Android 12+ Bluetooth runtime permissions are used (SCAN is
 * flagged neverForLocation in the manifest): scanning on Android 11 and
 * older would need fine LOCATION, which this app deliberately never
 * declares, so a pre-12 guest simply gets no nearby list (`unsupported`)
 * while a pre-12 HOST still advertises (no runtime permission needed
 * there). Everything runs on the main thread (scan/advertise callbacks
 * are posted over) so the page sees events in order.
 */
@SuppressLint("MissingPermission") // every entry point checks first (ensureReady)
class NearbyBridge(private val activity: Activity, private val webView: WebView) {
    companion object {
        /** The Pegasus nearby-room service (shared with iOS and the page). */
        val SERVICE_UUID: UUID = UUID.fromString("7e6a5148-0000-4b1e-8f3a-000000000001")
        const val PERMISSION_REQUEST = 0x5148
        /** 31 − (1 len + 1 type + 16 UUID) bytes of service data. */
        const val PAYLOAD_MAX = 13
    }

    private val main = Handler(Looper.getMainLooper())
    private val adapter: BluetoothAdapter?
        get() = (activity.getSystemService(Context.BLUETOOTH_SERVICE) as? BluetoothManager)?.adapter

    private var state = "idle"
    private var pending: JSONObject? = null // command parked on a permission ask
    private var advertiser: BluetoothLeAdvertiser? = null
    private var advCallback: AdvertiseCallback? = null
    private var scanner: BluetoothLeScanner? = null
    private var scanCallback: ScanCallback? = null

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
            "advertise" -> if (ensureReady(c, scanning = false)) advertise(c.optString("text"))
            "scan" -> if (ensureReady(c, scanning = true)) scan()
            "stop" -> stop("stopped")
            else -> emitError("unknown command ${c.optString("cmd")}")
        }
    }

    private fun emit(ev: String, fill: (JSONObject.() -> Unit)? = null) {
        val o = JSONObject().put("ev", ev)
        fill?.invoke(o)
        val js = "window.pegNearby&&pegNearby._on(${JSONObject.quote(o.toString())})"
        main.post { webView.evaluateJavascript(js, null) }
    }

    private fun setState(next: String, reason: String? = null) {
        state = next
        emit("state") {
            put("state", next)
            reason?.let { put("reason", it) }
        }
    }

    private fun emitError(msg: String) = emit("error") { put("msg", msg) }

    // ---- Permissions / adapter ------------------------------------------

    /** The runtime permission a role needs; null = none on this release. */
    private fun permissionFor(scanning: Boolean): String? = when {
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.S ->
            if (scanning) Manifest.permission.BLUETOOTH_SCAN else Manifest.permission.BLUETOOTH_ADVERTISE
        else -> null // pre-12 advertising is covered by install-time BLUETOOTH_ADMIN
    }

    /** True when the command may proceed; otherwise it is parked (permission) or refused. */
    private fun ensureReady(c: JSONObject, scanning: Boolean): Boolean {
        if (!activity.packageManager.hasSystemFeature(PackageManager.FEATURE_BLUETOOTH_LE)) {
            stop(null); setState("idle", "unsupported"); return false
        }
        if (scanning && Build.VERSION.SDK_INT < Build.VERSION_CODES.S) {
            // Scanning here would need fine location, which the app never
            // declares (see the class comment) — no nearby list on Android 11-.
            stop(null); setState("idle", "unsupported"); return false
        }
        val perm = permissionFor(scanning)
        if (perm != null && activity.checkSelfPermission(perm) != PackageManager.PERMISSION_GRANTED) {
            pending = c
            activity.requestPermissions(arrayOf(perm), PERMISSION_REQUEST)
            return false
        }
        val a = adapter
        if (a == null || !a.isEnabled) {
            stop(null); setState("idle", "off"); return false
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
            setState("idle", "denied")
        }
    }

    // ---- Host: advertise the room ---------------------------------------

    private fun advertise(text: String) {
        stop(null)
        val adv = adapter?.bluetoothLeAdvertiser ?: run {
            setState("idle", "unsupported"); return // no peripheral role on this chipset
        }
        advertiser = adv
        val settings = AdvertiseSettings.Builder()
            .setAdvertiseMode(AdvertiseSettings.ADVERTISE_MODE_LOW_LATENCY)
            .setTxPowerLevel(AdvertiseSettings.ADVERTISE_TX_POWER_HIGH)
            // Nothing to connect to (no GATT server): a scannable,
            // non-connectable advertisement — a stray central connecting
            // would otherwise pause the advertising for everyone else.
            .setConnectable(false)
            .setTimeout(0)
            .build()
        val advData = AdvertiseData.Builder()
            .addServiceUuid(ParcelUuid(SERVICE_UUID))
            .setIncludeDeviceName(false)
            .setIncludeTxPowerLevel(false)
            .build()
        // The page already caps the text; the copy here only guards the
        // 31-byte scan response against a longer one (the stack would
        // refuse it with ADVERTISE_FAILED_DATA_TOO_LARGE).
        val payload = text.toByteArray(Charsets.UTF_8).let {
            if (it.size > PAYLOAD_MAX) it.copyOf(PAYLOAD_MAX) else it
        }
        val scanResp = AdvertiseData.Builder()
            .addServiceData(ParcelUuid(SERVICE_UUID), payload)
            .build()
        val cb = object : AdvertiseCallback() {
            override fun onStartSuccess(settingsInEffect: AdvertiseSettings?) {
                main.post { setState("advertising") }
            }
            override fun onStartFailure(errorCode: Int) {
                main.post {
                    emitError("advertise failed: $errorCode")
                    stop(null); setState("idle", "failed")
                }
            }
        }
        advCallback = cb
        adv.startAdvertising(settings, advData, scanResp, cb)
    }

    // ---- Guest: scan for rooms ------------------------------------------

    private fun scan() {
        stop(null)
        val sc = adapter?.bluetoothLeScanner ?: run { setState("idle", "off"); return }
        scanner = sc
        val cb = object : ScanCallback() {
            override fun onScanResult(callbackType: Int, result: ScanResult) { main.post { onAdv(result) } }
            override fun onBatchScanResults(results: MutableList<ScanResult>) { main.post { results.forEach { onAdv(it) } } }
            override fun onScanFailed(errorCode: Int) {
                main.post {
                    emitError("scan failed: $errorCode")
                    stop(null); setState("idle", "failed")
                }
            }
        }
        scanCallback = cb
        sc.startScan(
            listOf(ScanFilter.Builder().setServiceUuid(ParcelUuid(SERVICE_UUID)).build()),
            ScanSettings.Builder().setScanMode(ScanSettings.SCAN_MODE_LOW_LATENCY).build(),
            cb,
        )
        setState("scanning")
    }

    private fun onAdv(result: ScanResult) {
        val rec = result.scanRecord ?: return
        // An Android host carries the text as service data, an iOS host as
        // its local name — take whichever is there; the page validates.
        val text = rec.getServiceData(ParcelUuid(SERVICE_UUID))?.toString(Charsets.UTF_8)
            ?: rec.deviceName ?: return
        emit("adv") {
            put("id", result.device.address)
            put("text", text)
            put("rssi", result.rssi)
        }
    }

    // ---- Teardown ---------------------------------------------------------

    /** Stop everything; `reason` null = silent (a role switch or an error path). */
    private fun stop(reason: String?) {
        try { advertiser?.stopAdvertising(advCallback) } catch (_: Exception) {}
        try { scanner?.stopScan(scanCallback) } catch (_: Exception) {}
        advertiser = null; advCallback = null; scanner = null; scanCallback = null
        val wasActive = state != "idle"
        state = "idle"
        if (reason != null && wasActive) setState("idle", reason)
    }

    fun destroy() = stop(null)
}
