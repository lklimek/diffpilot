import { Outlet, useParams } from "react-router";
import { useUserEventNotifications } from "../../app/userEvents";
import { AnalysisPathLinks } from "./AnalysisPathLinks";
import { AskChatProvider } from "./askChat";

/** The review of one analysis: Flows, Files and Dependencies share this layout, so the Ask chat survives switching. */
export function ReviewRoute() {
  const { analysisId = "" } = useParams();
  useUserEventNotifications();
  return (
    <AnalysisPathLinks key={analysisId} analysisId={analysisId}>
      <AskChatProvider analysisId={analysisId}>
        <Outlet />
      </AskChatProvider>
    </AnalysisPathLinks>
  );
}
