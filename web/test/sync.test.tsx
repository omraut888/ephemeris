// The sync UI against a stand-in host (docs/phase4b-design.md §4): the
// Sync sheet, the synced data in the app, saved queries, and the desktop
// host's command mapping.

import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { App } from "../src/App";
import { SyncSheet } from "../src/components/SyncSheet";
import { tauriSync } from "../src/desktop/sync";
import type { Invoke } from "../src/desktop/shell";
import type { SavedQuery, SyncHost, SyncStatus } from "../src/sync/host";
import { directEngine } from "../src/wasm/engine";
import { FEED_JSON } from "./fixtures";

const ME = "00000000000000aa";

function status(over: Partial<SyncStatus> = {}): SyncStatus {
  return {
    paired: true,
    replica: ME,
    folder: "/Users/me/iCloud/Ephemeris",
    fetcher: "00000000000000bb",
    isFetcher: false,
    hasData: true,
    fetchedThrough: "2024-08-27",
    devices: [
      { file: `${ME}.ephsync`, replica: ME, thisDevice: true, modified: 1_790_000_000_000, error: null },
      {
        file: "00000000000000cc.ephsync",
        replica: null,
        thisDevice: false,
        modified: null,
        error: "this sync file is from another key, or damaged",
      },
    ],
    lastSync: 1_790_000_000_000,
    error: null,
    ...over,
  };
}

/** A host that keeps its state in memory, like the native one would. */
function fakeHost(kind: "desktop" | "web", start: SyncStatus): SyncHost & { s: SyncStatus } {
  let queries: SavedQuery[] = [];
  const host = {
    kind,
    s: start,
    canChooseFolder: kind === "desktop",
    status: vi.fn(async () => host.s),
    createKey: vi.fn(async () => (host.s = { ...host.s, paired: true })),
    pair: vi.fn(async (k: string) => {
      if (!k.startsWith("0000")) throw "that is not a sync key (52 characters, 0-9 and A-Z)";
      return (host.s = { ...host.s, paired: true });
    }),
    showKey: vi.fn(async () => "000G-40R4-0M30"),
    chooseFolder: vi.fn(async () => host.s),
    syncNow: vi.fn(async () => host.s),
    takeFetcher:
      kind === "desktop"
        ? vi.fn(async () => (host.s = { ...host.s, isFetcher: true, fetcher: ME }))
        : undefined,
    importFetch: kind === "desktop" ? vi.fn(async () => host.s) : undefined,
    importFiles: kind === "web" ? vi.fn(async () => host.s) : undefined,
    exportFile: kind === "web" ? vi.fn(async () => {}) : undefined,
    feed: vi.fn(async () =>
      host.s.hasData
        ? {
            feedJson: FEED_JSON,
            optionsJson: JSON.stringify({ horizon_days: 45, paths: 300, seed: 11, targets: [] }),
          }
        : null,
    ),
    queries: vi.fn(async () => queries),
    saveQuery: vi.fn(async (amountCents: number, by: string, label: string) => {
      queries = [...queries, { id: `${ME}-${queries.length + 1}`, amountCents, by, label }];
      return queries;
    }),
    deleteQuery: vi.fn(async (id: string) => (queries = queries.filter((q) => q.id !== id))),
  };
  return host as SyncHost & { s: SyncStatus };
}

function sheet(host: SyncHost, s: SyncStatus | null, onShowSynced = vi.fn()) {
  const onStatus = vi.fn();
  render(
    <SyncSheet
      host={host}
      status={s}
      onStatus={onStatus}
      onClose={vi.fn()}
      onShowSynced={onShowSynced}
    />,
  );
  return { onStatus, onShowSynced };
}

