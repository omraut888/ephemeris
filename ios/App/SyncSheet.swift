// The Sync sheet (docs/phase4b-design.md §4.4; web/src/components/
// SyncSheet.tsx): pair this iPhone, choose the folder, see each device's
// file and who fetches. No fetcher actions: iOS never fetches (§1.4).

import EphemerisKit
import SwiftUI
import UIKit
import UniformTypeIdentifiers

struct SyncSheet: View {
    let sync: SyncModel
    /// Show the synced data in the app (the sheet closes first).
    let onShowSynced: () -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var keyText = ""
    @State private var shownKey: String?
    @State private var choosingFolder = false
    @State private var confirmForget = false

    private var status: SyncStatus { sync.status }

    var body: some View {
        NavigationStack {
            Form {
                if let error = status.error {
                    Section {
                        Text(error)
                            .font(.callout)
                            .foregroundStyle(Theme.bad)
                            .textSelection(.enabled)
                            .accessibilityIdentifier("sync-error")
                    }
                }
                if status.paired { paired } else { unpaired }
            }
            .navigationTitle("Sync")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
            .disabled(sync.busy)
            .fileImporter(isPresented: $choosingFolder, allowedContentTypes: [.folder]) { result in
                if case let .success(url) = result {
                    Task { await sync.chooseFolder(url) }
                }
            }
            .confirmationDialog("Remove the sync key from this iPhone?", isPresented: $confirmForget, titleVisibility: .visible) {
                Button("Remove the key", role: .destructive) {
                    shownKey = nil
                    Task { await sync.forgetKey() }
                }
            } message: {
                Text("This iPhone stops syncing. Its file stays in the folder for your other devices; edits not yet written there are lost. Pair again with the key from another device.")
            }
        }
    }

    @ViewBuilder private var unpaired: some View {
        Section {
            Text("Your devices share one encrypted file each in a folder you already sync (iCloud Drive, Dropbox). Only devices with the sync key can read them; no server ever can. Lose every copy of the key and your overrides, settings and saved queries are gone; bank data can be fetched again.")
                .font(.callout)
                .foregroundStyle(Theme.dim)
        }
        Section("Pair with another device") {
            TextField("Paste the key from another device", text: $keyText, axis: .vertical)
                .font(.body.monospaced())
                .textInputAutocapitalization(.characters)
                .autocorrectionDisabled()
                .accessibilityIdentifier("sync-key-field")
            Button("Pair") {
                let text = keyText
                Task {
                    await sync.pair(text: text)
                    if sync.status.paired { keyText = "" }
                }
            }
            .disabled(keyText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
        }
        Section {
            Button("Create a new sync key") { Task { await sync.createKey() } }
        } header: {
            Text("First device?")
        }
    }

    @ViewBuilder private var paired: some View {
        Section("Key") {
            if let shownKey {
                Text(shownKey)
                    .font(.body.monospaced())
                    .textSelection(.enabled)
                    .accessibilityIdentifier("sync-key")
                Button("Copy", systemImage: "doc.on.doc") { UIPasteboard.general.string = shownKey }
                Text("Enter it on your other devices. Keep it private.")
                    .font(.caption)
                    .foregroundStyle(Theme.dim)
            } else {
                Button("Show key") { shownKey = sync.showKey() }
            }
        }
        Section("Folder") {
            if let folder = status.folder {
                Text(folder)
            } else {
                Text("none chosen").foregroundStyle(Theme.dim)
            }
            Button("Choose folder…") { choosingFolder = true }
        }
        Section("Status") {
            HStack {
                Text("Last synced \(status.lastSync.map { $0.formatted(date: .abbreviated, time: .shortened) } ?? "never")")
                Spacer()
                Button("Sync now") { Task { await sync.syncNow() } }
                    .disabled(status.folder == nil)
            }
            ForEach(status.devices) { d in
                HStack(alignment: .firstTextBaseline) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(d.thisDevice ? "This device" : "Device \(short(d.replica))")
                        if let error = d.error {
                            Text(error).font(.caption).foregroundStyle(Theme.bad)
                        }
                    }
                    Spacer()
                    Text(d.modified.map { $0.formatted(date: .abbreviated, time: .shortened) } ?? "")
                        .font(.caption)
                        .foregroundStyle(Theme.dim)
                }
                .accessibilityElement(children: .combine)
            }
        }
        Section {
            Text(status.hasData
                 ? "Fetched through \(status.fetchedThrough ?? "?")"
                 : "None yet: the fetching device has not synced any.")
            Text(status.isFetcher
                 ? "This device fetches from the bank."
                 : status.fetcher.map { "Device \(short($0)) fetches from the bank." } ?? "No device fetches yet.")
                .foregroundStyle(Theme.dim)
                .accessibilityIdentifier("sync-fetcher")
            if status.hasData {
                Button("Show synced data") {
                    dismiss()
                    onShowSynced()
                }
                .bold()
            }
        } header: {
            Text("Bank data")
        } footer: {
            Text("This iPhone never fetches from the bank: the Mac does, and its data arrives here through the folder.")
        }
        Section {
            Button("Remove the key from this iPhone…", role: .destructive) { confirmForget = true }
        }
    }

    /// The last six hex digits of a replica id, as the other shells show it.
    private func short(_ replica: String?) -> String {
        replica.map { String($0.suffix(6)) } ?? "?"
    }
}
