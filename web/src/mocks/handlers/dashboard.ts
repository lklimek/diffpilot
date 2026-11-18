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

/** The refresh started through R2, if any. */
let live: RefreshStatus | null = null;
/** Refreshes changed by cancel or Retry ranking, by id. */
const changed = new Map<string, RefreshStatus>();

/** Forgets started and changed refreshes between mock tests. */
export function resetDashboard(): void {
  live = null;
  changed.clear();
}

const findRefresh = (id: unknown): RefreshStatus | undefined =>
  changed.get(String(id)) ?? (live !== null && live.id === id ? live : REFRESHES.find((r) => r.id === id));

/** Stores a changed refresh. */
function store(refresh: RefreshStatus): RefreshStatus {
  changed.set(refresh.id, refresh);
  if (live?.id === refresh.id) live = refresh;
  return refresh;
}

const isLive = (refresh: RefreshStatus) => refresh.state === "scanning" || refresh.state === "ranking";

/** `GET /api/v1/dashboard`. */
export const getDashboard = route("getDashboard", () =>
  HttpResponse.json({ ...SUMMARY, latest_refresh: live ?? SUMMARY.latest_refresh }),
);

/**
 * `GET /api/v1/dashboard/items`: `tier`, `kind` (`pr` | `issue`; absent = both; TODOs are listed whatever it
 * says), `todos=false` (no TODOs), `limit` and `cursor` (`<revision>:<offset>`).
 */
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
    (item.kind === "todo"
      ? query.get("todos") !== "false"
      : !query.get("kind") || item.kind === query.get("kind"));
  const matching = ITEMS.items.filter(keep);
  const next = offset + limit < matching.length ? `${ITEMS.revision}:${offset + limit}` : null;
  return HttpResponse.json({
    revision: ITEMS.revision,
    items: matching.slice(offset, offset + limit),
    next_cursor: next,
  });
});

/**
 * `POST /api/v1/dashboard/refreshes` (http-api.md "Refresh states"): 201, superseding a live ranking (it finishes
 * with ranking `cancelled`); 409 `refresh_running` while one scans.
 */
export const startDashboardRefresh = route("startDashboardRefresh", () => {
  const known = [live, ...changed.values()].filter((r): r is RefreshStatus => r !== null);
  if (known.some((r) => r.state === "scanning")) {
    return apiError(409, "refresh_running", "A refresh is already scanning.");
  }
  for (const r of known) {
    if (r.state === "ranking" && r.ranking) {
      store({ ...r, state: "finished", ranking: { ...r.ranking, state: "cancelled" } });
    }
  }
  changed.delete(SCANNING.id);
  live = { ...SCANNING, started_at: new Date().toISOString() };
  return HttpResponse.json(live, { status: 201 });
});

/** `GET /api/v1/dashboard/refreshes/{id}`. */
export const getDashboardRefresh = route("getDashboardRefresh", ({ params }) => {
  const refresh = findRefresh(params.id);
  return refresh ? HttpResponse.json(refresh) : apiError(404, "not_found", "This refresh does not exist.");
});

/**
 * `POST …/refreshes/{id}/cancel` (http-api.md "Refresh states"): a scan becomes `cancelled`, a ranking ends the
 * refresh `finished` with ranking `cancelled`; an ended refresh answers 409 `conflict`.
 */
export const cancelDashboardRefresh = route("cancelDashboardRefresh", ({ params }) => {
  const refresh = findRefresh(params.id);
  if (!refresh) return apiError(404, "not_found", "This refresh does not exist.");
  if (refresh.state === "scanning") return HttpResponse.json(store({ ...refresh, state: "cancelled" }));
  if (refresh.state === "ranking" && refresh.ranking) {
    return HttpResponse.json(
      store({ ...refresh, state: "finished", ranking: { ...refresh.ranking, state: "cancelled" } }),
    );
  }
  return apiError(409, "conflict", "This refresh has already ended.");
});

/**
 * `POST …/refreshes/{id}/rank` (Retry ranking): 202 for a `finished` refresh whose ranking ended `failed`,
 * `interrupted` or `cancelled`; 409 `refresh_running` while another refresh is live, else 409 `conflict`.
 */
export const rankDashboardRefresh = route("rankDashboardRefresh", ({ params }) => {
  const refresh = findRefresh(params.id);
  if (!refresh) return apiError(404, "not_found", "This refresh does not exist.");
  const retryable = ["failed", "interrupted", "cancelled"];
  if (refresh.state !== "finished" || !refresh.ranking || !retryable.includes(refresh.ranking.state)) {
    return apiError(409, "conflict", "This ranking cannot be retried.");
  }
  if ([live, ...changed.values()].some((r) => r !== null && r.id !== refresh.id && isLive(r))) {
    return apiError(409, "refresh_running", "A refresh is already running.");
  }
  const ranking = { ...refresh.ranking, state: "running" as const, message: null };
  return HttpResponse.json(store({ ...refresh, state: "ranking", ranking }), { status: 202 });
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
