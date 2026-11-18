// Pending-flow scenario: accepted fixture content becomes available after a requested job completes.
import type { FlowAnalysisStatus, Tour } from "../api/generated/types";
import { seedFor } from "./fixtures";
import { liveAnalysis, setLiveState } from "./live-analysis";
import { flowStage } from "./scenarios";
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

// The fixture plan in rank order; flow 1 is analysed from the start.
const plan = () => (seedFor("default", "tour") as Tour).flows;
const active = (id: string) => ["queued", "analysing"].includes(states.get(id)?.state ?? "");

/**
 * Publishes the running state like the server: the stage of the flow being analysed (or next); idle when no flow
 * work is admitted (nothing requested, and background analysis off or nothing left).
 */
function publish(): void {
  const current = liveAnalysis().state;
  if (current.state !== "running") return void setLiveState(current);
  const flows = plan();
  const working =
    flows.find((f) => states.get(f.id)?.state === "analysing") ?? flows.find((f) => active(f.id));
  const done = flows.filter((f) => f.position === 1 || states.get(f.id)?.state === "analysed").length;
  const left = flows.some((f) => f.position !== 1 && !states.has(f.id));
  setLiveState({
    state: "running",
    done,
    total: flows.length,
    idle: !working && !(liveAnalysis().background_analysis && left),
    ...(working ? { stage: flowStage(working.position, states.get(working.id)?.started_at ?? null) } : {}),
  });
}

/** With "Analyse all remaining flows" on and nothing queued, queues the next pending flow by rank. */
export function continueLazyFlows(): void {
  if (!liveAnalysis().background_analysis || plan().some((f) => active(f.id))) return;
  const next = plan().find((f) => f.position !== 1 && !states.has(f.id));
  if (next) requestLazyFlow(next.id);
}

export function requestLazyFlow(id: string) {
  if (states.get(id)?.state === "analysed" || states.get(id)?.state === "analysing") return;
  states.set(id, { state: "queued", started_at: null, message: null });
  publish();
  later(1_000, () => {
    states.set(id, { state: "analysing", started_at: new Date().toISOString(), message: null });
    publish();
    later(70_000, () => {
      states.set(id, { state: "analysed", started_at: null, message: null });
      publish();
      continueLazyFlows();
    });
  });
}
