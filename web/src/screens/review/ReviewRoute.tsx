import { Outlet, useParams } from "react-router";
import { AskChatProvider } from "./askChat";

/** The review of one analysis: Flows and Files share this layout, so the Ask chat survives switching between them. */
export function ReviewRoute() {
  const { analysisId = "" } = useParams();
  return (
    <AskChatProvider key={analysisId} analysisId={analysisId}>
      <Outlet />
    </AskChatProvider>
  );
}
