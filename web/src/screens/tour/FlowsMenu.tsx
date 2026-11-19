import type { FlowId, StepId, Tour } from "../../api/generated/types";
import type { useFlowWork } from "../../api/queries";
import { FlowsProgressMenu } from "../../components/top-bar/FlowsProgressMenu";
import { errorText } from "./code";

/** The top bar's flows dropdown of both review views, wired to the analysis work requests. */
export function FlowsMenu({
  tour,
  ticked,
  work,
  onOpenFlow,
}: {
  tour: Tour;
  ticked: readonly StepId[];
  work: ReturnType<typeof useFlowWork>;
  onOpenFlow: (flow: FlowId) => void;
}) {
  const allowed = ["planning", "running", "paused"].includes(tour.analysis.state.state);
  const failed = [work.request, work.pause, work.resume, work.background].find((m) => m.isError);
  return (
    <FlowsProgressMenu
      flows={tour.flows}
      analysis={tour.analysis.state}
      ticked={ticked}
      background={tour.analysis.background_analysis}
      busy={[work.request, work.pause, work.resume, work.background].some((m) => m.isPending)}
      error={failed ? `Not done: ${errorText(failed.error)}` : null}
      onOpenFlow={onOpenFlow}
      onRequest={allowed ? (flow) => work.request.mutate(flow) : undefined}
      onPause={allowed ? () => work.pause.mutate() : undefined}
      onResume={allowed ? () => work.resume.mutate() : undefined}
      onBackground={allowed ? (enabled) => work.background.mutate(enabled) : undefined}
    />
  );
}
