import UIKit

class SceneDelegate: UIResponder, UIWindowSceneDelegate {
    var window: UIWindow?

    func scene(
        _ scene: UIScene,
        willConnectTo session: UISceneSession,
        options connectionOptions: UIScene.ConnectionOptions
    ) {
        guard let windowScene = scene as? UIWindowScene else { return }
        let window = UIWindow(windowScene: windowScene)
        window.backgroundColor = .black
        let game = GameViewController()
        // Cold start from a Universal Link (https://pegasusmoonlander.com/?…):
        // the bundled game is what opens either way, but the query string is
        // forwarded so utm-tagged links attribute in analytics and future
        // link parameters reach the page. Only the site root / index.html is
        // universal-linked (see .well-known/apple-app-site-association).
        game.launchQuery = Self.universalLinkQuery(connectionOptions.userActivities)
        window.rootViewController = game
        self.window = window
        window.makeKeyAndVisible()
    }

    /// A Universal Link arriving while the app is already running. The game
    /// is deliberately left alone — reloading the page would kill a run in
    /// progress, and there is nothing to deep-link to yet. Foregrounding the
    /// app is the whole effect.
    ///
    /// TODO(#148): multiplayer invite links are `https://pegasusmoonlander.com/?join=<code>`
    /// (auto-join on landing). A cold start already forwards that query;
    /// once #148 lands, a WARM invite must hand the code to the page
    /// instead of being dropped — evaluateJavaScript into the page's
    /// join entry point (let the page decide: join from the menu, prompt
    /// mid-run), never a reload. If the invite ever moves to a `#`
    /// fragment, forward `url.fragment` too — `url.query` alone drops it.
    func scene(_ scene: UIScene, continue userActivity: NSUserActivity) {}

    private static func universalLinkQuery(_ activities: Set<NSUserActivity>) -> String? {
        for activity in activities where activity.activityType == NSUserActivityTypeBrowsingWeb {
            if let url = activity.webpageURL, let q = url.query, !q.isEmpty { return q }
        }
        return nil
    }
}
