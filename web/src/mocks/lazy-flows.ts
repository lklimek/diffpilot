// Pending-flow scenario: accepted fixture content becomes available after a requested job completes.
import type { FlowAnalysisStatus, Tour } from "../api/generated/types";
import { liveAnalysis, setLiveState } from "./live-analysis";
const states = new Map<string, FlowAnalysisStatus>();
export function lazyTour(seed: Tour): Tour {
  const tour = structuredClone(seed);
  tour.analysis = liveAnalysis();
  for (const flow of tour.flows) {
    flow.planned = {
      intent: `Plan: ${flow.llm.title}`,
      importance_rank: flow.position,
      ranges: flow.steps.map((s) => ({ file: s.facts.file, side: s.facts.side, ranges: [s.facts.range] })),
    };
    flow.analysis_status = states.get(flow.id) ?? {
      state: flow.position === 1 ? "analysed" : "not_analysed",
      started_at: null,
      message: null,
    };
    if (flow.analysis_status.state !== "analysed") {
      flow.steps = [];
      flow.graph = { nodes: [], edges: [] };
      flow.llm.risk = null;
      flow.facts.step_count = 0;
    }
  }
  return tour;
}
const timers = new Set<ReturnType<typeof setTimeout>>();
const later = (ms: number, run: () => void) => {
  const timer = setTimeout(() => {
    timers.delete(timer);
    run();
  }, ms);
  timers.add(timer);
};

/** Back to every flow pending (tests). */
export function resetLazyFlows(): void {
  for (const timer of timers) clearTimeout(timer);
  timers.clear();
  states.clear();
}

export function requestLazyFlow(id: string) {
  if (states.get(id)?.state === "analysed" || states.get(id)?.state === "analysing") return;
  states.set(id, { state: "queued", started_at: null, message: null });
  setLiveState(liveAnalysis().state);
  later(1_000, () => {
    states.set(id, { state: "analysing", started_at: new Date().toISOString(), message: null });
    setLiveState(liveAnalysis().state);
    later(70_000, () => {
      states.set(id, { state: "analysed", started_at: null, message: null });
      setLiveState({ state: "running", done: 2, total: 5 });
    });
  });
}
