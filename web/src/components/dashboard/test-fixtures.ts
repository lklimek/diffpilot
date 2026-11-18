// Dashboard fixtures (fixtures/dashboard/api, the approved mockup's data) typed for component tests.
import type { DashboardSummary, ItemPage, RefreshStatus, WorkItem } from "../../api/generated/types";
import summaryJson from "../../../../fixtures/dashboard/api/summary.json";
import itemsJson from "../../../../fixtures/dashboard/api/items.json";
import provisionalJson from "../../../../fixtures/dashboard/api/items-provisional.json";
import scanningJson from "../../../../fixtures/dashboard/api/refresh-scanning.json";
import rankingJson from "../../../../fixtures/dashboard/api/refresh-ranking.json";
import failedJson from "../../../../fixtures/dashboard/api/refresh-failed.json";

/** `fixtures/dashboard/api/summary.json`: finished refresh at 08:12, ranked 08:15. */
export const SUMMARY = summaryJson as unknown as DashboardSummary;
/** The 14 ranked items, every score final. */
export const ITEMS = (itemsJson as unknown as ItemPage).items;
/** The same items while a later refresh ranks (WUI-320 new, NC-482, CON-1534, PLAT-1210 changed). */
export const PROVISIONAL_ITEMS = (provisionalJson as unknown as ItemPage).items;
/** Scanning PR details 23 of 41. */
export const SCANNING = scanningJson as unknown as RefreshStatus;
/** Ranking 10 of 14. */
export const RANKING = rankingJson as unknown as RefreshStatus;
/** Failed on GitHub's rate limit, retry at 10:41. */
export const FAILED = failedJson as unknown as RefreshStatus;
/** The mockup's "now": Wed 18 Nov 2026, 10:27 UTC. */
export const NOW = new Date("2026-11-18T10:27:00Z");

/** The fixture item with display id `id`. */
export function itemById(id: string, items: WorkItem[] = ITEMS): WorkItem {
  const item = items.find((candidate) => candidate.id === id);
  if (!item) throw new Error(`no fixture item ${id}`);
  return item;
}
