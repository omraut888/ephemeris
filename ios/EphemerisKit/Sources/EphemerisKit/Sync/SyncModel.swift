// This iPhone as a sync device (docs/phase4b-design.md §3, §10): the iOS
// counterpart of the desktop's `Device` (crates/ephemeris-desktop/src/sync.rs)
// and the browser's `webSync` (web/src/sync/web.ts).
//
// - The key is in the Keychain (`SyncKeyStore`), never in a file.
// - In the app's Application Support folder, `sync/config.json` holds the
//   replica id, a bookmark to the chosen folder and the last sync time
//   (nothing secret). `sync/local.ephsync` is the state, sealed with the key,
//   with complete file protection.
// - The folder is one the user picks in Files (iCloud Drive, or any
//   provider). Each device writes only its own `<replica>.ephsync`.
// - iOS never fetches from the bank (design §1.4): there is no fetcher
//   action and no ingest here.
// - Every merge rule is in Rust. Swift keeps the state as an opaque string
//   and passes it between the FFI's functions; it never decodes one.

import Foundation
import Observation

/// One device's file in the sync folder.
public struct SyncDeviceFile: Equatable, Sendable, Identifiable {
    public var file: String
    /// The replica that wrote it, when it opened.
    public var replica: String?
    public var thisDevice: Bool
    public var modified: Date?
    /// Why it was skipped (another key, damaged, not downloaded yet).
    public var error: String?
    public var id: String { file }
}

/// What the Sync sheet shows.
public struct SyncStatus: Equatable, Sendable {
    /// This device holds the sync key.
    public var paired: Bool
    /// This device's replica id, 16 hex digits.
    public var replica: String
    /// The chosen folder's name.
    public var folder: String?
    /// The device allowed to fetch from the bank, once one has.
    public var fetcher: String?
    public var isFetcher: Bool
    /// Bank data has arrived, so there is a synced feed to show.
    public var hasData: Bool
    public var fetchedThrough: IsoDate?
    public var devices: [SyncDeviceFile]
    public var lastSync: Date?
    public var error: String?
}

/// A saved "can I afford it" question, synced between devices.
public struct SavedQuery: Codable, Equatable, Sendable, Identifiable {
    public var id: String
    public var amountCents: Int64
    public var by: IsoDate
    public var label: String

    enum CodingKeys: String, CodingKey {
        case id, by, label
        case amountCents = "amount_cents"
    }
}

/// The synced feed and the forecast options the synced settings ask for.
public struct SyncedFeed: Equatable, Sendable {
    public var feedJSON: String
    public var optionsJSON: String
    public var through: IsoDate?
}

@MainActor
@Observable
public final class SyncModel {
    public static let keyNeeded = "no sync key yet: create one, or pair with another device's"

    public private(set) var status: SyncStatus
    public private(set) var queries: [SavedQuery] = []
    public private(set) var busy = false

    @ObservationIgnored private let directory: URL
    @ObservationIgnored private let keys: SyncKeyStore
    @ObservationIgnored private let random: @Sendable (Int) -> Data
    @ObservationIgnored private let now: @Sendable () -> Date
    @ObservationIgnored private let log: @Sendable (String) -> Void
    @ObservationIgnored private var config: Config
    @ObservationIgnored private var key: Data?
    @ObservationIgnored private var state: String?

    struct Config: Codable, Equatable {
        var replica: String
        /// A bookmark to the folder the user chose in Files.
        var folderBookmark: Data?
        var folderName: String?
        /// Milliseconds since 1970.
        var lastSync: Double?
    }

