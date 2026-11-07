// The iOS sync device (docs/phase4b-design.md §10) against real temporary
// folders and an in-memory key; one test round-trips the real Keychain under
// a throwaway service (the desktop's tests in crates/ephemeris-desktop/src/
// sync.rs and the web's in web/test/websync.test.ts cover the same ground).

import Foundation
import Testing

@testable import EphemerisKit

/// Log lines, collected from any thread.
final class LogLines: @unchecked Sendable {
    private let lock = NSLock()
    private var lines: [String] = []
    func add(_ line: String) { lock.withLock { lines.append(line) } }
    var all: [String] { lock.withLock { lines } }
}

/// A temporary folder, deleted at the end of the test.
final class TempDir {
    let url = FileManager.default.temporaryDirectory.appending(path: "ephemeris-sync-test-\(UUID())")
    init() throws { try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true) }
    deinit { try? FileManager.default.removeItem(at: url) }
    func sub(_ name: String) throws -> URL {
        let u = url.appending(path: name)
        try FileManager.default.createDirectory(at: u, withIntermediateDirectories: true)
        return u
    }
}

@MainActor
@Suite struct SyncModelTests {
    let goldenKeyText: String

    init() throws {
        goldenKeyText = try Golden.text("key.txt").trimmingCharacters(in: .whitespacesAndNewlines)
    }

    func until(_ done: () -> Bool) async throws {
        let end = ContinuousClock.now + .seconds(10)
        while !done() {
            guard ContinuousClock.now < end else { throw CancellationError() }
            try await Task.sleep(for: .milliseconds(10))
        }
    }

    func device(_ dir: URL, keys: SyncKeyStore = MemoryKeyStore(), log: LogLines = LogLines()) -> SyncModel {
        SyncModel(directory: dir, keys: keys, log: { log.add($0) })
    }

