// The app's engine (docs/phase3-design.md §1): the FFI's JSON entry points
// run off the main thread, and `EngineClient` adds what the web client has.
// Calls that name the same lane keep only the newest: one runs, one waits,
// and a newer request replaces the waiting one, which fails with
// `SupersededError` without ever being computed (web/src/wasm/client.ts).

import Foundation

/// JSON text in, JSON text out: the engine as the shells see it.
public protocol Engine: Sendable {
    func simulateJSON(_ optionsJSON: String) async throws -> String
    func forecastJSON(_ feedJSON: String, _ optionsJSON: String) async throws -> String
}

/// The Rust engine through UniFFI, on a background thread: a forecast is
/// tens of milliseconds of CPU.
public struct FFIEngine: Engine {
    public init() {}

    public func simulateJSON(_ optionsJSON: String) async throws -> String {
        try await Task.detached(priority: .userInitiated) {
            try simulateJson(optionsJson: optionsJSON)
        }.value
    }

    public func forecastJSON(_ feedJSON: String, _ optionsJSON: String) async throws -> String {
        try await Task.detached(priority: .userInitiated) {
            try forecastJson(feedJson: feedJSON, optionsJson: optionsJSON)
        }.value
    }
}

/// A lane request that a newer one replaced before it ran.
public struct SupersededError: Error, Equatable {
    public init() {}
}

/// The message to show for an error: the engine's own text when it has one.
public func errorMessage(_ error: Error) -> String {
    if case let EngineError.Failed(message) = error { return message }
    if let e = error as? LocalizedError, let d = e.errorDescription { return d }
    return String(describing: error)
}

@MainActor
public final class EngineClient {
    private let engine: Engine
    private var lanes: [String: Lane] = [:]

    private struct Lane {
        var busy = false
        var waiting: CheckedContinuation<Void, Error>?
    }

    public init(engine: Engine = FFIEngine()) {
        self.engine = engine
    }

    /// A synthetic feed, as JSON text.
    public func simulate(_ options: SimulateOptions) async throws -> String {
        try await engine.simulateJSON(Self.encode(options))
    }

    /// A report for a feed's JSON text (passed through untouched).
    public func forecast(
        _ feedJSON: String, _ options: ForecastOptions, lane: String? = nil
    ) async throws -> ForecastReport {
        if let lane {
            try await enter(lane)
        }
        defer {
            if let lane { leave(lane) }
        }
        let json = try await engine.forecastJSON(feedJSON, Self.encode(options))
        return try JSONDecoder().decode(ForecastReport.self, from: Data(json.utf8))
    }

    /// Returns when this request may run: at once if the lane is free,
    /// otherwise after the running one, unless a newer request comes first.
    private func enter(_ name: String) async throws {
        guard lanes[name, default: Lane()].busy else {
            lanes[name, default: Lane()].busy = true
            return
        }
        lanes[name]?.waiting?.resume(throwing: SupersededError())
        // The closure runs before suspending, so the request is queued in
        // call order.
        try await withCheckedThrowingContinuation { lanes[name]?.waiting = $0 }
    }

    /// Hand the lane to the waiting request, or free it.
    private func leave(_ name: String) {
        if let next = lanes[name]?.waiting {
            lanes[name]?.waiting = nil
            next.resume()
        } else {
            lanes[name]?.busy = false
        }
    }

    private static func encode<T: Encodable>(_ value: T) throws -> String {
        String(decoding: try JSONEncoder().encode(value), as: UTF8.self)
    }
}
