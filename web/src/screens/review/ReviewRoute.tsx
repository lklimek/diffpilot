import { Outlet, useParams } from "react-router";
import { useUserEventNotifications } from "../../app/userEvents";
import { AskChatProvider } from "./askChat";

/** The review of one analysis: Flows, Files and Dependencies share this layout, so the Ask chat survives switching. */
export function ReviewRoute() {
  const { analysisId = "" } = useParams();
  useUserEventNotifications();
  return (
    <AskChatProvider key={analysisId} analysisId={analysisId}>
      <Outlet />
    </AskChatProvider>
  );
}