    /// The device whose files live in `directory`, creating its replica id
    /// on first use. A problem opening it is shown in the status.
    public init(
        directory: URL = URL.applicationSupportDirectory.appending(path: "sync"),
        keys: SyncKeyStore = KeychainKeyStore(),
        random: @escaping @Sendable (Int) -> Data = SyncModel.secureRandom,
        now: @escaping @Sendable () -> Date = { Date() },
        log: @escaping @Sendable (String) -> Void = SyncModel.stderr
    ) {
        self.directory = directory
        self.keys = keys
        self.random = random
        self.now = now
        self.log = log
        var problem: String?
        let configURL = directory.appending(path: "config.json")
        var config = Config(replica: Self.newReplica(random))
        do {
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            if FileManager.default.fileExists(atPath: configURL.path) {
                config = try JSONDecoder().decode(Config.self, from: Data(contentsOf: configURL))
            } else {
                try Self.writeProtected(JSONEncoder().encode(config), to: configURL)
            }
        } catch {
            problem = "this device's sync settings: \(errorMessage(error))"
        }
        self.config = config
        status = SyncStatus(
            paired: false, replica: config.replica, folder: config.folderName, fetcher: nil,
            isFetcher: false, hasData: false, fetchedThrough: nil, devices: [],
            lastSync: config.lastSync.map { Date(timeIntervalSince1970: $0 / 1000) }, error: problem)
        do {
            key = try keys.get()
            try loadState()
        } catch {
            status.error = errorMessage(error)
        }
        refresh()
    }

    // MARK: actions

    /// A new random key, for the first device.
    public func createKey() async {
        await act("create key") { try await self.setKey(self.random(32)) }
    }

    /// The key typed or pasted from another device.
    public func pair(text: String) async {
        await act("pair") {
            let key: Data
            do {
                key = try syncKeyFromText(text: text.trimmingCharacters(in: .whitespacesAndNewlines))
            } catch {
                throw SyncError(errorMessage(error))
            }
            try await self.setKey(key)
        }
    }

    /// The key as text, to pair another device.
    public func showKey() -> String? {
        do {
            guard let key else { throw SyncError(Self.keyNeeded) }
            let text = try syncKeyToText(key: key)
            log("[sync] show key: ok")
            return text
        } catch {
            fail("show key", error)
            return nil
        }
    }

    /// Sync through a folder picked in Files. The picker's URL is security
    /// scoped; a bookmark keeps access across launches.
    public func chooseFolder(_ url: URL) async {
        await act("choose folder \(url.lastPathComponent)") {
            let scoped = url.startAccessingSecurityScopedResource()
            defer { if scoped { url.stopAccessingSecurityScopedResource() } }
            var isDir: ObjCBool = false
            guard FileManager.default.fileExists(atPath: url.path, isDirectory: &isDir), isDir.boolValue else {
                throw SyncError("\(url.lastPathComponent) is not a folder this app can open")
            }
            let bookmark: Data
            do {
                bookmark = try url.bookmarkData(options: .minimalBookmark)
            } catch {
                throw SyncError("cannot keep access to \(url.lastPathComponent): \(error.localizedDescription)")
            }
            self.config.folderBookmark = bookmark
            self.config.folderName = url.lastPathComponent
            try self.saveConfig()
            try await self.syncFolder()
        }
    }

    /// Merge every device's file in the folder and write this device's. On
    /// launch, on becoming active and on pull to refresh (design §3.2). A
    /// failure is kept in the status and logged once, not thrown.
    public func syncNow() async {
        guard key != nil, config.folderBookmark != nil, !busy else { return }
        busy = true
        defer { busy = false }
        do {
            try await syncFolder()
            status.error = nil
        } catch {
            let message = errorMessage(error)
            if status.error != message { log("[sync] sync failed: \(message)") }
            status.error = message
        }
        refresh()
    }

    public func saveQuery(amountCents: Int64, by: IsoDate, label: String) async {
        let edit: [String: Any] = [
            "edit": "add_query", "query": ["amount_cents": amountCents, "by": by, "label": label],
        ]
        await act("save query") { try await self.change(edit) }
    }

    public func deleteQuery(id: String) async {
        await act("delete query") { try await self.change(["edit": "delete_query", "id": id]) }
    }

