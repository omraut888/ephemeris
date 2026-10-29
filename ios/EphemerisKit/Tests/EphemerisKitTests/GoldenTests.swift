// The fourth leg of the byte-identity proof (docs/phase3-design.md §6): the
// golden feed and report through the FFI, compared byte for byte with the
// files the native, WASM and desktop tests check.

import Foundation
import Testing

@testable import EphemerisKit

/// The golden files, copied in by scripts/build-ios.sh.
enum Golden {
    static func data(_ name: String) throws -> Data {
        let url = try #require(
            Bundle.module.url(forResource: name, withExtension: nil, subdirectory: "Golden"),
            "missing \(name): run scripts/build-ios.sh"
        )
        return try Data(contentsOf: url)
    }

    static func text(_ name: String) throws -> String {
        try #require(String(data: data(name), encoding: .utf8))
    }
}

@Suite struct GoldenTests {
    @Test func feedAndReportMatchTheGoldenFilesByteForByte() throws {
        let feed = try simulateJson(optionsJson: Golden.text("simulate-options.json"))
        let goldenFeed = try Golden.data("feed.json")
        #expect(Data(feed.utf8) == goldenFeed, "feed: \(feed.utf8.count) vs \(goldenFeed.count) bytes")

        let report = try forecastJson(feedJson: feed, optionsJson: Golden.text("forecast-options.json"))
        let goldenReport = try Golden.data("report.json")
        #expect(
            Data(report.utf8) == goldenReport,
            "report: \(report.utf8.count) vs \(goldenReport.count) bytes"
        )
    }

    @Test func engineErrorsArriveAsThrownMessages() {
        #expect {
            _ = try forecastJson(feedJson: "not json", optionsJson: "{}")
        } throws: { error in
            guard case let EngineError.Failed(message) = error else { return false }
            return message.hasPrefix("feed:")
        }
    }

    @Test func schemaVersionIsTheCores() {
        #expect(schemaVersion() == 1)
    }
}
