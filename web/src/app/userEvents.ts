import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useEffectEvent } from "react";
import { useNavigate } from "react-router";
import { api } from "../api/client";
import type { LlmActivity, UserEvent } from "../api/generated/types";
import { useMe } from "../api/queries";
import { prPath } from "../screens/pr/paths";
import { SETTINGS_NOTIFICATIONS_PATH } from "../screens/shell/paths";
import { notifyWorkDone, useNotifySetting, type WorkEvent } from "./workNotifications";

export const llmActivityKey = (login: string) => ["llm-activity", login] as const;

/** The notification of a user event; its key is the event id, so every tab shows it once. */
export function userEventNotice(event: UserEvent): WorkEvent {
  return {
    key: `user-event:${event.id}`,
    title: "diffpilot",
    body: event.text,
    path: event.pr ? prPath(event.pr) : SETTINGS_NOTIFICATIONS_PATH,
  };
}

/** Follow job activity and optional browser notifications on the signed-in user's stream. */
export function useUserEvents(): void {
  const login = useMe().data?.user?.login;
  const client = useQueryClient();
  const navigate = useNavigate();
  const { enabled } = useNotifySetting();
  const notify = useEffectEvent((event: UserEvent) => {
    if (enabled) void notifyWorkDone(userEventNotice(event), (path) => void navigate(path));
  });
  useEffect(() => {
    if (!login || typeof EventSource === "undefined") return;
    const queryKey = llmActivityKey(login);
    return api.subscribeUserEvents({
      onEvent: (event) => notify(event),
      onActivity: (activity) => {
        void client.cancelQueries({ queryKey, exact: true });
        client.setQueryData(queryKey, activity);
      },
      onReconnect: () => {
        if (client.getQueryData<LlmActivity | null>(queryKey) !== null) {
          void client.invalidateQueries({ queryKey, exact: true });
        }
      },
    });
  }, [client, login]);
}
