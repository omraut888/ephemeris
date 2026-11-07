// App state: a feed (demo or file) → a forecast report → the views
// (web/src/App.tsx). All computation goes through the engine; this file only
// sequences calls. It lives in the package, not the app target, so
// `swift test` covers it on the Mac.

import Foundation
import Observation

@MainActor
@Observable
public final class AppModel {
    public enum Status: Equatable, Sendable {
        case loading, ready, error
    }

    public enum Source: Equatable, Sendable {
        case demo(mode: SimMode, seed: UInt64)
        case file(name: String)
        /// The feed the sync state materializes (docs/phase4b-design.md §10).
        case synced(through: IsoDate?)

        public var label: String {
            switch self {
            case let .demo(mode, seed): "demo · \(mode == .matched ? "model-matched" : "realistic") · seed \(seed)"
            case let .file(name): "file · \(name)"
            case let .synced(through): "synced · through \(through ?? "?")"
            }
        }
    }

    public enum ConeView: Sendable {
        case daily, lows
    }

    /// Daily, as on the web and the CLI (docs/phase2-design.md §4).
    public static let defaultView = ConeView.daily
    public static let horizons = [30, 90, 180]
    public static let paths = 2000
    /// Fixed, so the same feed always draws the same cone.
    public static let forecastSeed: UInt64 = 0
    /// A horizon change waits this long for the next before it forecasts.
    public static let inputDebounce = Duration.milliseconds(150)
    /// Larger files are refused unread, as in the desktop app.
    public static let maxFeedBytes = 32 * 1024 * 1024

    public private(set) var status = Status.loading
    public private(set) var error: String?
    public private(set) var source: Source?
    public private(set) var feedJSON: String?
    public private(set) var report: ForecastReport?
    public var view = AppModel.defaultView
    public private(set) var horizon = 90
    public var mode = SimMode.realistic
    public var seed: UInt64 = 7

    @ObservationIgnored private let client: EngineClient
    /// Only the latest request may update the screen.
    @ObservationIgnored private var latest = 0
    @ObservationIgnored private var debounced: Task<Void, Never>?

    public init(engine: Engine = FFIEngine()) {
        client = EngineClient(engine: engine)
    }

    /// The synced settings' options, used while synced data is on screen.
    public private(set) var syncedOptions: ForecastOptions?

    /// The horizons to offer: the usual three, and the synced one if it differs.
    public var horizonChoices: [Int] {
        Set(Self.horizons + [horizon]).sorted()
    }

    public func options(horizon: Int, targets: [Double] = [Risk.overdrawnTarget]) -> ForecastOptions {
        options(for: source, horizon: horizon, targets: targets)
    }

    /// The synced settings' paths and seed for synced data, so every device
    /// draws the same cone; the app's own otherwise.
    func options(for source: Source?, horizon: Int, targets: [Double] = [Risk.overdrawnTarget]) -> ForecastOptions {
        // Target 0 gives the report its P(below $0 on some day) curve.
        if case .synced = source, let synced = syncedOptions {
            return ForecastOptions(
                horizonDays: horizon, paths: synced.paths ?? Self.paths,
                seed: synced.seed ?? Self.forecastSeed, targets: targets)
        }
        return ForecastOptions(horizonDays: horizon, paths: Self.paths, seed: Self.forecastSeed, targets: targets)
    }

    private func run(_ source: Source, feed: () async throws -> String, horizon: Int) async {
        debounced?.cancel()
        latest += 1
        let ticket = latest
        status = .loading
        do {
            let json = try await feed()
            let report = try await client.forecast(json, options(for: source, horizon: horizon), lane: "cone")
            guard ticket == latest else { return }
            self.source = source
            feedJSON = json
            self.report = report
            if report.forecast.cycleLows == nil { view = .daily }
            error = nil
            status = .ready
        } catch {
            guard ticket == latest, !(error is SupersededError) else { return }
            fail(errorMessage(error))
        }
    }

    private func fail(_ message: String) {
        error = message
        status = .error
    }

    /// Show an error from outside the engine (the synced data failing to
    /// materialize), superseding any forecast in flight.
    public func show(error message: String) {
        latest += 1
        fail(message)
    }

    /// A new demo feed with the chosen mode and seed.
    public func runDemo() async {
        let (mode, seed) = (mode, seed)
        await run(.demo(mode: mode, seed: seed), feed: {
            try await self.client.simulate(SimulateOptions(seed: seed, mode: mode))
        }, horizon: horizon)
    }

    /// Change the horizon; the forecast follows after the debounce.
    public func setHorizon(_ h: Int) {
        horizon = h
        guard let source, let feedJSON else { return }
        status = .loading
        debounced?.cancel()
        debounced = Task { [weak self] in
            try? await Task.sleep(for: Self.inputDebounce)
            guard !Task.isCancelled, let self else { return }
            // `run` cancels any pending debounce; this one is no longer pending.
            self.debounced = nil
            await self.run(source, feed: { feedJSON }, horizon: h)
        }
    }

    /// A feed's text, from the Files picker or a launch argument.
    public func load(name: String, text: String) async {
        guard (try? JSONSerialization.jsonObject(with: Data(text.utf8))) != nil else {
            latest += 1
            fail("\(name) is not JSON.")
            return
        }
        await run(.file(name: name), feed: { text }, horizon: horizon)
    }

    /// A file picked in Files: read inside its security scope, size-checked.
    public func open(url: URL) async {
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        let name = url.lastPathComponent
        do {
            let size = try url.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0
            guard size <= Self.maxFeedBytes else {
                latest += 1
                let mib = 1024 * 1024
                return fail("\(name) is \(size / mib) MiB; feeds over \(Self.maxFeedBytes / mib) MiB are not opened")
            }
            let text = String(decoding: try Data(contentsOf: url), as: UTF8.self)
            await load(name: name, text: text)
        } catch {
            latest += 1
            fail("\(name): \(errorMessage(error))")
        }
    }

    /// Show the synced data, with the synced horizon, paths and seed.
    public func showSynced(_ synced: SyncedFeed) async {
        do {
            let opts = try JSONDecoder().decode(ForecastOptions.self, from: Data(synced.optionsJSON.utf8))
            syncedOptions = opts
            horizon = opts.horizonDays ?? horizon
            await run(.synced(through: synced.through), feed: { synced.feedJSON }, horizon: horizon)
        } catch {
            latest += 1
            fail("the synced settings: \(errorMessage(error))")
        }
    }

    /// After a sync: redraw if synced data is on screen and it changed.
    public func refreshSynced(_ synced: SyncedFeed?) async {
        guard case .synced = source, let synced else { return }
        let opts = try? JSONDecoder().decode(ForecastOptions.self, from: Data(synced.optionsJSON.utf8))
        guard synced.feedJSON != feedJSON || opts != syncedOptions else { return }
        syncedOptions = opts
        await run(.synced(through: synced.through), feed: { synced.feedJSON }, horizon: horizon)
    }

    /// "Can I afford X by D?": the same forecast with X as the target, so the
    /// same paths as the cone on screen.
    public func afford(amount: Double, date: IsoDate) async throws -> AffordAnswer? {
        guard let feedJSON else { return nil }
        let r = try await client.forecast(feedJSON, options(horizon: horizon, targets: [amount]), lane: "afford")
        return AffordAnswer(report: r, amount: amount, date: date)
    }
}
