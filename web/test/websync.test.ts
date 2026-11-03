// @vitest-environment node
// The browser's sync host against the real sync WASM (docs/phase4b-design.md
// §10), with an in-memory folder standing in for the File System Access API.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { beforeAll, describe, expect, it } from "vitest";

import * as syncWasm from "../src/wasm/sync-pkg/ephemeris_sync_wasm.js";
import { forecastJson } from "../src/wasm/engine";
import { FOLDER_REFUSED, setSyncWasm, webSync } from "../src/sync/web";
import type { Directory, FileEntry, Writable } from "../src/sync/web";

beforeAll(() => {
  syncWasm.initSync({ module: readFileSync(resolve("src/wasm/sync-pkg/ephemeris_sync_wasm_bg.wasm")) });
  setSyncWasm(syncWasm);
});

const GOLDEN = resolve("../crates/ephemeris-sync/tests/golden");

/** A folder in memory, as the File System Access API presents one. */
class MemoryFolder implements Directory {
  name = "Ephemeris Sync";
  files = new Map<string, { bytes: Uint8Array; modified: number }>();
  async *values(): AsyncIterable<FileEntry> {
    for (const [name, f] of this.files) {
      yield {
        kind: "file",
        name,
        getFile: async () => new File([f.bytes as BlobPart], name, { lastModified: f.modified }),
      };
    }
  }
  async getFileHandle(name: string) {
    return {
      createWritable: async (): Promise<Writable> => {
        let data: Uint8Array = new Uint8Array();
        return {
          write: async (d) => {
            data = d;
          },
          close: async () => {
            this.files.set(name, { bytes: data, modified: 1_790_000_000_000 });
          },
        };
      },
    };
  }
}

/** Storage that records every write. */
function recordingStorage() {
  const items = new Map<string, string>();
  return {
    items,
    getItem: (k: string) => items.get(k) ?? null,
    setItem: (k: string, v: string) => void items.set(k, v),
  };
}

