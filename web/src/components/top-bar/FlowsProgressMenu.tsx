import { useEffect, useId, useRef, useState, type MouseEvent } from "react";
import type { AnalysisState, Flow, FlowId, StepId } from "../../api/generated/types";
import { RiskLabel } from "../badge/LlmLabel";
import { busyTip } from "../button/button";
import { isFlowReviewed, reviewedSteps } from "../flow-intro/progress";
import { flowWorkStatus } from "../flow-intro/status";
import { clockText } from "../format/time";
import { Tooltip } from "../tooltip/Tooltip";

interface FlowsProgressMenuProps {
  flows: readonly Flow[];
  analysis: AnalysisState;
  /** Steps ticked in the Flows view. */
  ticked: readonly StepId[];
  /** `background_analysis`: all remaining flows are analysed one after another; unknown: no switch. */
  background: boolean | undefined;
  /** A work request is being sent. */
  busy?: boolean;
  /** The last work request that failed, in plain words. */
  error?: string | null;
  /** Opens the flow's introduction. */
  onOpenFlow: (flow: FlowId) => void;
  /** Requests a flow's analysis; no ▶ without it. */
  onRequest?: (flow: FlowId) => void;
  /** Requests a failed flow's analysis again; no ▶ on a failed flow without it. */
  onRetry?: (flow: FlowId) => void;
  onPause?: () => void;
  onResume?: () => void;
  onBackground?: (enabled: boolean) => void;
}

const BUTTON =
  "inline-flex min-h-6 cursor-pointer items-center rounded-md border border-border bg-surface px-2 text-[12px] text-text hover:border-accent aria-disabled:cursor-wait aria-disabled:opacity-60";
const ICON =
  "inline-flex size-6 shrink-0 cursor-pointer items-center justify-center rounded border border-border bg-surface text-text hover:border-accent aria-disabled:cursor-wait aria-disabled:opacity-60";

const analysed = (flow: Flow) => (flow.analysis_status?.state ?? "analysed") === "analysed";
const named = (flows: readonly Flow[]) => flows.map((f) => `flow ${f.position}`);

/** The status line: what the analysis is doing now, or that it is paused. */
function StatusLine({
  flows,
  analysis,
  background,
  busy,
  onResume,
  onBackground,
}: Pick<FlowsProgressMenuProps, "flows" | "analysis" | "background" | "busy" | "onResume" | "onBackground">) {
  if (analysis.state === "paused") {
    const by = analysis.reason === "usage_limit" ? "until the LLM usage limit resets" : "by you";
    const at = analysis.paused_at ? ` at ${clockText(analysis.paused_at)}` : "";
    return (
      <p className="m-0 flex flex-wrap items-center gap-2 border-b border-border pb-1.5">
        <span>
          <span aria-hidden>⏸ </span>Analysis paused {by}
          {at}. Nothing starts until you resume.
        </span>
        {onResume && analysis.reason !== "usage_limit" && (
          <Tooltip
            tip={busyTip("Resume the analysis: requested flows start again. Starts LLM work.", busy ?? false)}
          >
            <button
              type="button"
              aria-disabled={busy || undefined}
              onClick={() => !busy && onResume()}
              className={BUTTON}
            >
              ▶ Resume
            </button>
          </Tooltip>
        )}
      </p>
    );
  }
  const now = flows.filter((f) => f.analysis_status?.state === "analysing");
  if (analysis.state !== "running" || now.length === 0) return null;
  const next = flows.filter((f) => f.analysis_status?.state === "queued");
  const then = next.length > 0 ? `, then ${named(next).join(", ")}` : "";
  return (
    <p className="m-0 flex flex-wrap items-center gap-2 border-b border-border pb-1.5">
      <span>
        {background ? "Analysing all remaining flows: " : "Analysing "}
        {named(now).join(", ")} now{then}.
      </span>
      {background && onBackground && (
        <Tooltip
          tip={busyTip(
            "Stops starting further flows. The running flow finishes and is kept. Starts no LLM work.",
            busy ?? false,
          )}
        >
          <button
            type="button"
            aria-disabled={busy || undefined}
            onClick={() => !busy && onBackground(false)}
            className={BUTTON}
          >
            Stop
          </button>
        </Tooltip>
      )}
    </p>
  );
}

/**
 * "Your progress k / n ▾" in the top bar (proposal §8): k = flows with every step ticked. It opens every flow with
 * its analysis state, steps ticked, and the one work action that fits (▶ analyse, ⏸ pause); never Cancel.
 */
