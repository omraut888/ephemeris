// The iOS app (docs/phase3-design.md §3): the web app's screens in SwiftUI,
// over the Rust engine. Views read the report; the model sequences calls.

import EphemerisKit
import SwiftUI

@main
struct EphemerisApp: App {
    @State private var model = AppModel()
    /// This iPhone as a sync device (docs/phase4b-design.md §10).
    @State private var sync = SyncModel()

    var body: some Scene {
        WindowGroup {
            ContentView(model: model, sync: sync)
        }
    }
}
