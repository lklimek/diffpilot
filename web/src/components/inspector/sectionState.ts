import { useState } from "react";

/** localStorage key of the open / collapsed state of the inspector sections. */
export const SECTION_STORAGE_KEY = "smart-review.inspector.sections";

/** Section key → open; keys missing here use their default. */
export type SectionState = Record<string, boolean>;

/** Open / collapsed state of the inspector sections, shared by every section of the panel. */
export interface SectionControl {
  isOpen: (key: string) => boolean;
  /** Opens or closes a section and stores that preference. */
  toggle: (key: string) => void;
  /** Opens a section for the current scope only (e.g. to show a flashed item); nothing is stored. */
  reveal: (key: string) => void;
}

// Brief §4.3: Questions and Assessment start collapsed; every other section starts open.
const CLOSED_BY_DEFAULT = new Set(["questions", "assessment"]);

/** Stored state; empty when storage is unavailable or holds something else. */
export function loadSectionState(): SectionState {
  try {
    const raw = window.localStorage.getItem(SECTION_STORAGE_KEY);
    const parsed: unknown = raw === null ? null : JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return {};
    return Object.fromEntries(
      Object.entries(parsed).filter((entry): entry is [string, boolean] => typeof entry[1] === "boolean"),
    );
  } catch {
    return {};
  }
}

/** Stores the state; without storage (disabled, private mode, quota) it lives for this page only. */
export function saveSectionState(state: SectionState): void {
  try {
    window.localStorage.setItem(SECTION_STORAGE_KEY, JSON.stringify(state));
  } catch {
    // Storage unavailable: the in-memory state still applies.
  }
}

/**
 * Section state kept in memory and mirrored to localStorage when it is available; sections revealed for
 * `scope` (e.g. the current block) close again when the scope changes.
 */
export function useSectionState(scope = ""): SectionControl {
  const [state, setState] = useState(loadSectionState);
  const [revealed, setRevealed] = useState<{ scope: string; keys: ReadonlySet<string> }>({
    scope,
    keys: new Set(),
  });
  if (revealed.scope !== scope) setRevealed({ scope, keys: new Set() });
  const isOpen = (key: string) =>
    (revealed.scope === scope && revealed.keys.has(key)) || (state[key] ?? !CLOSED_BY_DEFAULT.has(key));
  return {
    isOpen,
    toggle: (key) => {
      const next = { ...state, [key]: !isOpen(key) };
      saveSectionState(next);
      setState(next);
      setRevealed((r) => ({ scope: r.scope, keys: new Set([...r.keys].filter((k) => k !== key)) }));
    },
    reveal: (key) => setRevealed((r) => ({ scope: r.scope, keys: new Set(r.keys).add(key) })),
  };
}
