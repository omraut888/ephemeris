// The lanes, against the real engine slowed down and counted.

import Foundation
import Testing

@testable import EphemerisKit

/// The FFI engine, each forecast held for a while so requests pile up.
actor CountingEngine: Engine {
    private(set) var forecasts: [String] = []
    let delay: Duration

    init(delay: Duration = .milliseconds(80)) {
        self.delay = delay
    }

    func simulateJSON(_ optionsJSON: String) async throws -> String {
        try await FFIEngine().simulateJSON(optionsJSON)
    }

    func forecastJSON(_ feedJSON: String, _ optionsJSON: String) async throws -> String {
        forecasts.append(optionsJSON)
        try await Task.sleep(for: delay)
        return try await FFIEngine().forecastJSON(feedJSON, optionsJSON)
    }
}

@MainActor
@Suite struct EngineClientTests {
    let feed: String

    init() async throws {
        feed = try await FFIEngine().simulateJSON(#"{"seed": 7, "days": 120}"#)
    }

    @Test func aBurstInOneLaneComputesOnlyTheRunningAndTheNewest() async throws {
        let engine = CountingEngine()
        let client = EngineClient(engine: engine)
        let horizons = [30, 60, 90, 120, 150, 180, 45]
        let tasks = horizons.map { h in
            Task { @MainActor in
                try await client.forecast(feed, ForecastOptions(horizonDays: h, paths: 100), lane: "cone")
                    .forecast.horizonDays
            }
        }
        var results: [Result<Int, Error>] = []
        for t in tasks { results.append(await t.result) }

        #expect(await engine.forecasts.count == 2)
        #expect(try results.first?.get() == 30)
        #expect(try results.last?.get() == 45)
        for r in results.dropFirst().dropLast() {
            #expect(throws: SupersededError.self) { try r.get() }
        }
    }

    @Test func requestsWithoutALaneOrInOtherLanesAllRun() async throws {
        let engine = CountingEngine(delay: .milliseconds(20))
        let client = EngineClient(engine: engine)
        let opts = ForecastOptions(horizonDays: 20, paths: 100)
        async let a = client.forecast(feed, opts, lane: "cone")
        async let b = client.forecast(feed, opts, lane: "afford")
        async let c = client.forecast(feed, opts)
        async let d = client.forecast(feed, opts)
        let all = try await [a, b, c, d]
        #expect(all.map(\.forecast.horizonDays) == [20, 20, 20, 20])
        #expect(await engine.forecasts.count == 4)
    }

    @Test func aFailureFreesTheLaneForTheWaitingRequest() async throws {
        let client = EngineClient(engine: CountingEngine(delay: .milliseconds(40)))
        let bad = Task { @MainActor in try await client.forecast("{", ForecastOptions(), lane: "cone") }
        let good = Task { @MainActor in
            try await client.forecast(feed, ForecastOptions(horizonDays: 20, paths: 100), lane: "cone")
        }
        let err = await #expect(throws: EngineError.self) { try await bad.value }
        #expect(err.map(errorMessage)?.hasPrefix("feed:") == true)
        #expect(try await good.value.forecast.horizonDays == 20)
    }
}
