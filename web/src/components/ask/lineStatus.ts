import type { AskQuestion, LineSide, Tour } from "../../api/generated/types";
import { awaitsFinalAnswer } from "../../api/questionHistory";
import { linesOf } from "./chat/model";

export type AskLineStatus = "running" | "answered" | "failed";
export type AskLineStatusLookup = (path: string, side: LineSide, line: number) => AskLineStatus | undefined;
export type AskStatusAtLine = (side: LineSide, line: number) => AskLineStatus | undefined;

/** Multiple asks at one line keep the in-progress mark until every active answer finishes. */
const STATUS_RANK: Record<AskLineStatus, number> = { running: 3, answered: 2, failed: 1 };

const lineKey = (side: LineSide, line: number) => `${side}:${line}`;

/** One marker on the last line of each explicit line selection, like a review comment. */
export function indexAskLineStatus(
  tour: Tour,
  questions: readonly AskQuestion[] | undefined,
): AskLineStatusLookup | undefined {
  const statusByFile = new Map<string, Map<string, AskLineStatus>>();
  for (const question of questions ?? []) {
    const { anchor } = question;
    const hasLines = anchor.kind === "lines" ? anchor.range : anchor.kind === "step" && anchor.lines;
    if (question.mine === false || !hasLines) continue;
    const location = linesOf(anchor, tour);
    if (location?.to == null) continue;

    const status =
      question.state === "failed" ? "failed" : awaitsFinalAnswer(question) ? "running" : "answered";
    const statuses = statusByFile.get(location.path) ?? new Map<string, AskLineStatus>();
    const key = lineKey(location.side, location.to);
    const previous = statuses.get(key);
    if (!previous || STATUS_RANK[status] > STATUS_RANK[previous]) statuses.set(key, status);
    statusByFile.set(location.path, statuses);
  }
  if (statusByFile.size > 0) return (path, side, line) => statusByFile.get(path)?.get(lineKey(side, line));
}
