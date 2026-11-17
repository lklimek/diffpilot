import { UsageLine } from "../usage/UsageLine";
import type { UsageTotals } from "../../api/generated/types";
import { plural } from "../quiet/labels";
import type {
  AnalysisProgress as RunProgress,
  AnalysisEstimate,
  AnalysisStage,
  AnalysisState,
} from "../../api/generated/types";
import { busyTip, buttonClass } from "../button/button";
import { PAUSE_REASON, pausedText } from "./paused";
import { Tooltip } from "../tooltip/Tooltip";

/** What this page knows of the event stream; the screen ticks `now`. */
export interface ProgressActivity {
  /** Browser time, ms since the epoch. */
  now: number;
  /** Browser minus server clock (ms), from the last heartbeat; null before the first one. */
  clockOffset: number | null;
  /** Browser time when the last event of the stream (heartbeat or other) arrived. */
  lastSignalAt: number;
  /** The event stream is open. */
  connected: boolean;
}

/** Silence after which the stream counts as lost (heartbeats come every 15 s). */
export const SILENT_MS = 45_000;

interface AnalysisProgressProps {
  /** State of the analysis (brief §4.2). */
  state: AnalysisState;
  /** Whole-run facts; absent on older servers. */
  progress?: RunProgress;
  usage?: UsageTotals | null;
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
      return `${state.done} of ${plural(state.total, "flow")} analysed`;
    case "paused":
      return pausedText(state);
    case "failed":
      return state.flow
        ? `Analysis of flow “${state.flow.title}” failed`
        : "The analysis failed before any flow was planned";
    case "finished":
      return `${state.total} of ${plural(state.total, "flow")} analysed`;
    case "cancelled":
      return "Analysis cancelled";
  }
}

/** An LLM flow title as one plain line of at most 80 characters. */
function titleText(title: string): string {
  const line = title.replace(/\s+/g, " ").trim();
  return line.length > 80 ? `${line.slice(0, 79).trimEnd()}…` : line;
}

/** What a stage does, in plain words; `total` names the flow's place when known. */
function stageWork(stage: AnalysisStage, done: number | null, total: number | null): string {
  switch (stage.kind) {
    case "plan":
      return stage.part
        ? `planning the review, part ${stage.part.index} of ${stage.part.total}.`
        : "planning the review.";
    case "pr_context":
      return "reading the PR context.";
    case "flow":
      if (stage.flow) {
        const of = total === null ? "" : ` of ${total}`;
        return `analysing flow ${stage.flow.index}${of}: ${titleText(stage.flow.title)}`;
      }
      return done !== null && total !== null
        ? `analysing a flow (${total - done} of ${total} still to do).`
        : "analysing a flow.";
  }
}

/** The stage line of a planning or running analysis, from the server's `stage` only. */
function stageText(state: AnalysisState): string | null {
  if (state.state !== "planning" && state.state !== "running") return null;
  const { stage } = state;
  if (!stage) return "Between two jobs: the server starts the next one.";
  const [done, total] = state.state === "running" ? [state.done, state.total] : [null, null];
  const work = stageWork(stage, done, total);
  if (stage.started_at === null) return `Waiting for the runner; next: ${work}`;
  return work.charAt(0).toUpperCase() + work.slice(1);
}

/** `less than a minute`, `12 min`, `1 h 5 min`. */
function durationText(ms: number): string {
  const minutes = Math.floor(ms / 60_000);
  if (minutes < 1) return "less than a minute";
  if (minutes < 60) return `${minutes} min`;
  const rest = minutes % 60;
  return rest === 0 ? `${Math.floor(minutes / 60)} h` : `${Math.floor(minutes / 60)} h ${rest} min`;
}

/**
 * Time since the stage's `started_at`, on the server's clock: browser time minus the offset measured by the last
 * heartbeat; never negative (clock jitter). Without a heartbeat yet the offset is unknown and no time is claimed.
 */
function elapsedText(started: string | null, activity: ProgressActivity, working: boolean): string {
  const at = started === null ? NaN : Date.parse(started);
  if (Number.isNaN(at) || activity.clockOffset === null) return working ? "Working" : "";
  const since = `stage started ${durationText(Math.max(0, activity.now - activity.clockOffset - at))} ago`;
  return working ? `Working · this ${since}` : `This ${since}`;
}

/** `52 s` under two minutes, then whole minutes. */
const silenceText = (ms: number) =>
  ms < 120_000 ? `${Math.floor(ms / 1000)} s` : `${Math.floor(ms / 60_000)} min`;

const BAR_LOOK: Record<AnalysisState["state"], string> = {
  planning: "border-border",
  running: "border-border",
  paused: "border-dashed border-med-bd",
  failed: "border-dotted border-high-bd",
  finished: "border-border",
  cancelled: "border-border",
};