describe("webSync", () => {
  it("two browsers pair and converge through a folder; the key is never stored", async () => {
    const folder = new MemoryFolder();
    const storeA = recordingStorage();
    const a = webSync({ storage: storeA, pickDirectory: async () => folder, now: () => 1_000 });
    expect((await a.status()).paired).toBe(false);
    await a.createKey();
    let s = await a.chooseFolder();
    expect(s.folder).toBe("Ephemeris Sync");
    await a.saveQuery(150_000, "2026-11-15", "trip");
    const keyText = await a.showKey();

    const storeB = recordingStorage();
    const b = webSync({ storage: storeB, pickDirectory: async () => folder, now: () => 2_000 });
    await b.pair(keyText);
    s = await b.chooseFolder();
    expect(s.devices.map((d) => d.thisDevice)).toEqual(expect.arrayContaining([true, false]));
    expect(s.devices).toHaveLength(2);
    expect(await b.queries()).toEqual([
      { id: expect.stringMatching(/^[0-9a-f]{16}-1$/), amountCents: 150_000, by: "2026-11-15", label: "trip" },
    ]);
    await b.deleteQuery((await b.queries())[0]!.id);
    await a.syncNow();
    expect(await a.queries()).toEqual([]);

    // Only the replica id is kept; the key never is.
    for (const store of [storeA, storeB]) {
      expect([...store.items.keys()]).toEqual(["ephemeris.sync.replica"]);
      for (const v of store.items.values()) expect(v).not.toContain(keyText.slice(0, 9));
    }
    // A reload is the same device, and asks for the key again.
    const again = webSync({ storage: storeA, pickDirectory: async () => folder });
    const s2 = await again.status();
    expect(s2.replica).toBe((await a.status()).replica);
    expect(s2.paired).toBe(false);
  });

  it("imported golden replica files give the golden feed and report (the JavaScript leg)", async () => {
    const host = webSync({ storage: null });
    await host.pair(readFileSync(resolve(GOLDEN, "key.txt"), "utf8").trim());
    const file = (name: string) => new File([readFileSync(resolve(GOLDEN, name))], name);
    const s = await host.importFiles!([file("replica-a.ephsync"), file("replica-b.ephsync")]);
    expect(s.devices.every((d) => d.error === null)).toBe(true);
    expect(s.hasData).toBe(true);
    const synced = (await host.feed())!;
    expect(synced.feedJson).toBe(readFileSync(resolve(GOLDEN, "merged-feed.json"), "utf8"));
    expect(forecastJson(synced.feedJson, JSON.parse(synced.optionsJson))).toBe(
      readFileSync(resolve(GOLDEN, "merged-report.json"), "utf8"),
    );
    expect(s.fetcher).toBe("00000000000000aa");
    expect(s.isFetcher).toBe(false);
  });

  it("reports files it cannot open instead of failing, and exports a file that opens", async () => {
    const folder = new MemoryFolder();
    folder.files.set("0000000000000009.ephsync", { bytes: new Uint8Array([1, 2, 3]), modified: 5 });
    folder.files.set("notes.txt", { bytes: new Uint8Array([1]), modified: 5 });
    let exported: { name: string; bytes: Uint8Array } | null = null;
    const host = webSync({
      storage: null,
      pickDirectory: async () => folder,
      download: (name, bytes) => void (exported = { name, bytes }),
    });
    await host.createKey();
    const s = await host.chooseFolder();
    expect(s.error).toBeNull();
    const bad = s.devices.filter((d) => d.error);
    expect(bad).toHaveLength(1);
    expect(bad[0]!.error).toMatch(/not an Ephemeris sync file/);
    expect(folder.files.has("notes.txt")).toBe(true);

    await host.exportFile!();
    const e = exported as { name: string; bytes: Uint8Array } | null;
    expect(e?.name).toBe(`${s.replica}.ephsync`);
    const key = syncWasm.sync_key_from_text(await host.showKey());
    expect(JSON.parse(syncWasm.sync_open(e!.bytes, key)).replica).toBe(s.replica);
  });

  it("a folder the browser refuses gives a message pointing to import and export, which still work", async () => {
    for (const name of ["SecurityError", "NotAllowedError"]) {
      const host = webSync({
        storage: null,
        pickDirectory: async () => {
          throw new DOMException("It contains system files", name);
        },
        download: () => {},
      });
      await host.createKey();
      const err = await host.chooseFolder().then(
        () => null,
        (e: Error) => e.message,
      );
      expect(err).toContain(FOLDER_REFUSED);
      expect(err).toContain("It contains system files");
      expect(err).toMatch(/Import sync files…/);
      expect((await host.status()).folder).toBeNull();
      // The manual path is the way in.
      const golden = (n: string) => new File([readFileSync(resolve(GOLDEN, n))], n);
      const paired = webSync({ storage: null });
      await paired.pair(readFileSync(resolve(GOLDEN, "key.txt"), "utf8").trim());
      const s = await paired.importFiles!([golden("replica-a.ephsync"), golden("replica-b.ephsync")]);
      expect(s.hasData).toBe(true);
    }
  });

  it("cancelling the picker is not an error", async () => {
    const host = webSync({
      storage: null,
      pickDirectory: async () => {
        throw new DOMException("The user aborted a request.", "AbortError");
      },
    });
    await host.createKey();
    const s = await host.chooseFolder();
    expect(s.folder).toBeNull();
    expect(s.error).toBeNull();
  });

  it("write access refused after the pick shows the same message in the status", async () => {
    const folder = new MemoryFolder();
    folder.getFileHandle = async () => {
      throw new DOMException("Write permission denied", "NotAllowedError");
    };
    const host = webSync({ storage: null, pickDirectory: async () => folder });
    await host.createKey();
    const s = await host.chooseFolder();
    expect(s.error).toContain(FOLDER_REFUSED);
    expect(s.error).toContain("Write permission denied");
  });

  it("without a folder picker it offers no folder, and edits need a key", async () => {
    const host = webSync({ storage: null });
    expect(host.canChooseFolder).toBe(false);
    await expect(host.chooseFolder()).rejects.toThrow(/cannot open a folder/);
    await expect(host.saveQuery(1, "2026-01-01", "")).rejects.toThrow(/no sync key/);
    await expect(host.pair("not a key")).rejects.toThrow(/not a sync key/);
  });
});