export function FlowsProgressMenu(props: FlowsProgressMenuProps) {
  const {
    flows,
    analysis,
    ticked,
    background,
    busy = false,
    error,
    onOpenFlow,
    onRequest,
    onRetry,
    onPause,
  } = props;
  const [open, setOpen] = useState(false);
  const id = useId();
  const button = useRef<HTMLButtonElement>(null);
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const outside = (event: Event) => {
      if (!box.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("pointerdown", outside);
    return () => document.removeEventListener("pointerdown", outside);
  }, [open]);
  const reviewed = flows.filter((f) => isFlowReviewed(f, ticked)).length;
  const remaining = flows.filter((f) => !analysed(f)).length;
  const paused = analysis.state === "paused";
  return (
    <div
      ref={box}
      className="relative"
      onKeyDown={(event) => {
        if (event.key !== "Escape" || !open) return;
        event.preventDefault();
        event.stopPropagation();
        setOpen(false);
        button.current?.focus();
      }}
    >
      <Tooltip tip="Flows you reviewed (all steps ticked in the Flows view), out of all flows. Open it for each flow's analysis state, steps ticked and the analyse / pause actions. No percentage and no weighting by risk.">
        <button
          ref={button}
          type="button"
          aria-expanded={open}
          aria-controls={`${id}-flows`}
          aria-label={`Your progress ${reviewed} / ${flows.length}: flows reviewed. Show the flows with their analysis and review state`}
          onClick={() => setOpen((o) => !o)}
          className="review-count-button cursor-pointer rounded-md border border-transparent bg-transparent px-1.5 py-0.5 text-[12.5px] text-text hover:border-border"
        >
          Your progress{" "}
          <span className="whitespace-nowrap">
            <b>{reviewed}</b> / {flows.length} <span aria-hidden>▾</span>
          </span>
        </button>
      </Tooltip>
      {open && (
        <div
          id={`${id}-flows`}
          role="region"
          aria-label="Flows: analysis and review"
          className="absolute top-full right-0 z-40 mt-1 grid w-[min(30rem,90vw)] gap-1.5 rounded-md border border-border bg-surface p-2 text-[12.5px] whitespace-normal shadow-tip"
        >
          <StatusLine {...props} busy={busy} />
          <ol className="m-0 grid list-none gap-1 p-0">
            {flows.map((flow) => {
              const done = analysed(flow);
              const status = flow.analysis_status;
              const steps = reviewedSteps(flow, ticked);
              const state = done ? "Analysed" : status ? flowWorkStatus(status, analysis).text : "";
              return (
                <li
                  key={flow.id}
                  className="flex items-start gap-2 border-t border-border pt-1 first:border-t-0 first:pt-0"
                >
                  <span className="grid min-w-0 flex-1 gap-0.5">
                    <span className="flex flex-wrap items-center gap-1.5">
                      <a
                        href={`#${flow.id}`}
                        onClick={(event: MouseEvent) => {
                          event.preventDefault();
                          setOpen(false);
                          onOpenFlow(flow.id);
                        }}
                        className="text-text"
                      >
                        <b>{flow.position}</b> {flow.llm.title}
                      </a>
                      {done && <RiskLabel risk={flow.llm.risk} />}
                    </span>
                    <span className="text-[12px] text-muted">
                      {state} · {done ? `${steps} / ${flow.steps.length} steps ticked` : "no steps yet"}
                      {isFlowReviewed(flow, ticked) && <b className="ml-1 text-ok">☑ reviewed</b>}
                    </span>
                  </span>
                  {status?.state === "analysing" && onPause && !paused && (
                    <Tooltip
                      tip={busyTip(
                        "Pause the analysis (all flows): this flow finishes and is kept, no further flow starts until Resume. Plan, analysed flows and sessions are kept. Starts no LLM work.",
                        busy,
                      )}
                    >
                      <button
                        type="button"
                        aria-label="Pause analysis"
                        aria-disabled={busy || undefined}
                        onClick={() => !busy && onPause()}
                        className={ICON}
                      >
                        <span aria-hidden>⏸</span>
                      </button>
                    </Tooltip>
                  )}
                  {status?.state === "not_analysed" && onRequest && (
                    <Tooltip
                      tip={busyTip(
                        `Analyse this flow: starts LLM work (one agent run) for this flow${paused ? "; while the analysis is paused it waits until you resume" : ""}.`,
                        busy,
                      )}
                    >
                      <button
                        type="button"
                        aria-label={`Analyse flow ${flow.position}`}
                        aria-disabled={busy || undefined}
                        onClick={() => !busy && onRequest(flow.id)}
                        className={ICON}
                      >
                        <span aria-hidden>▶</span>
                      </button>
                    </Tooltip>
                  )}
                  {status?.state === "failed" && onRetry && (
                    <Tooltip
                      tip={busyTip(
                        `Analyse this flow again: starts LLM work (one agent run) from scratch for this flow${paused ? "; while the analysis is paused it waits until you resume" : ""}.`,
                        busy,
                      )}
                    >
                      <button
                        type="button"
                        aria-label={`Analyse flow ${flow.position} again`}
                        aria-disabled={busy || undefined}
                        onClick={() => !busy && onRetry(flow.id)}
                        className={ICON}
                      >
                        <span aria-hidden>▶</span>
                      </button>
                    </Tooltip>
                  )}
                </li>
              );
            })}
          </ol>
          {remaining === 0 ? (
            <p className="m-0 text-[12px] text-muted">All flows are analysed.</p>
          ) : (
            background === false &&
            props.onBackground && (
              <p className="m-0">
                <Tooltip
                  tip={busyTip(
                    `Analyse all remaining flows: starts LLM work for every flow not analysed yet, one after another${paused ? ", once you resume" : ""}.`,
                    busy,
                  )}
                >
                  <button
                    type="button"
                    aria-disabled={busy || undefined}
                    onClick={() => !busy && props.onBackground?.(true)}
                    className={BUTTON}
                  >
                    Analyse all remaining flows
                  </button>
                </Tooltip>
              </p>
            )
          )}
          {error && (
            <p role="alert" className="m-0 text-high">
              {error}
            </p>
          )}
        </div>
      )}
    </div>
  );
}
