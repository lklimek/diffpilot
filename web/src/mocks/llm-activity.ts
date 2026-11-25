import type { LlmActivity, QueuedLlmJob, RunningLlmJob } from "../api/provisional/llmActivity";

const at = (minutes: number) => new Date(Date.now() + minutes * 60_000).toISOString();

export const idleActivity: LlmActivity = {
  running: [],
  queued: [],
  paused_until: null,
  last_call: null,
};

const ranking: RunningLlmJob = {
  id: "ranking",
  owner: "self",
  kind: "ranking",
  subject: { label: "My work", href: "/" },
  since: at(-5),
};

const flow: QueuedLlmJob = {
  id: "flow",
  owner: "self",
  kind: "flow_analysis",
  subject: { label: "acme/node-client#482", href: "/pr/acme/node-client/482" },
  queued_at: at(-3),
  waits_for: "Waits for a free runner",
};

export const runningActivity: LlmActivity = {
  ...idleActivity,
  running: [ranking],
  queued: [
    flow,
    { id: "other", owner: "other", kind: "ask", subject: null, queued_at: at(-1), waits_for: null },
  ],
  last_call: { at: at(-6), status: { state: "working" } },
};

export const pausedActivity: LlmActivity = {
  ...idleActivity,
  paused_until: at(60),
  queued: [{ ...flow, waits_for: "Waits for the usage limit to reset" }],
};

export const failedActivity: LlmActivity = {
  ...idleActivity,
  last_call: {
    at: at(-1),
    status: { state: "not_working", message: "Codex sign-in expired." },
  },
};

export const manyJobsActivity: LlmActivity = {
  ...runningActivity,
  queued: Array.from({ length: 7 }, (_, index) => ({ ...flow, id: `flow-${index}` })),
};
