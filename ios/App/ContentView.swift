// The one screen: header, headline risk, the cone with its controls, the
// afford query and the current state (docs/phase3-design.md §3).

import EphemerisKit
import SwiftUI
import UniformTypeIdentifiers

struct ContentView: View {
    let model: AppModel
    @State private var importing = false
    @State private var demoSettings = false

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    if let error = model.error {
                        Text(error)
                            .font(.callout)
                            .foregroundStyle(Theme.bad)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(12)
                            .background(Theme.bad.opacity(0.12), in: RoundedRectangle(cornerRadius: 8))
                            .accessibilityIdentifier("error")
                    }
                    if let report = model.report {
                        RiskView(report: report)
                        ForecastPanel(model: model, report: report)
                        AffordView(model: model, report: report)
                            .id("\(report.asOf)-\(report.forecast.horizonDays)-\(model.source?.label ?? "")")
                        StatePanelView(report: report)
                    } else if model.status == .loading {
                        ProgressView("Loading the engine…")
                            .frame(maxWidth: .infinity, minHeight: 300)
                    } else {
                        Text("No data yet.").foregroundStyle(Theme.dim)
                    }
                }
                .padding(16)
            }
            .background(Theme.bg)
            .toolbar {
                ToolbarItem(placement: .principal) { header }
                ToolbarItem(placement: .topBarTrailing) {
                    Menu {
                        Button("Open feed…", systemImage: "doc") { importing = true }
                        Button("Demo…", systemImage: "wand.and.stars") { demoSettings = true }
                    } label: {
                        Image(systemName: "ellipsis.circle")
                    }
                    .accessibilityLabel("Data source")
                }
            }
            .navigationBarTitleDisplayMode(.inline)
            .fileImporter(isPresented: $importing, allowedContentTypes: [.json]) { result in
                switch result {
                case let .success(url): Task { await model.open(url: url) }
                case .failure: break  // cancelled, or Files reported its own error
                }
            }
            .sheet(isPresented: $demoSettings) {
                DemoSettings(model: model)
                    .presentationDetents([.medium])
            }
        }
        .tint(Theme.accent)
        .task { await start() }
        #if DEBUG
        .onChange(of: model.status) { _, status in
            // Lines for scripts/test-ios.sh to wait for, on stderr: unbuffered,
            // so they arrive even when the console is a pipe.
            if status == .ready { log("EPHEMERIS ready: \(model.source?.label ?? "")") }
            if status == .error { log("EPHEMERIS error: \(model.error ?? "")") }
        }
        #endif
    }

    private var header: some View {
        VStack(spacing: 1) {
            Text("EPHEMERIS")
                .font(.subheadline.weight(.bold))
                .tracking(2.5)
            HStack(spacing: 5) {
                Circle()
                    .fill(model.status == .loading ? Theme.warn : model.status == .error ? Theme.bad : Theme.good)
                    .frame(width: 6, height: 6)
                Text(model.status == .loading ? "computing…" : model.source?.label ?? "no data")
                    .font(.caption2)
                    .foregroundStyle(Theme.dim)
                    .lineLimit(1)
                    .accessibilityIdentifier("source")
            }
        }
    }

    /// Opens on a demo, or, in Debug builds, on what the screenshot script
    /// asks for: `-EPHEMERIS_FEED <file in Documents>`, `-EPHEMERIS_VIEW lows`.
    private func start() async {
        #if DEBUG
        let defaults = UserDefaults.standard
        if defaults.string(forKey: "EPHEMERIS_VIEW") == "lows" { model.view = .lows }
        if let name = defaults.string(forKey: "EPHEMERIS_FEED") {
            let url = URL.documentsDirectory.appending(path: name)
            if let text = try? String(contentsOf: url, encoding: .utf8) {
                await model.load(name: name, text: text)
            } else {
                await model.load(name: name, text: "")
            }
            if defaults.string(forKey: "EPHEMERIS_VIEW") == "lows" { model.view = .lows }
            return
        }
        #endif
        await model.runDemo()
    }
}

/// Demo mode and seed, then a new demo.
struct DemoSettings: View {
    @Bindable var model: AppModel
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            Form {
                Picker("Mode", selection: $model.mode) {
                    Text("realistic").tag(SimMode.realistic)
                    Text("model-matched").tag(SimMode.matched)
                }
                Stepper("Seed \(model.seed)", value: $model.seed, in: 0...9_999)
            }
            .navigationTitle("Demo")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Run") {
                        dismiss()
                        Task { await model.runDemo() }
                    }
                }
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
            }
        }
    }
}

#if DEBUG
private func log(_ line: String) {
    FileHandle.standardError.write(Data((line + "\n").utf8))
}
#endif
