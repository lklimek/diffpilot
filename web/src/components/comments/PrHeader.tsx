import type { WatchedPr, WatchState } from "../../api/generated/types";
import { clockText } from "../format/time";

/** Words of a watch state (plan §2.8). */
export const WATCH_STATE: Record<WatchState, string> = {
  active: "Watching",
  waiting_for_bot_round: "Waiting for the bot's round to end",
  job_running: "Assessment running",
  limit_reached: "Not assessed automatically: today's limit",
  paused: "Watching paused: sign in again",
  ended: "Watching ended",
};

/** The latest automatic assessment, e.g. "Assessed automatically 09:12 · 5 questions". */
export function autoPassText(watch: Pick<WatchedPr, "last_auto_pass">, now: Date): string | null {
  const pass = watch.last_auto_pass;
  if (!pass) return null;
  return `Assessed automatically ${clockText(pass.at, now)} · ${pass.questions} ${pass.questions === 1 ? "question" : "questions"}`;
}
