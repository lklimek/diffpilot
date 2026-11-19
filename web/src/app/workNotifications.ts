// Browser notifications when work finishes while the user looks elsewhere (owner request, 2026-11-19): an Ask
// answer, a flow analysed, review ready, an analysis finished or failed. Opt-in per browser; needs a secure context
// and the browser's permission, and an open diffpilot tab (no service worker). Screens feed their data.
import { useEffect, useRef, useSyncExternalStore } from "react";
import { useNavigate } from "react-router";
import type { AnalysisId, AnalysisState, AnalysisSummary, AskQuestion, Tour } from "../api/generated/types";
import type { NotificationSupport } from "../components/settings/NotificationsCard";
import { findStep } from "../screens/tour/sequence";
import { usePrDisplayId } from "./tabStatus";

/** localStorage key of the per-browser opt-in ("on" when chosen). */
export const NOTIFY_KEY = "diffpilot.notify-when-done";
const SEEN_KEY = "diffpilot.notified";
/** A tab not watched waits this long, so a tab watching the same work can claim the event first. */
export const NOTIFY_HANDOVER_MS = 1_500;
const SEEN_TTL_MS = 10 * 60_000;

/** Finished work worth a notification; `key` is the same in every tab. */
export interface WorkEvent {
  key: string;
  title: string;
  body: string;
  /** In-app path the click opens. */
  path: string;
}

/** Whether this browser can notify: not at all, not over http, blocked, not asked yet, or allowed. */
export function notifySupport(): NotificationSupport {
  if (!window.isSecureContext) return "insecure";
  if (typeof window.Notification !== "function") return "unsupported";
  return window.Notification.permission;
}

const storage = (): Storage | null => {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
};

/** The opt-in is stored and the browser allows notifications. */
export function notificationsOn(): boolean {
  return notifySupport() === "granted" && storage()?.getItem(NOTIFY_KEY) === "on";
}

const listeners = new Set<() => void>();
const changed = () => listeners.forEach((l) => l());
const subscribe = (listener: () => void) => {
  listeners.add(listener);
  window.addEventListener("storage", listener);
  return () => {
    listeners.delete(listener);
    window.removeEventListener("storage", listener);
  };
};

/** Asks for permission (call from a user gesture) and stores the opt-in when granted. */
export async function enableNotifications(): Promise<NotificationSupport> {
  const support = notifySupport();
  const result = support === "default" ? await window.Notification.requestPermission() : support;
  try {
    if (result === "granted") storage()?.setItem(NOTIFY_KEY, "on");
  } catch {
    // Blocked site data: the choice cannot be kept.
  }
  changed();
  return result;
}

/** Removes the opt-in. */
export function disableNotifications(): void {
  storage()?.removeItem(NOTIFY_KEY);
  changed();
}

/** The Settings view of the opt-in (screens only). */
export function useNotifySetting(): {
  support: NotificationSupport;
  enabled: boolean;
  onChange: (enabled: boolean) => void;
} {
  const state = useSyncExternalStore(subscribe, () => `${notifySupport()} ${notificationsOn()}`);
  const [support, enabled] = state.split(" ");
  return {
    support: support as NotificationSupport,
    enabled: enabled === "true",
    onChange: (on) => (on ? void enableNotifications() : disableNotifications()),
  };
}

/** Records `key` as handled in every tab; false when a tab already did. */
function claim(key: string): boolean {
  const store = storage();
  if (!store) return true;
  try {
    const now = Date.now();
    const seen = JSON.parse(store.getItem(SEEN_KEY) ?? "{}") as Record<string, number>;
    if (seen[key] !== undefined && now - seen[key] < SEEN_TTL_MS) return false;
    const kept = Object.entries(seen).filter(([, at]) => now - at < SEEN_TTL_MS);
    store.setItem(SEEN_KEY, JSON.stringify(Object.fromEntries([...kept, [key, now]])));
    return true;
  } catch {
    return true;
  }
}

/** Runs `run` while holding the cross-tab lock, where the browser has one. */
const exclusively = (run: () => void): Promise<void> =>
  navigator.locks ? navigator.locks.request("diffpilot.notify", run).then(() => {}) : Promise.resolve(run());

/**
 * Shows `event` when the opt-in is on and this tab is hidden or unfocused, once across tabs: a watching tab claims
 * it silently. A click focuses the tab and opens the event's path with `open`.
 */
export async function notifyWorkDone(event: WorkEvent, open: (path: string) => void): Promise<void> {
  if (!notificationsOn()) return;
  const watched = () => document.visibilityState === "visible" && document.hasFocus();
  if (!watched()) await new Promise((resolve) => setTimeout(resolve, NOTIFY_HANDOVER_MS));
  await exclusively(() => {
    if (!claim(event.key) || watched()) return;
    try {
      const shown = new window.Notification(event.title, { body: event.body, tag: `diffpilot:${event.key}` });
      shown.onclick = () => {
        window.focus();
        open(event.path);
        shown.close();
      };
    } catch {
      // Mobile browsers allow notifications from a service worker only.
    }
  });
}

