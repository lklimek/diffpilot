import type { AnalysisState } from "../../api/generated/types";
import { busyTip, buttonClass } from "../button/button";
import { clockText, dateTimeText } from "../format/time";
import { Tooltip } from "../tooltip/Tooltip";
import { PAUSE_REASON } from "./paused";

interface Props {
  /** State of the analysis behind the open review. */
  state: AnalysisState;
  /** A resume or retry request runs. */
  busy: boolean;
  onResume: () => void;
  onRetry: () => void;
  /** The progress screen (timing, failure details, pause and cancel). */
  progressHref: string;
  /** Why the last resume or retry request failed. */
  error?: string;
}

const SMALL_PLAIN = buttonClass("plain", true);

/** One line in the Tour when its analysis is paused or failed: the reason and the action available. */
export function AnalysisStateBanner({ state, busy, onResume, onRetry, progressHref, error }: Props) {
  let text: string;
  let action = null;
  if (state.state === "paused" && state.reason === "user") {
    const at = state.paused_at ? ` at ${dateTimeText(state.paused_at)}` : "";
    text = `Analysis paused by ${state.paused_by ?? "a user"}${at}. Flows not analysed yet wait until it resumes.`;
    action = (
      <Tooltip tip={busyTip("Continues the analysis with the next job; saved results are kept.", busy)}>
        <button
          type="button"
          aria-disabled={busy || undefined}
          className={SMALL_PLAIN}
          onClick={() => !busy && onResume()}
        >
          Resume
        </button>
      </Tooltip>
    );
  } else if (state.state === "paused") {
    const when = state.resumes_at
      ? `It resumes at ${clockText(state.resumes_at)}.`
      : "The resume time is not known.";
    text = `Analysis paused. ${PAUSE_REASON} ${when}`;
  } else if (state.state === "failed") {
    const what = state.flow ? `The analysis of flow “${state.flow.title}” failed.` : "The analysis failed.";
    text = `${what} ${state.message}`;
    if (state.reason !== "configuration")
      action = (
        <Tooltip tip={busyTip("Runs the failed analysis again; accepted results stay readable.", busy)}>
          <button
            type="button"
            aria-disabled={busy || undefined}
            className={SMALL_PLAIN}
            onClick={() => !busy && onRetry()}
          >
            Retry
          </button>
        </Tooltip>
      );
  } else {
    return null;
  }
  return (
    <section
      aria-label="Analysis state"
      className="mb-2 flex flex-wrap items-center gap-2 rounded-md border border-l-4 border-border border-l-med-bd bg-surface px-3 py-1.5 text-[12.5px]"
    >
      <p role="status" className="m-0 min-w-0 flex-1">
        {text}
      </p>
      {action}
      <Tooltip tip="Opens the analysis progress: what failed or why it waits, timing, pause and cancel.">
        <a href={progressHref} className="text-accent underline">
          Details
        </a>
      </Tooltip>
      {error && (
        <p role="alert" className="m-0 basis-full">
          {error}
        </p>
      )}
    </section>
  );
}
