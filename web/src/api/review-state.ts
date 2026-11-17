// The user's review state (ticks, last viewed block) per analysis, outliving the screen: what a GET confirmed,
// local changes not yet saved, and the single save queue. Local changes win over reads; nothing is written
// before a GET has read the stored state of that analysis.
import { useCallback, useEffect, useSyncExternalStore } from "react";
import type { AnalysisId, ReviewState } from "./generated/types";
import { api } from "./client";

/** What the screen sees of one analysis's review state. */
export interface ReviewStateView {
  /** The state to show: local changes over the last confirmed read; empty before any. */
  state: ReviewState;
  /** `loading` until the first read for this analysis finished; `ready` after a successful read. */
  status: "loading" | "ready" | "unreadable";
  /** Why the stored state could not be read (status `unreadable`): changes stay on this page. */
  loadError: unknown;
  /** The last failed save; null once a save succeeded. */
  saveError: unknown;
}

interface Entry {
  /** Last state a GET or a successful PUT confirmed; null before a GET succeeded. */
  confirmed: ReviewState | null;
  /** A GET succeeded for this analysis: saving is allowed. */
  readOnce: boolean;
  /** Local state not yet confirmed by a PUT; null when none. */
  local: ReviewState | null;
  /** Bumped by every local change; a read started before a change is ignored. */
  version: number;
  /** Newest state waiting for the queue; null when none. */
  next: ReviewState | null;
  running: boolean;
  reading: boolean;
  loadError: unknown;
  saveError: unknown;
  view: ReviewStateView;
  listeners: Set<() => void>;
}

const EMPTY: ReviewState = { ticked_steps: [], last_viewed: null };

/** The calls the store needs (the app's API client in production). */
export type ReviewStateClient = Pick<typeof api, "getReviewState" | "putReviewState">;

/** A store of review states per analysis over `client`. */
export function createReviewStateStore(client: ReviewStateClient) {
  const entries = new Map<AnalysisId, Entry>();

  const entry = (id: AnalysisId): Entry => {
    let e = entries.get(id);
    if (!e) {
      e = {
        confirmed: null,
        readOnce: false,
        local: null,
        version: 0,
        next: null,
        running: false,
        reading: false,
        loadError: null,
        saveError: null,
        view: { state: EMPTY, status: "loading", loadError: null, saveError: null },
        listeners: new Set(),
      };
      entries.set(id, e);
    }
    return e;
  };

  const publish = (e: Entry) => {
    e.view = {
      state: e.local ?? e.confirmed ?? EMPTY,
      status: e.readOnce ? "ready" : e.loadError !== null ? "unreadable" : "loading",
      loadError: e.readOnce ? null : e.loadError,
      saveError: e.saveError,
    };
    for (const listener of e.listeners) listener();
  };

  // One queue per analysis: requests in order, only the newest waiting state is sent.
  const drain = async (id: AnalysisId, e: Entry) => {
    if (e.running) return;
    e.running = true;
    while (e.next) {
      const sent = e.next;
      e.next = null;
      try {
        await client.putReviewState(id, sent);
        e.confirmed = sent;
        e.saveError = null;
        if (e.next === null && e.local === sent) e.local = null;
      } catch (error) {
        e.saveError = error;
      }
      publish(e);
    }
    e.running = false;
  };

  return {
    /** The current view (stable until the next change). */
    view: (id: AnalysisId): ReviewStateView => entry(id).view,

    /** Calls `listener` on every change of the analysis's view; returns the unsubscribe function. */
    subscribe(id: AnalysisId, listener: () => void): () => void {
      const e = entry(id);
      e.listeners.add(listener);
      return () => e.listeners.delete(listener);
    },

    /** Reads the stored state; a result is ignored when a local change happened while it was read. */
    async read(id: AnalysisId): Promise<void> {
      const e = entry(id);
      if (e.reading) return;
      e.reading = true;
      if (!e.readOnce && e.loadError !== null) {
        // Retrying the first read: loading again, so the screen opens on what it finds.
        e.loadError = null;
        publish(e);
      }
      const version = e.version;
      try {
        const stored = await client.getReviewState(id);
        if (!e.readOnce) {
          // Changes made before the first read were never saved and rest on an unknown state.
          e.readOnce = true;
          e.local = null;
          e.confirmed = stored;
        } else if (e.version === version && e.local === null && !e.running) {
          e.confirmed = stored;
        }
      } catch (error) {
        if (!e.readOnce) e.loadError = error;
      } finally {
        e.reading = false;
      }
      publish(e);
    },

    /** Applies a local change; saved when the stored state was read, else kept on this page only. */
    change(id: AnalysisId, update: (state: ReviewState) => ReviewState): void {
      const e = entry(id);
      const next = update(e.view.state);
      e.version += 1;
      e.local = next;
      if (e.readOnce) {
        e.next = next;
        void drain(id, e);
      }
      publish(e);
    },

    /** Forgets every analysis (tests). */
    reset(): void {
      entries.clear();
    },
  };
}

/** The app's review-state store. */
export const reviewStates = createReviewStateStore(api);

/** The review state of an analysis; reads it again on every mount. */
export function useReviewStateStore(id: AnalysisId) {
  const view = useSyncExternalStore(
    (listener) => reviewStates.subscribe(id, listener),
    () => reviewStates.view(id),
  );
  useEffect(() => {
    void reviewStates.read(id);
  }, [id]);
  const change = useCallback(
    (update: (state: ReviewState) => ReviewState) => reviewStates.change(id, update),
    [id],
  );
  return { ...view, change };
}
