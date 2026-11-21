import { useState } from "react";
import type { FlowId, StepId, Tour } from "../../api/generated/types";
import type { useFlowWork } from "../../api/queries";
import { FlowsProgressMenu } from "../../components/top-bar/FlowsProgressMenu";
import { errorText } from "./code";

type Work = ReturnType<typeof useFlowWork>;
type Action = "request" | "pause" | "resume" | "background";

/** The top bar's flows dropdown of both review views, wired to the analysis work requests. */
export function FlowsMenu({
  tour,
  ticked,
  work,
  onOpenFlow,
}: {
  tour: Tour;
  ticked: readonly StepId[];
  work: Work;
  onOpenFlow: (flow: FlowId) => void;
}) {
  const state = tour.analysis.state.state;
  const allowed = ["planning", "running", "paused"].includes(state);
  // Only the last action's error shows, named by that action: a later success clears an older refusal.
  const [last, setLast] = useState<{ action: Action; name: string } | null>(null);
  const run = (action: Action, name: string, start: () => void) => {
    setLast({ action, name });
    start();
  };
  const position = (flow: FlowId) => tour.flows.find((f) => f.id === flow)?.position;
  const failed = last && work[last.action].isError ? work[last.action].error : null;
  return (
    <FlowsProgressMenu
      flows={tour.flows}
      analysis={tour.analysis.state}
      ticked={ticked}
      background={tour.analysis.background_analysis}
      busy={[work.request, work.pause, work.resume, work.background].some((m) => m.isPending)}
      error={last && failed ? `${last.name} not done: ${errorText(failed)}` : null}
      onOpenFlow={onOpenFlow}
      onRequest={
        allowed
          ? (flow) => run("request", `Analyse flow ${position(flow)}`, () => work.request.mutate(flow))
          : undefined
      }
      // A failed flow can be analysed again also after the analysis finished, as in its introduction.
      onRetry={
        allowed || state === "finished"
          ? (flow) => run("request", `Analyse flow ${position(flow)} again`, () => work.request.mutate(flow))
          : undefined
      }
      onPause={allowed ? () => run("pause", "Pause", () => work.pause.mutate()) : undefined}
      onResume={allowed ? () => run("resume", "Resume", () => work.resume.mutate()) : undefined}
      onBackground={
        allowed
          ? (enabled) =>
              run("background", enabled ? "Analyse all remaining flows" : "Stop", () =>
                work.background.mutate(enabled),
              )
          : undefined
      }
    />
  );
}
