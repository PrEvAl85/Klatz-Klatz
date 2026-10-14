/** Layout discovered from the operating system. */
export interface LayoutInfo {
  /** Stable platform-specific identifier, e.g. `04090419` on Windows. */
  id: string;
  /** Human readable name shown in the UI. */
  name: string;
}

/** User configuration, persisted as JSON in the platform config directory. */
export interface Config {
  /** Enabled layouts in cycle order. */
  layouts: string[];
  hotkey: string;
  switchOnClick: boolean;
  autostart: boolean;
}

/** Snapshot returned by the `get_state` command. */
export interface AppState {
  /** All layouts the OS currently has available. */
  available: LayoutInfo[];
  /** Whether the platform supports runtime switching at all. */
  supported: boolean;
  /** Explanation shown to the user when `supported` is false. */
  unsupportedReason: string | null;
  /** Why the global hotkey is not registered, if registration failed. */
  hotkeyError: string | null;
  current: string | null;
  config: Config;
}