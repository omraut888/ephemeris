// The iOS app (docs/phase3-design.md §3): the web app's screens in SwiftUI,
// over the Rust engine. Views read the report; the model sequences calls.

import EphemerisKit
import SwiftUI

@main
struct EphemerisApp: App {
    @State private var model = AppModel()

    var body: some Scene {
        WindowGroup {
            ContentView(model: model)
        }
    }
}