    @Test func twoDevicesPairAndConvergeThroughAFolder() async throws {
        let tmp = try TempDir()
        let shared = try tmp.sub("Ephemeris Sync")
        let a = device(try tmp.sub("a"))
        #expect(!a.status.paired)
        await a.createKey()
        #expect(a.status.paired && a.status.error == nil)
        await a.chooseFolder(shared)
        #expect(a.status.folder == "Ephemeris Sync")
        #expect(a.status.lastSync != nil)
        #expect(a.status.devices.map(\.thisDevice) == [true])
        await a.saveQuery(amountCents: 200_000, by: "2026-12-31", label: "trip")
        #expect(a.queries.map(\.label) == ["trip"])
        #expect(a.queries[0].amountCents == 200_000 && a.queries[0].by == "2026-12-31")
        #expect(a.queries[0].id == "\(a.status.replica)-1")

        let b = device(try tmp.sub("b"))
        await b.pair(text: try #require(a.showKey()))
        await b.chooseFolder(shared)
        #expect(b.status.error == nil)
        #expect(b.queries == a.queries)
        #expect(b.status.devices.count == 2)
        #expect(b.status.devices.allSatisfy { $0.error == nil })
        let other = try #require(b.status.devices.first { !$0.thisDevice })
        #expect(other.replica == a.status.replica)
        #expect(other.file == "\(a.status.replica).ephsync")

        await b.deleteQuery(id: b.queries[0].id)
        #expect(b.queries.isEmpty)
        await a.syncNow()
        #expect(a.queries.isEmpty)
        #expect(a.status.devices.count == 2)
        // No fetcher yet, and no bank data: nothing to show.
        #expect(a.status.fetcher == nil && !a.status.hasData)
        #expect(try a.syncedFeed() == nil)
    }

    @Test func everyFileIsSealedAndTheStateReopens() async throws {
        let tmp = try TempDir()
        let shared = try tmp.sub("Ephemeris Sync")
        let keys = MemoryKeyStore()
        let dir = try tmp.sub("a")
        let a = device(dir, keys: keys)
        await a.createKey()
        await a.chooseFolder(shared)
        await a.saveQuery(amountCents: 50_000, by: "2027-01-01", label: "sofa")
        for file in [shared.appending(path: "\(a.status.replica).ephsync"), dir.appending(path: "local.ephsync")] {
            let bytes = try Data(contentsOf: file)
            #expect(bytes.prefix(4) == Data("EPHS".utf8), "\(file.lastPathComponent)")
            #expect(bytes.range(of: Data("clocks".utf8)) == nil)
            #expect(bytes.range(of: Data("sofa".utf8)) == nil)
        }
        let config = try String(contentsOf: dir.appending(path: "config.json"), encoding: .utf8)
        #expect(!config.contains(try #require(a.showKey())))

        // The same device after a relaunch: same replica, state, folder.
        let again = device(dir, keys: keys)
        #expect(again.status.replica == a.status.replica)
        #expect(again.status.paired && again.status.error == nil)
        #expect(again.status.folder == "Ephemeris Sync")
        #expect(again.queries == a.queries)
        await again.syncNow()
        #expect(again.status.error == nil && again.status.devices.count == 1)
    }

    @Test func theGoldenReplicasGiveTheGoldenFeedAndNameTheFetcher() async throws {
        let tmp = try TempDir()
        let shared = try tmp.sub("Ephemeris Sync")
        for name in ["replica-a.ephsync", "replica-b.ephsync"] {
            try Golden.data(name).write(to: shared.appending(path: name))
        }
        let phone = device(try tmp.sub("phone"))
        await phone.pair(text: goldenKeyText)
        await phone.chooseFolder(shared)
        let s = phone.status
        #expect(s.error == nil)
        #expect(s.devices.filter { $0.error == nil }.count == 3)
        #expect(s.fetcher == "00000000000000aa")
        #expect(!s.isFetcher)
        #expect(s.hasData)
        let synced = try #require(try phone.syncedFeed())
        #expect(Data(synced.feedJSON.utf8) == (try Golden.data("merged-feed.json")))
        #expect(synced.through == s.fetchedThrough)
        let report = try forecastJson(feedJson: synced.feedJSON, optionsJson: synced.optionsJSON)
        #expect(Data(report.utf8) == (try Golden.data("merged-report.json")))
        // The golden second device saved a query.
        #expect(!phone.queries.isEmpty)
    }

    @Test func showingSyncedDataUsesTheSyncedHorizonPathsAndSeed() async throws {
        let options = #"{"horizon_days":45,"paths":300,"seed":11,"targets":[]}"#
        let synced = SyncedFeed(feedJSON: try Golden.text("feed.json"), optionsJSON: options, through: "2024-08-27")
        let model = AppModel()
        await model.showSynced(synced)
        #expect(model.status == .ready)
        #expect(model.source == .synced(through: "2024-08-27"))
        #expect(model.source?.label == "synced · through 2024-08-27")
        #expect(model.horizon == 45)
        #expect(model.horizonChoices == [30, 45, 90, 180])
        let report = try #require(model.report)
        #expect(report.forecast.horizonDays == 45)
        #expect(report.forecast.paths == 300)
        #expect(model.options(horizon: 45) == ForecastOptions(horizonDays: 45, paths: 300, seed: 11, targets: [0]))

        // Unchanged after a sync: no new forecast. Changed: redrawn.
        await model.refreshSynced(synced)
        #expect(model.report == report)
        var changed = synced
        changed.optionsJSON = #"{"horizon_days":45,"paths":400,"seed":11,"targets":[]}"#
        await model.refreshSynced(changed)
        #expect(model.report?.forecast.paths == 400)
        #expect(model.horizon == 45)
        // A horizon the user picks stays through a sync with other changes...
        model.setHorizon(30)
        try await until { model.status == .ready && model.report?.forecast.horizonDays == 30 }
        changed.optionsJSON = #"{"horizon_days":45,"paths":500,"seed":11,"targets":[]}"#
        await model.refreshSynced(changed)
        #expect(model.report?.forecast.horizonDays == 30)
        // ...and a new synced horizon replaces it.
        changed.optionsJSON = #"{"horizon_days":60,"paths":500,"seed":11,"targets":[]}"#
        await model.refreshSynced(changed)
        #expect(model.horizon == 60 && model.report?.forecast.horizonDays == 60)

        // A demo afterwards uses the app's own options again.
        await model.runDemo()
        #expect(model.report?.forecast.paths == AppModel.paths)
    }

    @Test func filesThatWillNotOpenAreReportedAndKept() async throws {
        let tmp = try TempDir()
        let shared = try tmp.sub("Ephemeris Sync")
        try Data([1, 2, 3]).write(to: shared.appending(path: "0000000000000009.ephsync"))
        try Golden.data("replica-a.ephsync").write(to: shared.appending(path: "00000000000000aa.ephsync"))
        try Data("x".utf8).write(to: shared.appending(path: "notes.txt"))
        try Data([1]).write(to: shared.appending(path: ".00000000000000ee.ephsync.tmp"))
        try Data([1]).write(to: shared.appending(path: ".00000000000000dd.ephsync.icloud"))

        let phone = device(try tmp.sub("phone"))
        await phone.createKey()  // a new key: the golden file is from another
        await phone.chooseFolder(shared)
        #expect(phone.status.error == nil)
        let rows = Dictionary(uniqueKeysWithValues: phone.status.devices.map { ($0.file, $0) })
        #expect(rows.count == 4)
        #expect(rows["0000000000000009.ephsync"]?.error?.contains("not an Ephemeris sync file") == true)
        #expect(rows["00000000000000aa.ephsync"]?.error?.contains("another key") == true)
        #expect(rows["00000000000000dd.ephsync"]?.error == SyncFolder.notDownloaded)
        #expect(rows["\(phone.status.replica).ephsync"]?.error == nil)
        // Nothing is deleted.
        for name in ["0000000000000009.ephsync", "00000000000000aa.ephsync", "notes.txt"] {
            #expect(FileManager.default.fileExists(atPath: shared.appending(path: name).path))
        }
    }

    @Test func pairingAndEditsRefuseWhatTheyShould() async throws {
        let tmp = try TempDir()
        let log = LogLines()
        let d = device(try tmp.sub("d"), log: log)
        #expect(d.showKey() == nil)
        #expect(d.status.error == SyncModel.keyNeeded)
        await d.saveQuery(amountCents: 1, by: "2027-01-01", label: "")
        #expect(d.status.error == SyncModel.keyNeeded)
        await d.pair(text: "RNRA-CJ05")
        #expect(!d.status.paired)
        #expect(d.status.error?.isEmpty == false)

        await d.createKey()
        #expect(d.status.paired && d.status.error == nil)
        await d.pair(text: goldenKeyText)
        #expect(d.status.error == "this device already has a sync key")

        await d.chooseFolder(tmp.url.appending(path: "missing"))
        #expect(d.status.error?.contains("is not a folder") == true)
        #expect(d.status.folder == nil)

        // One line per action, and the key is never logged.
        let lines = log.all
        #expect(lines.contains("[sync] create key: ok"))
        #expect(lines.contains("[sync] pair failed: this device already has a sync key"))
        let key = try #require(d.showKey())
        #expect(!log.all.contains { $0.contains(key) || $0.contains(goldenKeyText) })
    }

    @Test func aMissingFolderIsAStatusErrorNotAThrow() async throws {
        let tmp = try TempDir()
        let shared = try tmp.sub("Ephemeris Sync")
        let log = LogLines()
        let d = device(try tmp.sub("d"), log: log)
        await d.createKey()
        await d.chooseFolder(shared)
        try FileManager.default.removeItem(at: shared)
        await d.syncNow()
        #expect(d.status.error != nil)
        await d.syncNow()
        // Logged once, not on every retry.
        #expect(log.all.filter { $0.hasPrefix("[sync] sync failed") }.count == 1)
    }

    @Test func forgettingTheKeyLeavesTheFolderAlone() async throws {
        let tmp = try TempDir()
        let shared = try tmp.sub("Ephemeris Sync")
        let keys = MemoryKeyStore()
        let dir = try tmp.sub("d")
        let d = device(dir, keys: keys)
        await d.createKey()
        await d.chooseFolder(shared)
        let replica = d.status.replica
        await d.forgetKey()
        #expect(d.status.error == nil)
        #expect(!d.status.paired && d.status.folder == nil && d.status.devices.isEmpty)
        #expect(try keys.get() == nil)
        #expect(!FileManager.default.fileExists(atPath: dir.appending(path: "local.ephsync").path))
        #expect(FileManager.default.fileExists(atPath: shared.appending(path: "\(replica).ephsync").path))
        #expect(d.status.replica == replica)
    }

    @Test func icloudPlaceholdersNameTheRealFile() {
        #expect(SyncFolder.placeholderTarget(".00000000000000dd.ephsync.icloud") == "00000000000000dd.ephsync")
        #expect(SyncFolder.placeholderTarget("00000000000000dd.ephsync.icloud") == nil)
        #expect(SyncFolder.placeholderTarget(".notes.txt.icloud") == nil)
        #expect(SyncFolder.placeholderTarget("..x.ephsync.icloud") == nil)
    }

    #if os(macOS)
    /// The real Keychain, under a throwaway service deleted afterwards. In
    /// the simulator the app runs the same check (scripts/test-ios.sh).
    @Test func theSelfTestPassesThroughTheRealKeychain() async throws {
        let service = "ephemeris.sync.test-\(UUID().uuidString.prefix(8))"
        let summary = try await SyncSelfTest.run(service: service)
        #expect(summary.hasPrefix("2 sealed files"))
        #expect(try KeychainKeyStore(service: service).get() == nil)
    }
    #endif
}
