// The sync golden check, Swift leg (docs/phase4b-design.md §5): the two
// sealed replica files opened and merged through the FFI, in this shell's own
// order (B, then A, into an empty state), must give the native merged state,
// feed and report byte for byte.

import Foundation
import Testing

@testable import EphemerisKit

@Suite struct SyncGoldenTests {
    /// The state inside `sync_open`'s result. It is a JSON string, so
    /// decoding the object hands back its exact text: Swift never re-encodes
    /// a state.
    private func state(_ file: String, key: Data) throws -> String {
        let opened = try syncOpen(file: Golden.data(file), key: key)
        let object = try JSONSerialization.jsonObject(with: Data(opened.utf8)) as? [String: Any]
        return try #require(object?["state"] as? String)
    }

    @Test func sealedReplicasMergeToTheGoldenFiles() throws {
        let keyText = try Golden.text("key.txt").trimmingCharacters(in: .whitespacesAndNewlines)
        let key = try syncKeyFromText(text: keyText)
        #expect(key.count == 32)
        let a = try state("replica-a.ephsync", key: key)
        let b = try state("replica-b.ephsync", key: key)

        let merged = try syncMerge(a: syncMerge(a: syncNew(), b: b), b: a)
        let goldenState = try Golden.data("merged-state.json")
        #expect(Data(merged.utf8) == goldenState, "state: \(merged.utf8.count) vs \(goldenState.count) bytes")

        let feed = try syncMaterialize(state: merged)
        #expect(Data(feed.utf8) == (try Golden.data("merged-feed.json")))

        let report = try forecastJson(feedJson: feed, optionsJson: syncForecastOptions(state: merged))
        let goldenReport = try Golden.data("merged-report.json")
        #expect(Data(report.utf8) == goldenReport, "report: \(report.utf8.count) vs \(goldenReport.count) bytes")
    }

    @Test func aWrongKeyIsAThrownMessage() throws {
        let wrong = Data(repeating: 1, count: 32)
        #expect {
            _ = try syncOpen(file: Golden.data("replica-a.ephsync"), key: wrong)
        } throws: { error in
            guard case let EngineError.Failed(message) = error else { return false }
            return message.contains("another key")
        }
    }
}
