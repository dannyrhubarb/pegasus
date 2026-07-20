import WebKit

/// Serves the app bundle's WebRoot/ folder on the custom `pegasus://` scheme.
/// Everything is answered synchronously from disk inside start(), so stop()
/// has nothing to cancel and the task can never be called after completion.
final class WebRootSchemeHandler: NSObject, WKURLSchemeHandler {
    static let scheme = "pegasus"

    func webView(_ webView: WKWebView, start urlSchemeTask: WKURLSchemeTask) {
        guard let url = urlSchemeTask.request.url else { return }

        // Drop the query — the page appends ?v= cache-busters and ?fresh=
        // reload markers, but the bundle holds exactly one copy of everything.
        var path = url.path
        if path.isEmpty || path == "/" { path = "/index.html" }
        let clean = (path as NSString).standardizingPath
        guard !clean.contains("..") else { return finish(urlSchemeTask, url: url, status: 403, data: Data()) }

        guard let root = Bundle.main.resourceURL?.appendingPathComponent("WebRoot"),
              let data = try? Data(contentsOf: root.appendingPathComponent(String(clean.dropFirst()))) else {
            // 404 is a working answer, not an error: the page probes for
            // optional files (config.json, version.json, whats-new.json) and
            // treats a miss as "feature off".
            return finish(urlSchemeTask, url: url, status: 404, data: Data())
        }
        finish(urlSchemeTask, url: url, status: 200, data: data,
               contentType: Self.mimeType(forExtension: (clean as NSString).pathExtension))
    }

    func webView(_ webView: WKWebView, stop urlSchemeTask: WKURLSchemeTask) {
        // Responses complete synchronously in start() — nothing in flight.
    }

    private func finish(
        _ task: WKURLSchemeTask, url: URL, status: Int, data: Data,
        contentType: String = "text/plain; charset=utf-8"
    ) {
        let response = HTTPURLResponse(
            url: url, statusCode: status, httpVersion: "HTTP/1.1",
            headerFields: [
                "Content-Type": contentType,
                "Content-Length": String(data.count),
                "Cache-Control": "no-cache",
            ]
        )!
        task.didReceive(response)
        task.didReceive(data)
        task.didFinish()
    }

    private static func mimeType(forExtension ext: String) -> String {
        switch ext.lowercased() {
        case "html": return "text/html; charset=utf-8"
        case "js": return "text/javascript; charset=utf-8"
        case "wasm": return "application/wasm"
        case "json": return "application/json"
        case "png": return "image/png"
        case "svg": return "image/svg+xml"
        case "ico": return "image/x-icon"
        case "css": return "text/css; charset=utf-8"
        // .level files, LICENSE (no extension) and anything else texty.
        case "level", "txt", "": return "text/plain; charset=utf-8"
        default: return "application/octet-stream"
        }
    }
}
