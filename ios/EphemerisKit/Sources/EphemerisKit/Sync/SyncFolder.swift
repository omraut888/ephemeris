// The sync folder (docs/phase4b-design.md §3.2): one `<replica>.ephsync` per
// device, each written only by its device. Every read and write goes through
// `NSFileCoordinator`, as iCloud Drive and other file providers require.
//
// A file iCloud has not downloaded yet is not read: its download is started
// and it is reported, so the next sync merges it. (Merging is idempotent, so
// a file read a sync later loses nothing.) It shows either as its own name
// with a "not downloaded" status, or as a `.<name>.icloud` placeholder.

import Foundation

public enum SyncFolder {
    public static let fileExtension = "ephsync"

    /// One `.ephsync` file found in the folder.
    public struct Entry: Equatable, Sendable {
        public var name: String
        public var modified: Date?
        /// The file's bytes, when it was read.
        public var bytes: Data?
        /// Why it was not read (not downloaded yet, unreadable).
        public var error: String?
    }

    public static let notDownloaded = "not downloaded from iCloud yet; its download has started, so sync again in a moment"

    /// Every device's file in `dir`, sorted by name. Other files, and names
    /// starting with a dot (temporary files), are ignored.
    public static func read(_ dir: URL) throws -> [Entry] {
        let keys: [URLResourceKey] = [
            .contentModificationDateKey, .isUbiquitousItemKey, .ubiquitousItemDownloadingStatusKey,
        ]
        // The listing is a coordinated read of the folder, so a provider can
        // bring it up to date first; each file is read after, on its own.
        var coordinationError: NSError?
        var listed: Result<[URL], Error> = .failure(SyncError("not listed"))
        NSFileCoordinator().coordinate(readingItemAt: dir, options: [], error: &coordinationError) { dir in
            listed = Result {
                try FileManager.default.contentsOfDirectory(at: dir, includingPropertiesForKeys: keys, options: [])
            }
        }
        let urls: [URL]
        do {
            if let coordinationError { throw coordinationError }
            urls = try listed.get()
        } catch {
            throw SyncError("cannot read the sync folder \(dir.lastPathComponent): \(error.localizedDescription)")
        }
        var found: [String: Entry] = [:]
        for url in urls {
            let name = url.lastPathComponent
            if let real = placeholderTarget(name) {
                // Older iCloud: `.<name>.icloud` stands in for a file not here yet.
                try? FileManager.default.startDownloadingUbiquitousItem(at: dir.appending(path: real))
                if found[real] == nil {
                    found[real] = Entry(name: real, modified: nil, bytes: nil, error: notDownloaded)
                }
                continue
            }
            guard url.pathExtension == fileExtension, !name.hasPrefix(".") else { continue }
            let values = try? url.resourceValues(forKeys: Set(keys))
            let modified = values?.contentModificationDate
            if values?.isUbiquitousItem == true,
               values?.ubiquitousItemDownloadingStatus == .notDownloaded {
                try? FileManager.default.startDownloadingUbiquitousItem(at: url)
                found[name] = Entry(name: name, modified: modified, bytes: nil, error: notDownloaded)
                continue
            }
            do {
                found[name] = Entry(name: name, modified: modified, bytes: try coordinatedRead(url), error: nil)
            } catch {
                found[name] = Entry(name: name, modified: modified, bytes: nil, error: "cannot read it: \(error.localizedDescription)")
            }
        }
        return found.values.sorted { $0.name < $1.name }
    }

    /// Write `bytes` as `name` in `dir`, replacing it whole: a reader sees the
    /// old file or the new one, never part of either.
    public static func write(_ bytes: Data, named name: String, in dir: URL) throws {
        let url = dir.appending(path: name)
        var coordinationError: NSError?
        var writeError: Error?
        NSFileCoordinator().coordinate(writingItemAt: url, options: .forReplacing, error: &coordinationError) { url in
            do {
                try bytes.write(to: url, options: .atomic)
            } catch {
                writeError = error
            }
        }
        if let e = coordinationError ?? writeError {
            throw SyncError("cannot write \(name) in the sync folder: \(e.localizedDescription)")
        }
    }

    static func coordinatedRead(_ url: URL) throws -> Data {
        var coordinationError: NSError?
        var result: Result<Data, Error> = .failure(SyncError("not read"))
        NSFileCoordinator().coordinate(readingItemAt: url, options: [], error: &coordinationError) { url in
            result = Result { try Data(contentsOf: url) }
        }
        if let coordinationError { throw coordinationError }
        return try result.get()
    }

    /// `X.ephsync` for an iCloud placeholder named `.X.ephsync.icloud`.
    static func placeholderTarget(_ name: String) -> String? {
        guard name.hasPrefix("."), name.hasSuffix(".\(fileExtension).icloud") else { return nil }
        let real = String(name.dropFirst().dropLast(".icloud".count))
        return real.hasPrefix(".") || real == ".\(fileExtension)" ? nil : real
    }
}
