# R8 rules for the Android shell (release + preview build types; debug is
# not minified). AGP's proguard-android-optimize.txt supplies the defaults.
#
# The page calls window.PegasusApp.setKeepAwake(...) / appBuild() by NAME
# (index.html's wake-lock holder and the About screen's Version row), so
# the JS-bridge methods must survive obfuscation. AGP's default rules
# already keep @JavascriptInterface members; this is the belt-and-braces
# copy so a default-file change can never silently rename the bridge —
# the failure mode is a silent one (the page feature-detects the bridge
# and quietly degrades: no wake lock, "dev" version row).
-keepclassmembers class * {
    @android.webkit.JavascriptInterface <methods>;
}

# Keep the bridge's class name readable in Play's crash/ANR traces even
# though nothing looks it up reflectively — it is one class, costs nothing,
# and a stack frame that says PegasusBridge beats one that says "a".
-keepnames class se.danielfalk.pegasus.MainActivity$PegasusBridge
