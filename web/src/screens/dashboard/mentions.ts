import { useCallback, useState } from "react";

/** Browser preference; bots remain available in the API regardless of this setting. */
export const MENTION_BOTS_KEY = "sr.dashboard.mentions.with-bots";

export function useMentionBots(): [boolean, (value: boolean) => void] {
  const [withBots, setWithBots] = useState(() => {
    try {
      return localStorage.getItem(MENTION_BOTS_KEY) === "true";
    } catch {
      return false;
    }
  });
  const set = useCallback((value: boolean) => {
    setWithBots(value);
    try {
      localStorage.setItem(MENTION_BOTS_KEY, String(value));
    } catch {
      /* The current choice still applies. */
    }
  }, []);
  return [withBots, set];
}
