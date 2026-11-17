import type { AnalysisState } from "../../api/generated/types";
import { buttonClass } from "../button/button";
import { clockText } from "../format/time";
import { Tooltip } from "../tooltip/Tooltip";

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
}

function label(state: AnalysisState): string {
  switch (state.state) {
    case "planning":
      return "Planning flows…";
    case "running":
      return `${state.done} of ${state.total} flows analysed`;
    case "paused": {
      const count = state.total === null ? "" : `, ${state.done} of ${state.total} flows analysed`;
      return state.resumes_at
        ? `Paused${count}: Codex usage limit reached, resumes at ${clockText(state.resumes_at)}`
        : `Paused${count}: Codex usage limit reached; the resume time is not known`;
    }
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

function Bar({ state }: { state: AnalysisState }) {
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
        className={`block h-full ${state.state === "planning" ? "motion-safe:animate-pulse" : ""}`}
        style={{ flexGrow: filled, background }}
      />
      <span style={{ flexGrow: empty }} />
    </div>
  );
}

/** The single centred progress bar of an analysis with its actions (brief §4.2). */
export function AnalysisProgress({ state, reviewHref, busy, onCancel, onRetry }: AnalysisProgressProps) {
  const live = state.state !== "finished" && state.state !== "cancelled";
  return (
    <section aria-label="Analysis progress" className="w-3/5 max-w-[1400px] min-w-[min(100vw_-_4rem,480px)]">
      <p aria-live="polite" className="mb-2 text-[15px] font-semibold">
        {label(state)}
      </p>
      <div className="flex items-center gap-4">
        <div className="min-w-0 flex-1">
          <Bar state={state} />
        </div>
        <span className="flex flex-none gap-2">
          {live && (
            <Tooltip tip="Stops the analysis and discards its partial results; you return to Home.">
              <button type="button" disabled={busy} onClick={onCancel} className={buttonClass("danger")}>
                Cancel analysis
              </button>
            </Tooltip>
          )}
          {state.state === "failed" && (
            <Tooltip tip="Runs the failed analysis again.">
              <button type="button" disabled={busy} onClick={onRetry} className={buttonClass("primary")}>
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
      {state.state === "failed" && <p className="mt-2 max-w-[100ch]">{state.message}</p>}
    </section>
  );
}