// Without a whole-run denominator the bar stays indeterminate. Stripes distinguish planning and pauses.
const STRIPES = {
  planning: "repeating-linear-gradient(135deg, var(--color-accent) 0 8px, var(--color-accent-bg) 8px 16px)",
  paused: "repeating-linear-gradient(135deg, var(--color-med-bd) 0 6px, var(--color-med-bg) 6px 12px)",
};

function Bar({
  state,
  working,
  progress,
}: {
  state: AnalysisState;
  working: boolean;
  progress?: RunProgress;
}) {
  const counted = progress
    ? progress.percent === null
      ? null
      : { now: progress.percent, max: 100 }
    : state.state === "running" || (state.state === "paused" && state.total !== null)
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
      aria-valuetext={
        progress && counted
          ? percentLabel(counted.now) + label(state)
          : state.state === "planning"
            ? "Planning flows"
            : label(state)
      }
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
  progress,
  reviewHref,
  usage,
  busy,
  onCancel,
  onRetry,
  activity,
}: AnalysisProgressProps) {
  const live = state.state !== "finished" && state.state !== "cancelled";
  const stage = stageText(state);
  const started =
    (state.state === "planning" || state.state === "running") && state.stage ? state.stage.started_at : null;
  const silence = activity ? activity.now - activity.lastSignalAt : 0;
  const silent = activity !== undefined && silence > SILENT_MS;
  // Activity only while a job runs and heartbeats arrive: never a loop on its own.
  const working = started !== null && activity !== undefined && activity.connected && !silent;
  const elapsed = stage !== null && activity !== undefined ? elapsedText(started, activity, working) : "";
  const running = state.state === "planning" || state.state === "running";
  const percentText = progress?.percent != null ? percentLabel(progress.percent) : "";
  const analysisElapsed =
    progress && activity && activity.clockOffset !== null && running
      ? durationText(Math.max(0, activity.now - activity.clockOffset - Date.parse(progress.started_at)))
      : null;
  return (
    <section aria-label="Analysis progress" className="w-3/5 max-w-[1400px] min-w-[min(100vw_-_4rem,480px)]">
      {/* Changes with the server's state and stage, and the silence notice every tick at most; the time stays outside. */}
      <div aria-live="polite" className="mb-2 grid gap-0.5">
        <p className="text-[15px] font-semibold">{label(state)}</p>
        {stage && <p className="text-[13px]">{stage}</p>}
        {silent ? (
          <p className="text-[13px]">{`No signal from the server for ${silenceText(silence)}; reconnecting…`}</p>
        ) : (
          activity && !activity.connected && <p className="text-[13px]">Connection lost; reconnecting…</p>
        )}
      </div>
      {progress && <StageList progress={progress} />}
      <div className="flex items-center gap-4">
        <div className="min-w-0 flex-1">
          <Bar state={state} working={working} progress={progress} />
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
          {state.state === "failed" && state.reason !== "configuration" && (
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
      {progress && running && (
        <p className="mt-2 text-[12.5px] text-muted">
          {percentText && <span>{percentText}</span>}
          {progress.estimate ? estimateText(progress.estimate) : "No estimate yet"}
        </p>
      )}
      {analysisElapsed && (
        <p className="mt-2 text-[12.5px] text-muted">Analysis started {analysisElapsed} ago</p>
      )}
      <UsageLine usage={usage} />
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

/** Waiting progress only; review ticks never contribute to these counts. */
function StageList({ progress }: { progress: RunProgress }) {
  const stages = [
    ["Planning", progress.planning],
    ["PR context", progress.pr_context],
    ["Flows", progress.flows],
    ["Finishing", progress.finishing],
  ] as const;
  return (
    <ol aria-label="Analysis stages" className="mb-3 flex flex-wrap gap-x-5 gap-y-1 text-[12.5px]">
      {stages.map(([name, stage]) => (
        <li key={name} aria-current={stage.state === "current" ? "step" : undefined}>
          <span className={stage.state === "current" ? "font-semibold" : "text-muted"}>
            {name}:{" "}
            {stage.total === null
              ? name === "Flows"
                ? "known after planning"
                : "waiting"
              : `${stage.done} / ${stage.total}`}{" "}
            · {stage.state}
          </span>
        </li>
      ))}
    </ol>
  );
}

function estimateText(estimate: AnalysisEstimate): string {
  const basis = {
    this_analysis: "this analysis's finished parts",
    recent_runs: "recent runs",
    lines_only: "changed lines in recent runs",
  }[estimate.basis];
  const duration = durationText(estimate.remaining_seconds * 1000);
  return `About ${duration} left (${estimate.rough ? "rough estimate" : "estimate"} from ${basis})`;
}

function percentLabel(percent: number): string {
  return `${percent}% of analysis · `; // wording-check: allow analysis waiting progress, brief 4.2
}
