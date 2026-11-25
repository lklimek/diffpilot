import { SETTINGS_NOTIFICATIONS_PATH } from "../screens/shell/paths";
// Browser notifications of the review loop (plan docs/review-workflow-plan.md §5.5, US16, RQ18): "questions ready"
// on a watched PR and "round executed", from the per-user event stream (RW3), under the existing per-browser
// "Notify me when work finishes" opt-in.
import { useEffect } from "react";
import { useNavigate } from "react-router";
import { api } from "../api/client";
import type { UserEvent } from "../api/generated/types";
import { commentsPath } from "../screens/pr/paths";
import { notifyWorkDone, useNotifySetting, type WorkEvent } from "./workNotifications";

/** The notification of a user event; its key is the event id, so every tab shows it once. */
export function userEventNotice(event: UserEvent): WorkEvent {
  return {
    key: `user-event:${event.id}`,
    title: "diffpilot",
    body: event.text,
    path: event.pr ? commentsPath(event.pr) : SETTINGS_NOTIFICATIONS_PATH,
  };
}

/** Opens the per-user event stream while notifications are on and shows its events (screens and layouts only). */
export function useUserEventNotifications(): void {
  const navigate = useNavigate();
  const { enabled } = useNotifySetting();
  useEffect(() => {
    if (!enabled || typeof EventSource === "undefined") return;
    return api.subscribeUserEvents({
      onEvent: (event) => void notifyWorkDone(userEventNotice(event), (path) => void navigate(path)),
      // The browser reconnects by itself; a closed stream ends with this page.
      onError: () => {},
    });
  }, [enabled, navigate]);
}
