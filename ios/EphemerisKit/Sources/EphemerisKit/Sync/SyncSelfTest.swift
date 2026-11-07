// A sync round trip through the real Keychain and a real folder, for the
// places a unit test cannot reach: the app runs it in the iOS simulator
// (scripts/test-ios.sh, launch argument `-EPHEMERIS_SYNC_SELFTEST YES`), and
// `swift test` runs it on the Mac.
//
// It uses a throwaway Keychain service, never `ephemeris.sync`, and deletes
// the service's item and its temporary folders whether it passes or not.

import Foundation

public enum SyncSelfTest {
    /// Two devices pair through a temporary folder: a query saved on one
    /// reaches the other, and a delete there reaches the first. Returns a
    /// one-line summary, or throws what went wrong.
    @MainActor
    public static func run(service: String = "ephemeris.sync.selftest") async throws -> String {
        let keychain = KeychainKeyStore(service: service)
        let root = FileManager.default.temporaryDirectory.appending(path: "ephemeris-sync-selftest-\(UUID())")
        let shared = root.appending(path: "Ephemeris Sync")
        try FileManager.default.createDirectory(at: shared, withIntermediateDirectories: true)
        try? keychain.delete()
        defer {
            try? keychain.delete()
            try? FileManager.default.removeItem(at: root)
        }
        func check(_ ok: Bool, _ what: String, _ model: SyncModel) throws {
            guard ok else { throw SyncError("\(what) (status: \(model.status.error ?? "no error"))") }
        }
        let quiet: @Sendable (String) -> Void = { _ in }

        let a = SyncModel(directory: root.appending(path: "a"), keys: keychain, log: quiet)
        await a.createKey()
        try check(a.status.paired, "the key was not stored in the Keychain", a)
        let text = try require(a.showKey(), "the key did not read back")
        await a.chooseFolder(shared)
        await a.saveQuery(amountCents: 50_000, by: "2030-01-01", label: "selftest")
        try check(a.status.error == nil && a.queries.count == 1, "the query was not saved", a)

        let b = SyncModel(directory: root.appending(path: "b"), keys: MemoryKeyStore(), log: quiet)
        await b.pair(text: text)
        await b.chooseFolder(shared)
        try check(b.queries.map(\.label) == ["selftest"], "the query did not reach the second device", b)
        try check(b.status.devices.count == 2, "expected two devices, saw \(b.status.devices.count)", b)
        await b.deleteQuery(id: b.queries[0].id)
        await a.syncNow()
        try check(a.queries.isEmpty, "the delete did not reach the first device", a)

        let files = try FileManager.default.contentsOfDirectory(atPath: shared.path).filter { $0.hasSuffix(".ephsync") }
        for f in files {
            let head = try FileHandle(forReadingFrom: shared.appending(path: f)).read(upToCount: 4)
            guard head == Data("EPHS".utf8) else { throw SyncError("\(f) is not a sealed sync file") }
        }
        // Reopened from disk with the Keychain's key: the same state.
        let again = SyncModel(directory: root.appending(path: "a"), keys: keychain, log: quiet)
        try check(again.status.paired && again.queries.isEmpty && again.status.error == nil, "the saved state did not reopen", again)
        return "\(files.count) sealed files, Keychain round trip, a query saved and deleted across two devices"
    }
}

/// The value, or a `SyncError` saying what was missing.
private func require<T>(_ value: T?, _ message: String) throws -> T {
    guard let value else { throw SyncError(message) }
    return value
}