describe("SyncSheet", () => {
  it("pairs with a key, and shows the host's error for a bad one", async () => {
    const host = fakeHost("desktop", status({ paired: false }));
    const { onStatus } = sheet(host, host.s);
    expect(screen.getByRole("dialog", { name: "Sync" })).toBeTruthy();
    expect(screen.getByText(/Lose every copy of the key/)).toBeTruthy();
    const input = screen.getByLabelText("Sync key");
    await userEvent.type(input, "nope");
    await userEvent.click(screen.getByRole("button", { name: "Pair" }));
    expect((await screen.findByRole("alert")).textContent).toMatch(/not a sync key/);
    await userEvent.clear(input);
    await userEvent.type(input, "0000-1111");
    await userEvent.click(screen.getByRole("button", { name: "Pair" }));
    await waitFor(() => expect(onStatus).toHaveBeenCalledWith(expect.objectContaining({ paired: true })));
    expect(host.pair).toHaveBeenLastCalledWith("0000-1111");
  });

  it("creates a key on the first device", async () => {
    const host = fakeHost("desktop", status({ paired: false }));
    sheet(host, host.s);
    await userEvent.click(screen.getByRole("button", { name: "Create a new sync key" }));
    expect(host.createKey).toHaveBeenCalledOnce();
  });

  it("on the desktop: folder, devices with their errors, the key on request, and the fetcher", async () => {
    const host = fakeHost("desktop", status());
    const { onShowSynced } = sheet(host, host.s);
    expect(screen.getByText("/Users/me/iCloud/Ephemeris")).toBeTruthy();
    const devices = screen.getByRole("list", { name: "Devices" });
    expect(within(devices).getByText("This device")).toBeTruthy();
    expect(within(devices).getByText(/from another key/)).toBeTruthy();
    expect(screen.queryByTestId("sync-key")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Show key" }));
    expect((await screen.findByTestId("sync-key")).textContent).toBe("000G-40R4-0M30");
    expect(screen.getByText("Device 0000bb fetches from the bank.")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Import a Plaid fetch/ })).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Make this device the fetcher" }));
    expect(host.takeFetcher).toHaveBeenCalledOnce();
    await userEvent.click(screen.getByRole("button", { name: "Choose folder…" }));
    expect(host.chooseFolder).toHaveBeenCalledOnce();
    await userEvent.click(screen.getByRole("button", { name: "Show synced data" }));
    expect(onShowSynced).toHaveBeenCalledOnce();
  });

  it("offers the fetch import only to the fetcher", () => {
    const host = fakeHost("desktop", status({ isFetcher: true, fetcher: ME }));
    sheet(host, host.s);
    expect(screen.getByText("This device fetches from the bank.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Import a Plaid fetch…" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Make this device the fetcher" })).toBeNull();
  });

  it("in a browser without folder access: import and export, no fetcher actions", async () => {
    const host = fakeHost("web", status({ folder: null }));
    sheet(host, host.s);
    expect(screen.queryByRole("button", { name: "Choose folder…" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Make this device the fetcher" })).toBeNull();
    const file = new File([new Uint8Array([1, 2, 3])], "x.ephsync");
    await userEvent.upload(screen.getByTestId("sync-import"), file);
    await waitFor(() => expect(host.importFiles).toHaveBeenCalledWith([file]));
    await userEvent.click(screen.getByRole("button", { name: "Export this device's file" }));
    expect(host.exportFile).toHaveBeenCalledOnce();
  });
});

describe("App with sync", () => {
  it("shows the synced data with the synced settings' horizon, paths and seed", async () => {
    const host = fakeHost("desktop", status());
    const forecast = vi.spyOn(directEngine, "forecast");
    render(<App engine={directEngine} sync={host} />);
    await screen.findByText(/demo · realistic/);
    await userEvent.click(screen.getByRole("button", { name: "Sync" }));
    await userEvent.click(await screen.findByRole("button", { name: "Show synced data" }));
    expect(await screen.findByText("synced · through 2024-08-27")).toBeTruthy();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(forecast).toHaveBeenLastCalledWith(
      FEED_JSON,
      { horizon_days: 45, paths: 300, seed: 11, targets: [0] },
      "cone",
    );
    expect((screen.getByLabelText("Horizon") as HTMLSelectElement).value).toBe("45");
    forecast.mockRestore();
  });

  it("saves, reuses and deletes afford queries through the host", async () => {
    const host = fakeHost("desktop", status());
    render(<App engine={directEngine} sync={host} />);
    await screen.findByText(/demo · realistic/);
    const amount = await screen.findByLabelText("Amount");
    await userEvent.clear(amount);
    await userEvent.type(amount, "1500.25");
    await userEvent.type(screen.getByLabelText("Label"), "trip");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    const list = await screen.findByRole("list", { name: "Saved queries" });
    expect(host.saveQuery).toHaveBeenCalledWith(150025, expect.stringMatching(/^\d{4}-\d{2}-\d{2}$/), "trip");
    await userEvent.clear(amount);
    await userEvent.click(within(list).getByRole("button", { name: /\$1,500\.25 by .* · trip/ }));
    expect((amount as HTMLInputElement).value).toBe("1500.25");
    await userEvent.click(within(list).getByRole("button", { name: "Delete saved query trip" }));
    await waitFor(() => expect(screen.queryByRole("list", { name: "Saved queries" })).toBeNull());
  });

  it("has no Sync button and no saving without a sync host", async () => {
    render(<App engine={directEngine} />);
    await screen.findByText(/demo · realistic/);
    expect(screen.queryByRole("button", { name: "Sync" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
  });

  it("syncs when the window gains focus, while paired with a folder", async () => {
    const host = fakeHost("desktop", status());
    render(<App engine={directEngine} sync={host} />);
    await screen.findByText(/demo · realistic/);
    await waitFor(() => expect(host.status).toHaveBeenCalled());
    vi.spyOn(document, "hasFocus").mockReturnValue(true);
    await act(async () => {
      window.dispatchEvent(new Event("focus"));
    });
    await waitFor(() => expect(host.syncNow).toHaveBeenCalled());
    vi.restoreAllMocks();
  });
});

describe("tauriSync", () => {
  it("maps each call to its command with camelCase arguments", async () => {
    const call = vi.fn(async () => null) as unknown as Invoke;
    const host = tauriSync(call);
    await host.pair("KEY");
    await host.saveQuery(1000, "2026-12-01", "x");
    await host.deleteQuery("id1");
    await host.importFetch!();
    await host.takeFetcher!();
    expect(call).toHaveBeenCalledWith("sync_pair", { key: "KEY" });
    expect(call).toHaveBeenCalledWith("sync_save_query", {
      amountCents: 1000,
      by: "2026-12-01",
      label: "x",
    });
    expect(call).toHaveBeenCalledWith("sync_delete_query", { id: "id1" });
    expect(call).toHaveBeenCalledWith("sync_import_fetch");
    expect(call).toHaveBeenCalledWith("sync_take_fetcher");
    expect(host.kind).toBe("desktop");
  });
});
