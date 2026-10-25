// Brand, the current data source, and the two ways to get a feed: a demo
// generated in WASM, or a feed JSON file (picked here, or dropped anywhere).

import { useRef } from "react";

import type { SimMode } from "../schema.gen";

export type Source =
  | { kind: "demo"; mode: SimMode; seed: number }
  | { kind: "file"; name: string };

export type Status = "loading" | "ready" | "error";

interface Props {
  source: Source | null;
  status: Status;
  mode: SimMode;
  seed: number;
  onMode: (m: SimMode) => void;
  onSeed: (s: number) => void;
  onDemo: () => void;
  onFile: (f: File) => void;
}

function Mark() {
  return (
    <svg width="18" height="18" viewBox="0 0 32 32" aria-hidden="true">
      <path d="M2 16 L30 5 L30 27 Z" fill="currentColor" opacity="0.35" />
      <path d="M2 16 L30 11 L30 21 Z" fill="currentColor" opacity="0.7" />
      <path d="M2 16 L30 16" stroke="currentColor" strokeWidth="2" />
    </svg>
  );
}

function describe(source: Source | null): string {
  if (!source) return "no data";
  return source.kind === "demo"
    ? `demo · ${source.mode} · seed ${source.seed}`
    : `file · ${source.name}`;
}

export function Header({ source, status, mode, seed, onMode, onSeed, onDemo, onFile }: Props) {
  const fileInput = useRef<HTMLInputElement>(null);
  return (
    <header className="header">
      <div className="brand">
        <Mark />
        EPHEMERIS
      </div>
      <div className="source" data-testid="source">
        <span
          className={`dot${status === "loading" ? " busy" : status === "error" ? " error" : ""}`}
          aria-hidden="true"
        />
        {status === "loading" ? "computing…" : describe(source)}
      </div>
      <div className="controls">
        <select
          aria-label="Demo mode"
          value={mode}
          onChange={(e) => onMode(e.target.value as SimMode)}
        >
          <option value="realistic">realistic</option>
          <option value="matched">model-matched</option>
        </select>
        <input
          type="number"
          aria-label="Demo seed"
          min={0}
          step={1}
          value={seed}
          onChange={(e) => onSeed(Math.max(0, Math.floor(Number(e.target.value) || 0)))}
        />
        <button type="button" onClick={onDemo}>
          Demo
        </button>
        <button type="button" onClick={() => fileInput.current?.click()}>
          Load feed…
        </button>
        <input
          ref={fileInput}
          type="file"
          accept="application/json,.json"
          hidden
          data-testid="file-input"
          onChange={(e) => {
            const f = e.target.files?.[0];
            if (f) onFile(f);
            e.target.value = "";
          }}
        />
      </div>
    </header>
  );
}
