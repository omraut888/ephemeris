// The Swift half of the schema drift test (docs/phase3-design.md §1). The
// Rust half builds report-all-fields.json with every field and enum variant;
// here it and the golden report are decoded into the Codable types,
// re-encoded, and compared with the original as JSON values. A field Swift
// lacks is dropped on re-encode, a misspelled key goes missing or fails to
// decode, an unknown variant fails to decode: each fails this test.

import Foundation
import Testing

@testable import EphemerisKit

/// Differences between two JSON values, as paths. `null` and an absent key
/// count as the same: serde writes `None` as `null`, Swift omits it.
func jsonDiff(_ a: Any, _ b: Any, at path: String = "$") -> [String] {
    switch (a, b) {
    case let (a as [String: Any], b as [String: Any]):
        let a = a.filter { !($0.value is NSNull) }
        let b = b.filter { !($0.value is NSNull) }
        var out: [String] = []
        for key in Set(a.keys).union(b.keys).sorted() {
            switch (a[key], b[key]) {
            case let (x?, y?): out += jsonDiff(x, y, at: "\(path).\(key)")
            case (_?, nil): out.append("\(path).\(key): missing after the round trip")
            case (nil, _?): out.append("\(path).\(key): not in the original")
            case (nil, nil): break
            }
        }
        return out
    case let (a as [Any], b as [Any]):
        guard a.count == b.count else { return ["\(path): \(a.count) vs \(b.count) items"] }
        return zip(a, b).enumerated().flatMap { i, p in jsonDiff(p.0, p.1, at: "\(path)[\(i)]") }
    case let (a as NSNumber, b as NSNumber):
        return a == b ? [] : ["\(path): \(a) vs \(b)"]
    case let (a as String, b as String):
        return a == b ? [] : ["\(path): \(a) vs \(b)"]
    default:
        return ["\(path): \(type(of: a)) vs \(type(of: b))"]
    }
}

func roundTrip<T: Codable>(_ type: T.Type, _ data: Data) throws -> [String] {
    let value = try JSONDecoder().decode(type, from: data)
    let again = try JSONEncoder().encode(value)
    return jsonDiff(
        try JSONSerialization.jsonObject(with: data),
        try JSONSerialization.jsonObject(with: again)
    )
}

@Suite struct DriftTests {
    @Test func allFieldsFixtureRoundTrips() throws {
        let diff = try roundTrip([ForecastReport].self, Golden.data("report-all-fields.json"))
        #expect(diff.isEmpty, "\(diff.joined(separator: "\n"))")
    }

    @Test func goldenReportRoundTrips() throws {
        let diff = try roundTrip(ForecastReport.self, Golden.data("report.json"))
        #expect(diff.isEmpty, "\(diff.joined(separator: "\n"))")
    }

    /// The comparison itself catches a field the types do not know.
    @Test func anUnknownFieldIsReported() throws {
        var json = try #require(
            try JSONSerialization.jsonObject(with: Golden.data("report.json")) as? [String: Any])
        json["new_field"] = 1
        let diff = try roundTrip(ForecastReport.self, JSONSerialization.data(withJSONObject: json))
        #expect(diff == ["$.new_field: missing after the round trip"])
    }

    /// Options are only encoded; the engine rejects unknown fields, so a
    /// misspelled key fails here.
    @Test func everyOptionFieldIsAccepted() throws {
        let sim = SimulateOptions(seed: 3, days: 120, mode: .matched, start: "2026-01-01")
        let feed = try simulateJson(optionsJson: String(decoding: JSONEncoder().encode(sim), as: UTF8.self))
        let opts = ForecastOptions(horizonDays: 21, paths: 150, seed: 5, targets: [100, 2500])
        let report = try JSONDecoder().decode(
            ForecastReport.self,
            from: Data(forecastJson(
                feedJson: feed,
                optionsJson: String(decoding: JSONEncoder().encode(opts), as: UTF8.self)
            ).utf8))
        #expect(report.forecast.horizonDays == 21)
        #expect(report.forecast.paths == 150)
        #expect(report.forecast.seed == 5)
        #expect(report.forecast.targets.map(\.amount) == [100, 2500])
    }
}
