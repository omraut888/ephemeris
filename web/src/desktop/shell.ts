// The desktop app's engine and file host, over Tauri's IPC
// (docs/phase4-design.md §2–3). Only the desktop build imports this
// module, so the web bundle has no Tauri code.
//
// The engine is the web client's `portEngine` with a port that sends the
// worker's `Request`s as Tauri commands instead of messages, so lanes and
// supersession work the same in both shells.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import type { DropHandlers, FileHost, OpenedFeed, RecentFeed } from "../files";
import { portEngine } from "../wasm/client";
import type { Engine, Port } from "../wasm/client";
import type { Request, Response } from "../wasm/protocol";
import type { SyncHost } from "../sync/host";
import { tauriSync } from "./sync";

export type Invoke = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;
export type Listen = <T>(
  event: string,
  handler: (e: { payload: T }) => void,
) => Promise<() => void>;

/** Tauri rejects with the command's error value, a string from Rust. */
const message = (e: unknown) =>
  typeof e === "string" ? e : e instanceof Error ? e.message : String(e);

function command(req: Request): [string, Record<string, unknown>] {
  return req.op === "simulate"
    ? ["simulate", { optionsJson: JSON.stringify(req.options) }]
    : ["forecast", { feedJson: req.feedJson, optionsJson: JSON.stringify(req.options) }];
}

/** A `Port` whose messages are Tauri commands to the native engine. */
export function tauriPort(call: Invoke = invoke): Port {
  const port: Port = {
    onmessage: null,
    postMessage(req) {
      const reply = (res: Response) => port.onmessage?.({ data: res } as MessageEvent<Response>);
      const [cmd, args] = command(req);
      call<string>(cmd, args).then(
        (json) => reply({ id: req.id, ok: true, json }),
        (e: unknown) => reply({ id: req.id, ok: false, error: message(e) }),
      );
    },
  };
  return port;
}

/** Event names, as in the Rust crate's `events` module. */
const FEED_OPENED = "feed-opened";
const FEED_ERROR = "feed-error";
const FEED_HOVER = "feed-hover";

export function tauriFiles(call: Invoke = invoke, on: Listen = listen): FileHost {
  return {
    pick: () => call<OpenedFeed | null>("pick_feed"),
    recent: () => call<RecentFeed[]>("recent_feeds"),
    openRecent: (path) => call<OpenedFeed>("open_recent", { path }),
    onDrop(h: DropHandlers) {
      const subs = [
        on<OpenedFeed>(FEED_OPENED, (e) => h.opened(e.payload)),
        on<string>(FEED_ERROR, (e) => h.error(e.payload)),
        on<boolean>(FEED_HOVER, (e) => h.hover(e.payload)),
      ];
      return () => {
        for (const s of subs) void s.then((unlisten) => unlisten());
      };
    },
  };
}

export function desktopShell(): { engine: Engine; files: FileHost; sync: SyncHost } {
  return { engine: portEngine(tauriPort()), files: tauriFiles(), sync: tauriSync() };
}
