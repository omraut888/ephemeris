// Native file handling, when the shell has it (the desktop app,
// docs/phase4-design.md §3). The web app has none and uses the browser's
// file input and drop events instead.

/** A feed the shell read from disk. */
export interface OpenedFeed {
  name: string;
  path: string;
  json: string;
}

/** A feed opened before. */
export interface RecentFeed {
  name: string;
  path: string;
}

export interface DropHandlers {
  opened(feed: OpenedFeed): void;
  error(message: string): void;
  /** A file is dragged over the window (true) or has left it (false). */
  hover(over: boolean): void;
}

export interface FileHost {
  /** The native open dialog; null when cancelled. */
  pick(): Promise<OpenedFeed | null>;
  /** Recent feeds, newest first. */
  recent(): Promise<RecentFeed[]>;
  /** Open a feed from the recent list. */
  openRecent(path: string): Promise<OpenedFeed>;
  /** Listen for feeds dropped on the window; returns the unsubscribe. */
  onDrop(handlers: DropHandlers): () => void;
}
