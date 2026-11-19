const FORMER = "smart-review";
const CURRENT = "diffpilot";

/**
 * Moves what the browser stored under the project's former name (layout, drafts, collapsed panels) to the
 * current key names, once; a value already stored under the current name wins. `open` runs inside the guard,
 * because reading `window.localStorage` itself throws when site data is blocked.
 */
export function adoptFormerStorageKeys(open: () => Storage): void {
  try {
    const storage = open();
    const former = Object.keys(storage).filter((key) => /^smart-review[.:]/.test(key));
    for (const key of former) {
      const current = CURRENT + key.slice(FORMER.length);
      const value = storage.getItem(key);
      if (value !== null && storage.getItem(current) === null) storage.setItem(current, value);
      storage.removeItem(key);
    }
  } catch {
    // Private windows and blocked site data: the app works without stored preferences.
  }
}
