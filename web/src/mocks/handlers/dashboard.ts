import { HttpResponse } from "msw";
import type {
  DashboardEvent,
  DashboardSummary,
  ItemPage,
  RefreshStatus,
  WorkItem,
} from "../../api/generated/types";
import summaryJson from "../../../../fixtures/dashboard/api/summary.json";
import itemsJson from "../../../../fixtures/dashboard/api/items.json";
import scanningJson from "../../../../fixtures/dashboard/api/refresh-scanning.json";
import rankingJson from "../../../../fixtures/dashboard/api/refresh-ranking.json";
import failedJson from "../../../../fixtures/dashboard/api/refresh-failed.json";
import { apiError, route } from "./common";

// Dashboard routes R1–R5 (docs/v1.1-dashboard-plan.md §E.2) answered from fixtures/dashboard; WP11-WEB-S adds the
// screen scenarios.

const SUMMARY = summaryJson as unknown as DashboardSummary;
const ITEMS = itemsJson as unknown as ItemPage;
const SCANNING = scanningJson as unknown as RefreshStatus;
const RANKING = rankingJson as unknown as RefreshStatus;
const REFRESHES: RefreshStatus[] = [
  ...(SUMMARY.latest_refresh ? [SUMMARY.latest_refresh] : []),
  SCANNING,
  RANKING,
  failedJson as unknown as RefreshStatus,
];

let live: RefreshStatus | null = null;

/** Forgets a started refresh between mock tests. */
export function resetDashboard(): void {
  live = null;
}

const findRefresh = (id: unknown) => (live?.id === id ? live : REFRESHES.find((r) => r.id === id));

/** `GET /api/v1/dashboard`. */
export const getDashboard = route("getDashboard", () =>
  HttpResponse.json({ ...SUMMARY, latest_refresh: live ?? SUMMARY.latest_refresh }),
);

/** `GET /api/v1/dashboard/items`: `tier`, `kind`, `limit` and `cursor` (`<revision>:<offset>`). */
export const listDashboardItems = route("listDashboardItems", ({ request }) => {
  const query = new URL(request.url).searchParams;
  const limit = Math.min(Number(query.get("limit") ?? 100), 500);
  const cursor = query.get("cursor");
  let offset = 0;
  if (cursor) {
    const [revision, at] = cursor.split(":");
    if (revision !== ITEMS.revision) {
      return apiError(409, "cursor_expired", "The list changed; load it again from the start.");
    }
    offset = Number(at);
  }
  const keep = (item: WorkItem) =>
    (!query.get("tier") || item.score.tier === query.get("tier")) &&
    (!query.get("kind") || item.kind === query.get("kind"));
  const matching = ITEMS.items.filter(keep);
  const next = offset + limit < matching.length ? `${ITEMS.revision}:${offset + limit}` : null;
  return HttpResponse.json({
    revision: ITEMS.revision,
    items: matching.slice(offset, offset + limit),
    next_cursor: next,
  });
});

/** `POST /api/v1/dashboard/refreshes`: 201, or 409 `refresh_running` while one is live. */
export const startDashboardRefresh = route("startDashboardRefresh", () => {
  if (live && (live.state === "scanning" || live.state === "ranking")) {
    return apiError(409, "refresh_running", "A refresh is already running.");
  }
  live = { ...SCANNING, started_at: new Date().toISOString() };
  return HttpResponse.json(live, { status: 201 });
});

/** `GET /api/v1/dashboard/refreshes/{id}`. */
export const getDashboardRefresh = route("getDashboardRefresh", ({ params }) => {
  const refresh = findRefresh(params.id);
  return refresh ? HttpResponse.json(refresh) : apiError(404, "not_found", "This refresh does not exist.");
});

/** `POST …/refreshes/{id}/cancel`. */
export const cancelDashboardRefresh = route("cancelDashboardRefresh", ({ params }) => {
  const refresh = findRefresh(params.id);
  if (!refresh) return apiError(404, "not_found", "This refresh does not exist.");
  const cancelled: RefreshStatus = { ...refresh, state: "cancelled" };
  if (live?.id === refresh.id) live = cancelled;
  return HttpResponse.json(cancelled);
});

/** `POST …/refreshes/{id}/rank` (Retry ranking): 202. */
export const rankDashboardRefresh = route("rankDashboardRefresh", ({ params }) => {
  const refresh = findRefresh(params.id);
  if (!refresh) return apiError(404, "not_found", "This refresh does not exist.");
  return HttpResponse.json({ ...refresh, ranking: RANKING.ranking }, { status: 202 });
});

/** `GET …/refreshes/{id}/events` (SSE): the current status, then a heartbeat every 15 s. */
export const dashboardRefreshEvents = route("dashboardRefreshEvents", ({ params, request }) => {
  const refresh = findRefresh(params.id);
  if (!refresh) return apiError(404, "not_found", "This refresh does not exist.");
  const encoder = new TextEncoder();
  let timer: ReturnType<typeof setInterval> | undefined;
  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      const send = (event: DashboardEvent) => {
        try {
          controller.enqueue(encoder.encode(`event: ${event.event}\ndata: ${JSON.stringify(event)}\n\n`));
        } catch {
          clearInterval(timer);
        }
      };
      send({ event: "refresh", status: refresh });
      timer = setInterval(() => send({ event: "heartbeat", at: new Date().toISOString() }), 15_000);
      request.signal.addEventListener("abort", () => clearInterval(timer));
    },
    cancel() {
      clearInterval(timer);
    },
  });
  return new HttpResponse(body, {
    headers: { "Content-Type": "text/event-stream", "Cache-Control": "no-store" },
  });
});
