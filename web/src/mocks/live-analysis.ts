// The fixture analysis as the mocks' server holds it: create, cancel and retry change it, a scripted run moves it
// planning → running k of n → finished, and every change is published to the open event streams.
import type { AnalysisState, AnalysisSummary } from "../api/generated/types";
import { seedNow } from "./fixtures";
import { scenario } from "./scenarios";

/** Delay between two steps of the scripted run. */
export const RUN_STEP_MS = 400;

type Listener = (summary: AnalysisSummary) => void;

let stored: AnalysisSummary | null = null;
let timer: ReturnType<typeof setTimeout> | undefined;
const listeners = new Set<Listener>();

/** The analysis now: the scenario's pinned state, else the fixture seed, until something changes it. */
export function liveAnalysis(): AnalysisSummary {
  if (stored) return structuredClone(stored);
  const seed = seedNow("analysis") as AnalysisSummary;
  const pinned = scenario().analysis;
  return pinned ? { ...seed, state: pinned, finished_at: null } : seed;
}

/** Publishes a new state. */
export function setLiveState(state: AnalysisState): AnalysisSummary {
  const terminal = ["finished", "failed", "cancelled"].includes(state.state);
  stored = { ...liveAnalysis(), state, finished_at: terminal ? new Date().toISOString() : null };
  for (const listener of listeners) listener(structuredClone(stored));
  return liveAnalysis();
}

/** Calls `listener` on every change until the returned function is called. */
export function subscribeLive(listener: Listener): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** Runs the analysis from `done` of `total` flows to finished, one step per `RUN_STEP_MS`. */
export function startRun(done = 0, total = 5): void {
  clearTimeout(timer);
  const step = (k: number) => {
    timer = setTimeout(() => {
      if (k > total) {
        setLiveState({ state: "finished", total });
        return;
      }
      setLiveState({ state: "running", done: k, total });
      step(k + 1);
    }, RUN_STEP_MS);
  };
  step(done);
}

/** Stops the scripted run. */
export function stopRun(): void {
  clearTimeout(timer);
}

/** Back to the seed: no run, no stored state, no listeners (tests). */
export function resetLive(): void {
  stopRun();
  stored = null;
  listeners.clear();
}