const reviewPath = (id: AnalysisId, fragment?: string) =>
  `/analyses/${encodeURIComponent(id)}${fragment ? `#${encodeURIComponent(fragment)}` : ""}`;

/** The analysis finished or failed between `before` and `after`. */
function analysisEnded(
  id: AnalysisId,
  before: AnalysisState,
  after: AnalysisState,
  pr: string,
  failedPath: string,
): WorkEvent[] {
  if (before.state === after.state) return [];
  if (after.state === "finished")
    return [
      {
        key: `${id}:finished`,
        title: `Analysis finished — ${pr}`,
        body: "The LLM analysis of this pull request finished.",
        path: reviewPath(id),
      },
    ];
  if (after.state === "failed")
    return [
      {
        key: `${id}:failed`,
        title: `Analysis failed — ${pr}`,
        body: "Open diffpilot to see what failed and try again.",
        path: failedPath,
      },
    ];
  return [];
}

/** Flows analysed or failed, or the analysis finished or failed (which then stands for its last flow). */
export function tourEvents(before: Tour, after: Tour, pr: string): WorkEvent[] {
  const id = after.analysis.id;
  const ended = analysisEnded(id, before.analysis.state, after.analysis.state, pr, reviewPath(id));
  if (ended.length) return ended;
  return after.flows.flatMap((flow): WorkEvent[] => {
    const now = flow.analysis_status?.state;
    const was = before.flows.find((f) => f.id === flow.id)?.analysis_status?.state;
    if (!was || was === now || (now !== "analysed" && now !== "failed")) return [];
    const n = flow.position;
    return [
      now === "analysed"
        ? {
            key: `${id}:flow:${flow.id}:analysed:${flow.analysis_status?.started_at ?? ""}`,
            title: `Flow ${n} analysed — ${pr}`,
            body: `The LLM analysis of flow ${n} is ready.`,
            path: reviewPath(id, flow.id),
          }
        : {
            key: `${id}:flow:${flow.id}:failed:${flow.analysis_status?.started_at ?? ""}`,
            title: `Flow ${n} failed — ${pr}`,
            body: `The LLM analysis of flow ${n} failed. Open it to try again.`,
            path: reviewPath(id, flow.id),
          },
    ];
  });
}

/** The user's own questions answered or failed since `before`; never the question or answer text. */
export function questionEvents(
  id: AnalysisId,
  before: readonly AskQuestion[],
  after: readonly AskQuestion[],
  tour: Tour,
  pr: string,
): WorkEvent[] {
  return after.flatMap((q): WorkEvent[] => {
    const was = before.find((b) => b.id === q.id)?.state;
    if (q.mine === false || (was !== "pending" && was !== "answering")) return [];
    if (q.state !== "answered" && q.state !== "failed") return [];
    const found = findStep(tour, q.step_id);
    const where = found ? `flow ${found.flow.position}, step ${found.step.number}` : "a step";
    return [
      {
        key: `${id}:question:${q.id}:${q.state}`,
        title: q.state === "answered" ? `Answer ready — ${pr}` : `Question failed — ${pr}`,
        body:
          q.state === "answered"
            ? `LLM answer to your question on ${where}.`
            : `No LLM answer to your question on ${where}.`,
        path: reviewPath(id, q.step_id),
      },
    ];
  });
}

/** Review ready, or the analysis finished or failed, on the progress page. */
export function progressEvents(before: AnalysisSummary, after: AnalysisSummary, pr: string): WorkEvent[] {
  if (before.id !== after.id) return [];
  const id = after.id;
  const ended = analysisEnded(id, before.state, after.state, pr, `${reviewPath(id)}/progress`);
  if (ended.length) return ended;
  if (!before.review_ready && after.review_ready)
    return [
      {
        key: `${id}:review-ready`,
        title: `Review ready — ${pr}`,
        body: "Plan and PR context are ready; flows are analysed while you review.",
        path: reviewPath(id),
      },
    ];
  return [];
}

/** Notifies of the events `diff` finds between the previous and the current `value` (none on the first). */
function useEvents<T>(value: T | undefined, diff: (before: T, after: T) => WorkEvent[]): void {
  const navigate = useNavigate();
  const previous = useRef(value);
  useEffect(() => {
    const before = previous.current;
    previous.current = value;
    if (before === undefined || value === undefined || before === value) return;
    for (const event of diff(before, value)) void notifyWorkDone(event, (path) => void navigate(path));
  }, [value, diff, navigate]);
}

/** Notifications of the review screen: flows, the analysis's end and the user's Ask answers (screens only). */
export function useTourNotifications(tour: Tour, questions: readonly AskQuestion[] | undefined): void {
  const pr = usePrDisplayId(tour.analysis.pr) ?? "";
  useEvents(tour, (before, after) => tourEvents(before, after, pr));
  useEvents(questions, (before, after) => questionEvents(tour.analysis.id, before, after, tour, pr));
}

/** Notifications of the progress page: review ready and the analysis's end (screens only). */
export function useProgressNotifications(summary: AnalysisSummary | undefined): void {
  const pr = usePrDisplayId(summary?.pr);
  useEvents(summary, (before, after) => (pr ? progressEvents(before, after, pr) : []));
}
