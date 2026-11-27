import { Outlet, useOutletContext, useParams } from "react-router";
import type { PrRef } from "../../api/generated/types";
import { AnalysisPathLinks } from "./AnalysisPathLinks";
import { AskChatProvider } from "./askChat";

/** The review of one analysis: Flows, Files and Dependencies share this layout, so the Ask chat survives switching. */
export function ReviewRoute() {
  const { analysisId = "" } = useParams();
  const pr = useOutletContext<PrRef>();
  return (
    <AnalysisPathLinks key={analysisId} analysisId={analysisId}>
      <AskChatProvider analysisId={analysisId}>
        <Outlet context={pr} />
      </AskChatProvider>
    </AnalysisPathLinks>
  );
}