    /// Remove the key from this device, with this device's local copy and its
    /// folder choice. What is already in the folder stays there, readable by
    /// the other devices; edits not yet written to the folder are lost.
    public func forgetKey() async {
        await act("forget key") {
            try self.keys.delete()
            self.key = nil
            self.state = nil
            try? FileManager.default.removeItem(at: self.localURL)
            self.config.folderBookmark = nil
            self.config.folderName = nil
            self.config.lastSync = nil
            self.status.devices = []
            try self.saveConfig()
        }
    }

    /// The synced feed and options, once bank data has arrived.
    public func syncedFeed() throws -> SyncedFeed? {
        guard let state, status.hasData else { return nil }
        return SyncedFeed(
            feedJSON: try syncMaterialize(state: state),
            optionsJSON: try syncForecastOptions(state: state),
            through: status.fetchedThrough)
    }

    // MARK: the steps

    /// Run a user action: clear the last error, then keep a failure's message
    /// in the status (the sheet shows it) and log one line either way, as the
    /// desktop does.
    private func act(_ what: String, _ body: @escaping () async throws -> Void) async {
        busy = true
        defer { busy = false }
        status.error = nil
        do {
            try await body()
            log("[sync] \(what): ok")
        } catch {
            fail(what, error)
        }
        refresh()
    }

    private func fail(_ what: String, _ error: Error) {
        let message = errorMessage(error)
        log("[sync] \(what) failed: \(message)")
        status.error = message
    }

    private func setKey(_ new: Data) async throws {
        guard try keys.get() == nil, key == nil else {
            throw SyncError("this device already has a sync key")
        }
        try keys.set(new)
        key = new
        try loadState()
        try saveState()
        try await syncFolder()
    }

    /// Apply an edit, save, and write it to the folder.
    private func change(_ edit: [String: Any]) async throws {
        guard let state else { throw SyncError(Self.keyNeeded) }
        let json = String(decoding: try JSONSerialization.data(withJSONObject: edit), as: UTF8.self)
        let ms = UInt64(now().timeIntervalSince1970 * 1000)
        let out = try syncEdit(state: state, editJson: json, replica: config.replica, nowMs: ms)
        self.state = try Self.field(out, "state")
        try saveState()
        refresh()
        try await syncFolder()
    }

    /// The folder pass: resolve the bookmark, then read, merge and write off
    /// the main thread. The result is merged into the current state rather
    /// than replacing it, so an edit made meanwhile is kept (merging is
    /// commutative and idempotent).
    private func syncFolder() async throws {
        guard let bookmark = config.folderBookmark else { return }
        guard let key, let state else { throw SyncError(Self.keyNeeded) }
        var stale = false
        let dir: URL
        do {
            dir = try URL(resolvingBookmarkData: bookmark, bookmarkDataIsStale: &stale)
        } catch {
            throw SyncError("cannot open the sync folder \(config.folderName ?? "") again (\(error.localizedDescription)): choose it again")
        }
        let scoped = dir.startAccessingSecurityScopedResource()
        defer { if scoped { dir.stopAccessingSecurityScopedResource() } }
        if stale, let fresh = try? dir.bookmarkData(options: .minimalBookmark) {
            config.folderBookmark = fresh
        }
        let (replica, nonce) = (config.replica, random(24))
        let (merged, devices) = try await Task.detached(priority: .userInitiated) {
            try Self.pass(dir: dir, state: state, key: key, replica: replica, nonce: nonce)
        }.value
        self.state = try syncMerge(a: self.state ?? merged, b: merged)
        status.devices = devices
        config.lastSync = (now().timeIntervalSince1970 * 1000).rounded()
        try saveState()
        try saveConfig()
    }

