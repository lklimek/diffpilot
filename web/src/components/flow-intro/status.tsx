import type { AnalysisState, FlowAnalysisStatus } from "../../api/generated/types";
import { Tooltip } from "../tooltip/Tooltip";

/** Text and tip of a flow's work state; work only, never a risk or an assessment. */
export interface FlowWorkStatus {
  text: string;
  tip: string;
}

const NOT_ASSESSED = "Its changes are not assessed yet.";

/** What a flow not analysed yet is waiting for, truthful for the state of the whole analysis. */
export function flowWorkStatus(status: FlowAnalysisStatus, analysis?: AnalysisState): FlowWorkStatus {
  if (status.state === "failed")
    return { text: "Failed", tip: `The LLM analysis of this flow failed. ${NOT_ASSESSED}` };
  if (analysis?.state === "failed" || analysis?.state === "cancelled")
    return {
      text: `Not analysed — the analysis ${analysis.state === "failed" ? "failed" : "was cancelled"}`,
      tip: `The analysis stopped before this flow was analysed; nothing runs for it now. ${NOT_ASSESSED}`,
    };
  if (status.state === "analysing")
    return { text: "Being analysed", tip: `The LLM is analysing this flow now. ${NOT_ASSESSED}` };
  const paused = analysis?.state === "paused";
  const base = status.state === "queued" ? "Queued" : "Not analysed yet";
  return {
    text: paused ? `${base} — analysis paused` : base,
    tip: paused
      ? `The analysis is paused, so this flow waits until it resumes. ${NOT_ASSESSED}`
      : status.state === "queued"
        ? `Requested; it runs after the job that is running now. ${NOT_ASSESSED}`
        : `The LLM has not analysed this flow yet; it runs when you ask for it. ${NOT_ASSESSED}`,
  };
}

/** A flow's work state as a small label with its tip (shown instead of a risk label until it is analysed). */
export function FlowWorkLabel({
  status,
  analysis,
}: {
  status: FlowAnalysisStatus;
  analysis?: AnalysisState;
}) {
  const { text, tip } = flowWorkStatus(status, analysis);
  return (
    <Tooltip tip={tip}>
      <span tabIndex={0} className="cursor-help text-right text-xs text-muted">
        {text}
      </span>
    </Tooltip>
  );
}
