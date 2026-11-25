// TODO(v1.4 phase B): replace with generated contract type (plan C17)
import type { AnalysisId, AnalysisStatus, PrRef, WatchState } from "../generated/types";

export const PR_HISTORY_PATH = "/api/v1/me/prs";
export const PR_HISTORY_PAGE_SIZE = 50;

export interface PrHistoryEntry {
  pr: PrRef;
  title: string;
  last_activity_at: string;
  opened_at?: string;
  analysis?: { id: AnalysisId; status: AnalysisStatus; head_sha: string; head_moved: boolean };
  watch?: { state: WatchState; since: string; ended_at: string | null };
}

export interface PrHistoryPage {
  prs: PrHistoryEntry[];
  next_cursor: string | null;
}

export async function fetchPrHistory(
  cursor: string | null,
  signal: AbortSignal,
): Promise<PrHistoryPage | null> {
  const query = new URLSearchParams({ limit: String(PR_HISTORY_PAGE_SIZE) });
  if (cursor !== null) query.set("cursor", cursor);
  const response = await fetch(`${PR_HISTORY_PATH}?${query}`, {
    headers: { Accept: "application/json" },
    credentials: "same-origin",
    signal,
  });
  if (
    [404, 405, 501].includes(response.status) ||
    response.headers.get("Content-Type")?.includes("text/html")
  )
    return null;
  if (!response.ok) throw new Error("Could not load pull request history. Try again.");
  return (await response.json()) as PrHistoryPage;
}
