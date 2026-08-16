import UIKit
import AVFoundation

@main
class AppDelegate: UIResponder, UIApplicationDelegate {
    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]?
    ) -> Bool {
        // Ambient + mixWithOthers: the game's synthesized thruster/boom sounds
        // must not pause the player's podcast/music like the default playback
        // category would.
        try? AVAudioSession.sharedInstance().setCategory(.ambient, options: [.mixWithOthers])
        return true
    }

    func application(
        _ application: UIApplication,
        configurationForConnecting connectingSceneSession: UISceneSession,
        options: UIScene.ConnectionOptions
    ) -> UISceneConfiguration {
        // Besides the app's one phone/pad scene, the system offers the
        // AirPlay-mirrored display as a NON-INTERACTIVE external-display
        // scene (this callback fires for it even with
        // UIApplicationSupportsMultipleScenes = false). Answering with a
        // delegate lets the app replace the letterboxed mirror image with
        // its own full-screen TV window — see AirPlay.swift. The role is
        // matched negatively so both the iOS 16+ role and the legacy
        // pre-16 external-display role take this path without deprecation
        // dances (no other non-application role can reach us — CarPlay
        // needs an entitlement this app doesn't have).
        if connectingSceneSession.role != .windowApplication {
            let config = UISceneConfiguration(
                name: "External Display", sessionRole: connectingSceneSession.role)
            config.delegateClass = ExternalSceneDelegate.self
            return config
        }
        return UISceneConfiguration(name: "Default", sessionRole: connectingSceneSession.role)
    }
}
