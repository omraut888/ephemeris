// The Sync sheet (docs/phase4b-design.md §4.4): pair this device, choose the
// folder, see each device's file, and, on the desktop, take the fetcher
// role and import a fetch. Everything goes through the shell's `SyncHost`.

import { useState } from "react";
import type { FormEvent, ReactNode } from "react";

import type { SyncHost, SyncStatus } from "../sync/host";

interface Props {
  host: SyncHost;
  status: SyncStatus | null;
  onStatus: (s: SyncStatus) => void;
  onClose: () => void;
  /** Show the synced data in the app. */
  onShowSynced: () => void;
}

const message = (e: unknown) =>
  typeof e === "string" ? e : e instanceof Error ? e.message : String(e);

const short = (replica: string | null) => (replica ? replica.slice(-6) : "?");

function when(ms: number | null): string {
  if (ms === null) return "never";
  return new Date(ms).toLocaleString("en-US", {
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  });
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="sheet-row">
      <div className="label">{label}</div>
      <div>{children}</div>
    </div>
  );
}

export function SyncSheet({ host, status, onStatus, onClose, onShowSynced }: Props) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [keyText, setKeyText] = useState("");
  const [shownKey, setShownKey] = useState<string | null>(null);

  const act = async (f: () => Promise<SyncStatus | void>) => {
    setBusy(true);
    setError(null);
    try {
      const s = await f();
      if (s) onStatus(s);
    } catch (e) {
      setError(message(e));
    } finally {
      setBusy(false);
    }
  };

  const pair = (e: FormEvent) => {
    e.preventDefault();
    void act(() => host.pair(keyText.trim()));
  };

  const shownError = error ?? status?.error ?? null;

  return (
    <div className="sheet-backdrop" onClick={onClose}>
      <section
        className="sheet"
        role="dialog"
        aria-modal="true"
        aria-label="Sync"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="sheet-head">
          <h2>Sync</h2>
          <button type="button" aria-label="Close" onClick={onClose}>
            ×
          </button>
        </div>
        {shownError && (
          <p className="sheet-error" role="alert">
            {shownError}
          </p>
        )}
        {!status ? (
          <p className="dim">Loading…</p>
        ) : !status.paired ? (
          <div className="sheet-body">
            <p className="dim">
              Your devices share one encrypted file each in a folder you already sync (iCloud
              Drive, Dropbox). Only devices with the sync key can read them; no server ever
              can. Lose every copy of the key and your overrides, settings and saved queries
              are gone; bank data can be fetched again.
            </p>
            <form onSubmit={pair} className="sheet-pair">
              <input
                aria-label="Sync key"
                placeholder="Paste the key from another device"
                value={keyText}
                autoComplete="off"
                spellCheck={false}
                onChange={(e) => setKeyText(e.target.value)}
              />
              <button type="submit" disabled={busy || !keyText.trim()}>
                Pair
              </button>
            </form>
            <p className="dim">
              First device?{" "}
              <button type="button" disabled={busy} onClick={() => void act(host.createKey)}>
                Create a new sync key
              </button>
            </p>
            {host.kind === "web" && (
              <p className="dim">This browser keeps the key only until you close the tab.</p>
            )}
          </div>
        ) : (
          <div className="sheet-body">
            <Row label="Key">
              {shownKey ? (
                <>
                  <code className="sync-key" data-testid="sync-key">
                    {shownKey}
                  </code>
                  <div className="dim">Enter it on your other devices. Keep it private.</div>
                </>
              ) : (
                <button
                  type="button"
                  disabled={busy}
                  onClick={() => void act(async () => setShownKey(await host.showKey()))}
                >
                  Show key
                </button>
              )}
            </Row>
            <Row label="Folder">
              <div>{status.folder ?? <span className="dim">none chosen</span>}</div>
              {host.canChooseFolder ? (
                <button type="button" disabled={busy} onClick={() => void act(host.chooseFolder)}>
                  Choose folder…
                </button>
              ) : (
                <div className="dim">
                  This browser cannot keep a folder open. Import the files from your sync
                  folder, and export this device's file back into it.
                  <div className="sheet-actions">
                    <label className="button">
                      Import sync files…
                      <input
                        type="file"
                        multiple
                        accept=".ephsync"
                        hidden
                        data-testid="sync-import"
                        onChange={(e) => {
                          const files = [...(e.target.files ?? [])];
                          e.target.value = "";
                          if (files.length && host.importFiles) {
                            void act(() => host.importFiles!(files));
                          }
                        }}
                      />
                    </label>
                    <button
                      type="button"
                      disabled={busy}
                      onClick={() => void act(() => host.exportFile!())}
                    >
                      Export this device's file
                    </button>
                  </div>
                </div>
              )}
            </Row>
            <Row label="Status">
              <div>
                Last synced {when(status.lastSync)}{" "}
                <button type="button" disabled={busy} onClick={() => void act(host.syncNow)}>
                  Sync now
                </button>
              </div>
              {status.devices.length > 0 && (
                <ul className="list sync-devices" aria-label="Devices">
                  {status.devices.map((d) => (
                    <li key={d.file}>
                      <span>
                        {d.thisDevice ? "This device" : `Device ${short(d.replica)}`}
                        {d.error && <span className="sheet-error"> · {d.error}</span>}
                      </span>
                      <span className="dim">{when(d.modified)}</span>
                    </li>
                  ))}
                </ul>
              )}
            </Row>
            <Row label="Bank data">
              <div>
                {status.hasData
                  ? `Fetched through ${status.fetchedThrough}`
                  : "None yet: the fetching device has not synced any."}
              </div>
              <div className="dim">
                {status.isFetcher
                  ? "This device fetches from the bank."
                  : status.fetcher
                    ? `Device ${short(status.fetcher)} fetches from the bank.`
                    : "No device fetches yet."}
              </div>
              <div className="sheet-actions">
                {host.takeFetcher && !status.isFetcher && (
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => void act(host.takeFetcher!)}
                  >
                    Make this device the fetcher
                  </button>
                )}
                {host.importFetch && status.isFetcher && (
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => void act(host.importFetch!)}
                  >
                    Import a Plaid fetch…
                  </button>
                )}
              </div>
            </Row>
            {status.hasData && (
              <button type="button" className="primary" onClick={onShowSynced}>
                Show synced data
              </button>
            )}
          </div>
        )}
      </section>
    </div>
  );
}
