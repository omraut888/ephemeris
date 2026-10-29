// The app's state flow with the real engine, slowed and counted where it
// matters (the web's App tests, web/test/components.test.tsx).

import Foundation
import Testing

@testable import EphemerisKit

@MainActor
@Suite struct AppModelTests {
    /// Wait until `done` holds, polling the main actor.
    func until(_ what: String, timeout: Duration = .seconds(10), _ done: () -> Bool) async throws {
        let end = ContinuousClock.now + timeout
        while !done() {
            guard ContinuousClock.now < end else { throw CancellationError() }
            try await Task.sleep(for: .milliseconds(10))
        }
    }

    @Test func opensOnADemoWithTheRiskHeadline() async throws {
        let model = AppModel()
        await model.runDemo()
        #expect(model.status == .ready)
        #expect(model.source == .demo(mode: .realistic, seed: 7))
        #expect(model.source?.label == "demo · realistic · seed 7")
        #expect(model.view == AppModel.defaultView)
        let report = try #require(model.report)
        #expect(report.forecast.horizonDays == 90)
        #expect(report.forecast.paths == 2000)
        #expect(Risk.belowZero(report) != nil)
    }

    @Test func runsOneForecastForABurstOfHorizonChanges() async throws {
        let engine = CountingEngine(delay: .zero)
        let model = AppModel(engine: engine)
        await model.runDemo()
        let before = await engine.forecasts.count

        for h in [30, 180, 90, 30, 180, 90, 30, 180, 90, 180] { model.setHorizon(h) }
        #expect(model.status == .loading)
        try await until("the forecast") { model.status == .ready && model.report?.forecast.horizonDays == 180 }
        // Nothing left over fires later.
        try await Task.sleep(for: AppModel.inputDebounce * 3)
        #expect(await engine.forecasts.count - before == 1)
    }

    @Test func loadsAFileAndReportsBadOnes() async throws {
        let model = AppModel()
        let feed = try Golden.text("feed.json")
        await model.load(name: "sandbox-feed.json", text: feed)
        #expect(model.status == .ready)
        #expect(model.source == .file(name: "sandbox-feed.json"))

        await model.load(name: "broken.json", text: "{not json")
        #expect(model.status == .error)
        #expect(model.error == "broken.json is not JSON.")

        await model.load(name: "v9.json", text: #"{"schema_version": 9}"#)
        #expect(model.error?.hasPrefix("feed:") == true)
        // The last good report stays on screen.
        #expect(model.source == .file(name: "sandbox-feed.json"))
    }

    @Test func refusesLargeFilesUnread() async throws {
        let url = FileManager.default.temporaryDirectory.appending(path: "ephemeris-big-\(UUID()).json")
        FileManager.default.createFile(atPath: url.path, contents: nil)
        let handle = try FileHandle(forWritingTo: url)
        try handle.truncate(atOffset: UInt64(AppModel.maxFeedBytes + 1))
        try handle.close()
        defer { try? FileManager.default.removeItem(at: url) }

        let model = AppModel()
        await model.open(url: url)
        #expect(model.status == .error)
        #expect(model.error?.contains("feeds over 32 MiB are not opened") == true)
    }

    @Test func answersTheAffordQueryFromTheSamePaths() async throws {
        let model = AppModel()
        await model.runDemo()
        let report = try #require(model.report)
        let date = report.forecast.days[29].date
        let answer = try #require(try await model.afford(amount: 2000, date: date))
        // The reference: the engine run directly with the same options.
        let direct = try JSONDecoder().decode(ForecastReport.self, from: Data(forecastJson(
            feedJson: #require(model.feedJSON),
            optionsJson: String(decoding: JSONEncoder().encode(model.options(horizon: 90, targets: [2000])), as: UTF8.self)
        ).utf8))
        let target = try #require(direct.forecast.targets.first)
        let (onDay, reachedBy): (Double, Double) = (target.atLeastOnDay[29], target.reachedBy[29])
        #expect(answer.onDay == onDay)
        #expect(answer.reachedBy == reachedBy)
        #expect(answer.reachedBy >= answer.onDay)
        let outside = try await model.afford(amount: 2000, date: "1999-01-01")
        #expect(outside == nil)
    }

    @Test func readsBandsAndRiskTiers() throws {
        let b = BandIndex(levels: [0.05, 0.25, 0.5, 0.75, 0.95])
        #expect([b.lo, b.qlo, b.mid, b.qhi, b.hi] as [Int] == [0, 1, 2, 3, 4])
        let odd = BandIndex(levels: [0.1, 0.3, 0.45, 0.9])
        #expect([odd.lo, odd.qlo, odd.mid, odd.qhi, odd.hi] as [Int] == [0, 1, 2, 3, 3])
        #expect(Risk.tone(0.01) == .good)
        #expect(Risk.tone(0.05) == .warn)
        #expect(Risk.tone(0.2) == .bad)
    }
}
