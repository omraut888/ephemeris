// Where this device keeps the sync key (docs/phase4b-design.md §3.3): the
// Keychain, never a file. The key is stored as the text people copy between
// devices, as the desktop app and the CLI store it on the Mac.

import Foundation
import Security

/// The sync key's home. `get` is nil until a key is created or paired.
public protocol SyncKeyStore: Sendable {
    func get() throws -> Data?
    func set(_ key: Data) throws
    func delete() throws
}

/// A sync failure the user sees, worded for the Sync sheet.
public struct SyncError: LocalizedError, Equatable, Sendable {
    public let message: String
    public init(_ message: String) { self.message = message }
    public var errorDescription: String? { message }
}

/// The iOS Keychain (on the Mac, the login keychain, for `swift test`).
///
/// - Accessible after the first unlock, on this device only: the key is not
///   restored from a backup onto another phone, and does not travel by
///   iCloud Keychain. Another device gets it by pairing.
/// - Service `ephemeris.sync`, account `key`, as on the Mac.
public struct KeychainKeyStore: SyncKeyStore {
    public let service: String
    public let account: String

    public init(service: String = "ephemeris.sync", account: String = "key") {
        self.service = service
        self.account = account
    }

    private var query: [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
    }

    public func get() throws -> Data? {
        var q = query
        q[kSecReturnData as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        var out: CFTypeRef?
        let status = SecItemCopyMatching(q as CFDictionary, &out)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess, let data = out as? Data else {
            throw SyncError("could not read the sync key from the Keychain: \(Self.reason(status))")
        }
        let text = String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
        do {
            return try syncKeyFromText(text: text)
        } catch {
            throw SyncError("the Keychain's sync key: \(errorMessage(error))")
        }
    }

    public func set(_ key: Data) throws {
        let text = try syncKeyToText(key: key)
        var add = query
        add[kSecValueData as String] = Data(text.utf8)
        add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        var status = SecItemAdd(add as CFDictionary, nil)
        if status == errSecDuplicateItem {
            status = SecItemUpdate(query as CFDictionary, [kSecValueData as String: Data(text.utf8)] as CFDictionary)
        }
        guard status == errSecSuccess else {
            throw SyncError("could not store the sync key in the Keychain: \(Self.reason(status))")
        }
        // Read it back, as the Mac's store does: a key that did not land is
        // an error now, not a lost key later.
        guard try get() == key else {
            throw SyncError("could not store the sync key in the Keychain: it did not read back")
        }
    }

    public func delete() throws {
        let status = SecItemDelete(query as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw SyncError("could not delete the sync key from the Keychain: \(Self.reason(status))")
        }
    }

    /// The Keychain's own words for a status, and its number.
    static func reason(_ status: OSStatus) -> String {
        let said = SecCopyErrorMessageString(status, nil) as String? ?? "unknown error"
        return "\(said) (\(status))"
    }
}

/// A key held in memory: tests, and nothing else.
public final class MemoryKeyStore: SyncKeyStore, @unchecked Sendable {
    private let lock = NSLock()
    private var key: Data?

    public init(_ key: Data? = nil) { self.key = key }

    public func get() throws -> Data? { lock.withLock { key } }
    public func set(_ key: Data) throws { lock.withLock { self.key = key } }
    public func delete() throws { lock.withLock { key = nil } }
}
