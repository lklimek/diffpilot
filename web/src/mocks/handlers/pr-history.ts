import { HttpResponse } from "msw";
import { route } from "./common";
import type { PrHistory, PrHistoryItem, PrRef } from "../../api/generated/types";
import { scenario } from "../scenarios";
import { watches } from "../review-loop";
import { resolvedPr } from "./resolve-pr";
import { liveAnalysis } from "../live-analysis";
import { myAnalyses } from "./list-my-analyses";

const key = ({ repo, number }: PrRef) => `${repo.owner}/${repo.name}#${number}`.toLowerCase();
const opened = new Map<string, PrHistoryItem>();

export function recordPrOpened(pr: PrRef, title: string) {
  const at = new Date().toISOString();
  opened.set(key(pr), { pr, title, opened_at: at, last_activity_at: at });
}

export const resetPrHistory = () => opened.clear();

export const getPrHistory = route("listMyPrs", ({ request }) => {
  if (scenario().legacyPrHistory) return new HttpResponse(null, { status: 501 });
  const query = new URL(request.url).searchParams;
  const limit = Number(query.get("limit") ?? 50);
  const offset = Number(query.get("cursor") ?? 0);
  const visits = new Map(opened);
  if (scenario().emptyPrHistory)
    return HttpResponse.json<PrHistory>({ prs: [...visits.values()], next_cursor: null });
  const analysis = liveAnalysis();
  const { head_moved } = resolvedPr();
  for (const row of myAnalyses().analyses) {
    const previous = visits.get(key(row.pr));
    visits.set(key(row.pr), {
      ...previous,
      pr: row.pr,
      title: row.title,
      last_activity_at:
        previous && Date.parse(previous.last_activity_at) >= Date.parse(row.created_at)
          ? previous.last_activity_at
          : row.created_at,
      analysis: {
        id: row.id,
        status: row.status,
        requested_at: row.created_at,
        head_sha: row.id === analysis.id ? analysis.pr.head_sha : "",
        head_moved: head_moved && row.id === analysis.id,
      },
    });
  }
  for (const { pr, title, since, ended_at, last_auto_pass, state } of watches().watches) {
    const previous = visits.get(key(pr));
    const at = ended_at ?? last_auto_pass?.at ?? since;
    visits.set(key(pr), {
      ...previous,
      pr,
      title: previous?.title ?? title,
      last_activity_at:
        previous && Date.parse(previous.last_activity_at) >= Date.parse(at) ? previous.last_activity_at : at,
      watch: { state, since, ended_at },
    });
  }
  const prs = [...visits.values()].sort(
    (a, b) => Date.parse(b.last_activity_at) - Date.parse(a.last_activity_at),
  );
  const next = offset + limit;
  return HttpResponse.json<PrHistory>({
    prs: prs.slice(offset, next),
    next_cursor: next < prs.length ? String(next) : null,
  });
});
