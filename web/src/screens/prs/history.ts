import type { MyAnalysis, PrRef, WatchedPr, PrHistoryItem } from "../../api/generated/types";

export type HistoryRow = Omit<PrHistoryItem, "analysis" | "watch"> & {
  analysis?: Pick<NonNullable<PrHistoryItem["analysis"]>, "id" | "status"> & {
    requestedAt: string;
    headMoved?: boolean;
  };
  watch?: NonNullable<PrHistoryItem["watch"]>;
};

export const historyKey = ({ repo, number }: PrRef) => `${repo.owner}/${repo.name}#${number}`.toLowerCase();

export function historyFromLegacy(
  analyses: readonly MyAnalysis[],
  watches: readonly WatchedPr[],
): HistoryRow[] {
  const entries = new Map<string, HistoryRow>();
  for (const { pr, title, created_at, id, status } of analyses) {
    const key = historyKey(pr);
    const previous = entries.get(key)?.analysis?.requestedAt;
    if (previous && Date.parse(previous) >= Date.parse(created_at)) continue;
    entries.set(key, {
      pr,
      title,
      last_activity_at: created_at,
      analysis: { id, status, requestedAt: created_at },
    });
  }
  for (const { pr, title, state, since, ended_at, last_auto_pass } of watches) {
    const key = historyKey(pr);
    const previous = entries.get(key);
    const stamp = ended_at ?? last_auto_pass?.at ?? since;
    const latest = Date.parse(stamp) >= Date.parse(previous?.last_activity_at ?? "") || !previous;
    entries.set(key, {
      pr,
      title: latest ? title : previous.title,
      last_activity_at: latest ? stamp : previous.last_activity_at,
      analysis: previous?.analysis,
      watch: { state, since, ended_at },
    });
  }
  return [...entries.values()].sort(
    (a, b) => Date.parse(b.last_activity_at) - Date.parse(a.last_activity_at),
  );
}

export function historyFromServer({ analysis, ...entry }: PrHistoryItem): HistoryRow {
  return {
    ...entry,
    analysis: analysis && {
      id: analysis.id,
      status: analysis.status,
      requestedAt: analysis.requested_at,
      headMoved: analysis.head_moved,
    },
  };
}
