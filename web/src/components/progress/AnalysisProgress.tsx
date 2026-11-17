import type { AnalysisState } from "../../api/generated/types";
import { busyTip, buttonClass } from "../button/button";
import { PAUSE_REASON, pausedText } from "./paused";
import { Tooltip } from "../tooltip/Tooltip";

/** What this page observed of the event stream (not server data); the screen ticks `now`. */
export interface ProgressActivity {
  /** Current time, ms since the epoch. */
  now: number;
  /** When this page first saw the current state; `exact` false when a (re)connection already showed it. */
  stageSince: { at: number; exact: boolean };
  /** When the last update from the server arrived. */
  lastUpdateAt: number;
  /** The event stream is open. */
  connected: boolean;
}

/** Silence from the server after which the page says so instead of showing activity. */
export const STALE_MS = 5 * 60_000;

interface AnalysisProgressProps {
  /** State of the analysis (brief §4.2). */
  state: AnalysisState;
  /** Where "Open review" leads once the analysis has finished. */
  reviewHref: string;
  /** A cancel or retry request is running: the actions are disabled. */
  busy: boolean;
  /** Cancel analysis (discards partial results). */
  onCancel: () => void;
  /** Retry the failed analysis. */
  onRetry: () => void;
  /** Connection and timing as this page saw them; without it, no elapsed time and no activity indicator. */
  activity?: ProgressActivity;
}

function label(state: AnalysisState): string {
  switch (state.state) {
    case "planning":
      return "Planning flows…";
    case "running":
      return `${state.done} of ${state.total} flows analysed`;
    case "paused":
      return pausedText(state);
    case "failed":
      return state.flow
        ? `Analysis of flow “${state.flow.title}” failed`
        : "The analysis failed before any flow was planned";
    case "finished":
      return `${state.total} of ${state.total} flows analysed`;
    case "cancelled":
      return "Analysis cancelled";
  }
}

/**
 * What runs now, from the state alone: the server plans, then reads the PR context, then analyses the flows one at
 * a time in order, so with `done` flows ended flow `done + 1` runs (or the PR context while none has ended).
 */
function stageText(state: AnalysisState): string | null {
  if (state.state === "planning") return "Planning the review: grouping the changes into flows.";
  if (state.state !== "running") return null;
  if (state.done >= state.total) return "Finishing the review: every flow is done.";
  if (state.done === 0) return `Reading the PR context, then analysing flow 1 of ${state.total}.`;
  return `Analysing flow ${state.done + 1} of ${state.total}.`;
}

/** `less than a minute`, `12 min`, `1 h 5 min`. */
function durationText(ms: number): string {
  const minutes = Math.floor(ms / 60_000);
  if (minutes < 1) return "less than a minute";
  if (minutes < 60) return `${minutes} min`;
  const rest = minutes % 60;
  return rest === 0 ? `${Math.floor(minutes / 60)} h` : `${Math.floor(minutes / 60)} h ${rest} min`;
}

/** Real time since the stage started; from a (re)connection on only a lower bound, so none under a minute. */
function elapsedText({ now, stageSince }: ProgressActivity, working: boolean): string {
  const elapsed = now - stageSince.at;
  if (!stageSince.exact && elapsed < 60_000) return working ? "Working" : "";
  const since = `stage started ${stageSince.exact ? "" : "at least "}${durationText(elapsed)} ago`;
  return working ? `Working · this ${since}` : `This ${since}`;
}

const BAR_LOOK: Record<AnalysisState["state"], string> = {
  planning: "border-border",
  running: "border-border",
  paused: "border-dashed border-med-bd",
  failed: "border-dotted border-high-bd",
  finished: "border-border",
  cancelled: "border-border",
};

// Planning, and paused while planning, have no count: the bar is indeterminate (striped; planning pulses unless
// motion is reduced). Paused with a count is striped too, filled to the flows analysed.
const STRIPES = {
  planning: "repeating-linear-gradient(135deg, var(--color-accent) 0 8px, var(--color-accent-bg) 8px 16px)",
  paused: "repeating-linear-gradient(135deg, var(--color-med-bd) 0 6px, var(--color-med-bg) 6px 12px)",
};