    /// Read every device's file in `dir`, merge each that opens, and write
    /// this device's. A file that will not open or merge is reported in its
    /// row, never deleted.
    nonisolated static func pass(
        dir: URL, state: String, key: Data, replica: String, nonce: Data
    ) throws -> (String, [SyncDeviceFile]) {
        let own = "\(replica).\(SyncFolder.fileExtension)"
        var state = state
        var devices: [SyncDeviceFile] = []
        for entry in try SyncFolder.read(dir) {
            var row = SyncDeviceFile(
                file: entry.name, replica: nil, thisDevice: entry.name == own,
                modified: entry.modified, error: entry.error)
            if let bytes = entry.bytes {
                do {
                    let opened = try syncOpen(file: bytes, key: key)
                    row.replica = try field(opened, "replica")
                    state = try syncMerge(a: state, b: field(opened, "state"))
                } catch {
                    row.error = errorMessage(error)
                }
            }
            devices.append(row)
        }
        let sealed = try syncSeal(state: state, replica: replica, key: key, nonce: nonce)
        try SyncFolder.write(sealed, named: own, in: dir)
        // The folder was read before this device wrote: on the first sync,
        // list the file just written.
        if !devices.contains(where: \.thisDevice) {
            devices.append(SyncDeviceFile(file: own, replica: replica, thisDevice: true, modified: Date(), error: nil))
            devices.sort { $0.file < $1.file }
        }
        return (state, devices)
    }

    /// Open the local copy, or start an empty state, once there is a key.
    private func loadState() throws {
        guard let key else { return }
        if FileManager.default.fileExists(atPath: localURL.path) {
            do {
                state = try Self.field(syncOpen(file: Data(contentsOf: localURL), key: key), "state")
            } catch {
                throw SyncError("this device's saved sync state: \(errorMessage(error))")
            }
        } else {
            state = syncNew()
        }
    }

    private func saveState() throws {
        guard let key, let state else { return }
        let sealed = try syncSeal(state: state, replica: config.replica, key: key, nonce: random(24))
        try Self.writeProtected(sealed, to: localURL)
    }

    private func saveConfig() throws {
        try Self.writeProtected(JSONEncoder().encode(config), to: directory.appending(path: "config.json"))
    }

    private var localURL: URL { directory.appending(path: "local.ephsync") }

    /// The status and the saved queries from the current state.
    private func refresh() {
        status.paired = key != nil
        status.replica = config.replica
        status.folder = config.folderName
        status.lastSync = config.lastSync.map { Date(timeIntervalSince1970: $0 / 1000) }
        guard let state else {
            (status.fetcher, status.isFetcher, status.hasData, status.fetchedThrough) = (nil, false, false, nil)
            queries = []
            return
        }
        do {
            struct Summary: Decodable {
                var has_data: Bool
                var fetched_through: String?
                var fetcher: String?
            }
            let s = try JSONDecoder().decode(Summary.self, from: Data(syncSummary(state: state).utf8))
            status.fetcher = s.fetcher
            status.isFetcher = s.fetcher == config.replica
            status.hasData = s.has_data
            status.fetchedThrough = s.fetched_through
            queries = try JSONDecoder().decode([SavedQuery].self, from: Data(syncSavedQueries(state: state).utf8))
        } catch {
            status.error = errorMessage(error)
        }
    }

    // MARK: helpers

    /// A string field of a JSON object the FFI returned. A state comes back
    /// as a JSON string inside the object, so this hands back its exact text.
    nonisolated static func field(_ json: String, _ name: String) throws -> String {
        let object = try JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any]
        guard let value = object?[name] as? String else { throw SyncError("no \(name) in the sync result") }
        return value
    }

    nonisolated static func writeProtected(_ data: Data, to url: URL) throws {
        #if os(iOS)
        try data.write(to: url, options: [.atomic, .completeFileProtection])
        #else
        try data.write(to: url, options: .atomic)
        #endif
    }

    /// 16 hex digits, never zero (the low bit set, as on the other shells).
    nonisolated static func newReplica(_ random: (Int) -> Data) -> String {
        var bytes = [UInt8](random(8))
        bytes[7] |= 1
        return bytes.map { String(format: "%02x", $0) }.joined()
    }

    public nonisolated static func secureRandom(_ n: Int) -> Data {
        var g = SystemRandomNumberGenerator()
        return Data((0..<n).map { _ in UInt8.random(in: .min ... .max, using: &g) })
    }

    public nonisolated static func stderr(_ line: String) {
        FileHandle.standardError.write(Data((line + "\n").utf8))
    }
}
