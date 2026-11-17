// The fixture analysis as the mocks' server holds it: create, cancel and retry change it, a scripted run moves it
// planning → running k of n → finished, and every change is published to the open event streams.
import type { AnalysisState, AnalysisSummary } from "../api/generated/types";
import { seedNow } from "./fixtures";
import { flowStage, scenario } from "./scenarios";

/** Delay between two steps of the scripted run. */
export const RUN_STEP_MS = 400;

/** Interval of `heartbeat` events on an open event stream (sse-events.md). */
export const HEARTBEAT_MS = 15_000;

type Listener = (summary: AnalysisSummary) => void;

let stored: AnalysisSummary | null = null;
let timer: ReturnType<typeof setTimeout> | undefined;
let scriptPlayed = false;
let streamsOpened = 0;
const listeners = new Set<Listener>();

/** The analysis now: the scenario's pinned state, else the fixture seed, until something changes it. */
export function liveAnalysis(): AnalysisSummary {
  if (stored) return structuredClone(stored);
  const seed = seedNow("analysis") as AnalysisSummary;
  const pinned = scenario().analysis;
  const usageSeed = { ...seed, usage: scenario().usage ?? seed.usage };
  const summary = pinned ? { ...usageSeed, state: pinned, finished_at: null } : usageSeed;
  const progress = scenario().progress;
  return progress ? { ...summary, progress: progress(summary.state) } : summary;
}

/** Publishes a new state. */
export function setLiveState(state: AnalysisState): AnalysisSummary {
  const terminal = ["finished", "failed", "cancelled"].includes(state.state);
  stored = { ...liveAnalysis(), state, finished_at: terminal ? new Date().toISOString() : null };
  const progress = scenario().progress;
  if (progress) stored.progress = progress(state);
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
      // The runner takes flow k + 1 right away (the PR context ran before flow 1 in the real order).
      const next = k < total ? { stage: flowStage(k + 1, new Date().toISOString()) } : {};
      setLiveState({ state: "running", done: k, total, ...next });
      step(k + 1);
    }, RUN_STEP_MS);
  };
  step(done);
}

/** Publishes each state after its delay; the last one stays. */
export function playScript(states: readonly [number, () => AnalysisState][]): void {
  clearTimeout(timer);
  const [next, ...rest] = states;
  if (!next) return;
  timer = setTimeout(() => {
    setLiveState(next[1]());
    playScript(rest);
  }, next[0]);
}

/** `playScript` the first time only (the scenario's script, on the first event stream). */
export function playScriptOnce(states: readonly [number, () => AnalysisState][]): void {
  if (scriptPlayed) return;
  scriptPlayed = true;
  playScript(states);
}

/** Counts an opened event stream; returns how many were opened before it. */
export function openStream(): number {
  return streamsOpened++;
}

/** Stops the scripted run. */
export function stopRun(): void {
  clearTimeout(timer);
}

/** Back to the seed: no run, no stored state, no listeners (tests). */
export function resetLive(): void {
  stopRun();
  scriptPlayed = false;
  streamsOpened = 0;
  stored = null;
  listeners.clear();
}
