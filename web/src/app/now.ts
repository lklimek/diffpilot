import { useEffect, useState } from "react";

/** Tick of `useNow`: elapsed times are shown in whole minutes, so a few seconds late is fine. */
export const NOW_TICK_MS = 10_000;

/** The current time (ms since the epoch), updated every `NOW_TICK_MS`. */
export function useNow(): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), NOW_TICK_MS);
    return () => clearInterval(timer);
  }, []);
  return now;
}
