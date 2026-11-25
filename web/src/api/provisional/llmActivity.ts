// TODO(v1.4 phase B): replace with generated contract type (plan C7)
import type { LlmCallOutcome } from "../generated/types";
import { provisionalRouteAbsent } from "./settingsResponse";

export const LLM_ACTIVITY_PATH = "/api/v1/llm/activity";

export type LlmJobKind = "analysis" | "flow_analysis" | "ranking" | "rejudge" | "comment_assessment" | "ask";

type LlmJob = { id: string; kind: LlmJobKind } &
  // Subject links are app-relative. Other readers’ subjects are omitted on the server.
  ({ owner: "self"; subject: { label: string; href: string } | null } | { owner: "other"; subject: null });

export type RunningLlmJob = LlmJob & { since: string };
/** waits_for is the observed reason, as display text. */
export type QueuedLlmJob = LlmJob & { queued_at: string; waits_for: string | null };

/** The activity snapshot and its future C7 event carry the pause and last-call outcome. */
export interface LlmActivity {
  running: RunningLlmJob[];
  queued: QueuedLlmJob[];
  paused_until: string | null;
  last_call: LlmCallOutcome | null;
}

/** Null when the provisional route is absent. Status and pause can still come from Settings. */
export async function fetchLlmActivity(signal?: AbortSignal): Promise<LlmActivity | null> {
  const response = await fetch(LLM_ACTIVITY_PATH, {
    headers: { Accept: "application/json" },
    credentials: "same-origin",
    signal,
  });
  if (provisionalRouteAbsent(response)) return null;
  if (!response.ok) throw new Error("Job activity unavailable.");
  return (await response.json()) as LlmActivity;
}
