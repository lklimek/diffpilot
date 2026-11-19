import { useCallback, useState } from "react";

/** Browser preference; bots remain available in the API regardless of this setting. */
export const MENTION_BOTS_KEY = "sr.dashboard.mentions.with-bots";

export const MENTION_EXPANDED_KEY = "sr.dashboard.mentions.expanded";

export const useMentionBots = () => usePreference(MENTION_BOTS_KEY);
export const useMentionsExpanded = () => usePreference(MENTION_EXPANDED_KEY);

function usePreference(key: string): [boolean, (value: boolean) => void] {
  const [withBots, setWithBots] = useState(() => {
    try {
      return localStorage.getItem(key) === "true";
    } catch {
      return false;
    }
  });
  const set = useCallback(
    (value: boolean) => {
      setWithBots(value);
      try {
        localStorage.setItem(key, String(value));
      } catch {
        /* The current choice still applies. */
      }
    },
    [key],
  );
  return [withBots, set];
}