function Bar({ state, working }: { state: AnalysisState; working: boolean }) {
  const counted =
    state.state === "running" || (state.state === "paused" && state.total !== null)
      ? { now: state.done, max: state.total ?? 0 }
      : state.state === "finished"
        ? { now: state.total, max: state.total }
        : null;
  // Filled and empty parts share the bar by flex-grow: done vs. remaining flows, or all filled without a count.
  const [filled, empty] = counted ? [counted.now, counted.max - counted.now] : [1, 0];
  const background =
    state.state === "planning" || state.state === "paused"
      ? STRIPES[state.state]
      : state.state === "failed"
        ? "var(--color-high-bg)"
        : "var(--color-accent)";
  return (
    <div
      role="progressbar"
      aria-label="Analysis progress"
      aria-valuemin={counted ? 0 : undefined}
      aria-valuemax={counted?.max}
      aria-valuenow={counted?.now}
      aria-valuetext={state.state === "planning" ? "Planning flows" : label(state)}
      className={`flex h-3 overflow-hidden rounded-md border bg-surface-2 ${BAR_LOOK[state.state]}`}
    >
      <i
        className={`block h-full ${state.state === "planning" && working ? "motion-safe:animate-pulse" : ""}`}
        style={{ flexGrow: filled, background }}
      />
      <span style={{ flexGrow: empty }} />
    </div>
  );
}

/** The single centred progress bar of an analysis with its actions (brief §4.2). */
export function AnalysisProgress({
  state,
  reviewHref,
  busy,
  onCancel,
  onRetry,
  activity,
}: AnalysisProgressProps) {
  const live = state.state !== "finished" && state.state !== "cancelled";
  const stage = stageText(state);
  const stale = stage !== null && activity !== undefined && activity.now - activity.lastUpdateAt > STALE_MS;
  // Activity is shown only while the stream is open and the server spoke recently: never a loop on its own.
  const working = stage !== null && activity !== undefined && activity.connected && !stale;
  const elapsed = stage !== null && activity !== undefined ? elapsedText(activity, working) : "";
  return (
    <section aria-label="Analysis progress" className="w-3/5 max-w-[1400px] min-w-[min(100vw_-_4rem,480px)]">
      {/* Changes with the server's state only (one per stage or flow); the ticking time stays outside. */}
      <div aria-live="polite" className="mb-2 grid gap-0.5">
        <p className="text-[15px] font-semibold">{label(state)}</p>
        {stage && <p className="text-[13px]">{stage}</p>}
        {stale && (
          <p className="text-[13px]">
            No update from the server for more than 5 min; the analysis may still be running.
          </p>
        )}
      </div>
      <div className="flex items-center gap-4">
        <div className="min-w-0 flex-1">
          <Bar state={state} working={working} />
        </div>
        <span className="flex flex-none gap-2">
          {live && (
            <Tooltip
              tip={busyTip("Stops the analysis and discards its partial results; you return to Home.", busy)}
            >
              <button
                type="button"
                aria-disabled={busy || undefined}
                onClick={() => !busy && onCancel()}
                className={buttonClass("danger")}
              >
                Cancel analysis
              </button>
            </Tooltip>
          )}
          {state.state === "failed" && (
            <Tooltip tip={busyTip("Runs the failed analysis again.", busy)}>
              <button
                type="button"
                aria-disabled={busy || undefined}
                onClick={() => !busy && onRetry()}
                className={buttonClass("primary")}
              >
                Retry
              </button>
            </Tooltip>
          )}
          {state.state === "finished" && (
            <Tooltip tip="Opens the review of this analysis.">
              <a href={reviewHref} className={buttonClass("primary")}>
                Open review
              </a>
            </Tooltip>
          )}
        </span>
      </div>
      {elapsed && (
        <p
          data-activity={working || undefined}
          className="mt-2 flex items-center gap-2 text-[12.5px] text-muted"
        >
          {working && (
            <span aria-hidden className="size-2 rounded-full bg-accent motion-safe:animate-pulse" />
          )}
          {elapsed}
        </p>
      )}
      {state.state === "failed" && <p className="mt-2 max-w-[100ch]">{state.message}</p>}
      {state.state === "paused" && <p className="mt-2 max-w-[100ch]">{PAUSE_REASON}</p>}
    </section>
  );
}
