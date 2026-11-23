import { HttpResponse } from "msw";
import type {
  DashboardEvent,
  ExcludedItem,
  Mention,
  DashboardSummary,
  ItemPage,
  RefreshStatus,
  ScanStep,
  SnapshotMeta,
  WorkItem,
} from "../../api/generated/types";
import summaryJson from "../../../../fixtures/dashboard/api/summary.json";
import itemsJson from "../../../../fixtures/dashboard/api/items.json";
import provisionalJson from "../../../../fixtures/dashboard/api/items-provisional.json";
import scanningJson from "../../../../fixtures/dashboard/api/refresh-scanning.json";
import rankingJson from "../../../../fixtures/dashboard/api/refresh-ranking.json";
import failedJson from "../../../../fixtures/dashboard/api/refresh-failed.json";
import { setFixtureState } from "../fixtures";
import { setScenario } from "../scenarios";
import { apiError, route } from "./common";

// Dashboard routes R1–R5 (docs/v1.1-dashboard-plan.md §E.2) answered from fixtures/dashboard. `?dashboard=<name>`
// selects a scenario (the mockup states, DASHBOARD_SCENARIOS); a refresh started with R2 (or a ranking restarted
// with R4) then follows a short script through scanning, ranking and finished, read alike by R1, R2b and the SSE.

const MENTIONS: Mention[] = Array.from({ length: 5 }, (_, i) => ({
  id: `mention-${i}`,
  item_key: "acme~platform~pr~1203",
  repo: { owner: "acme", name: "platform" },
  number: 1203,
  kind: "pr",
  title: "A quick question",
  author: i === 4 ? "helper" : "bob",
  partly_read: false,
  is_bot: i === 4,
  created_at: `2026-11-18T0${i}:00:00Z`,
  text: "@alice <script>please respond</script>",
  url: `https://github.com/acme/platform/pull/1203#issuecomment-${i + 1}`,
}));
const SUMMARY = summaryJson as unknown as DashboardSummary;
const ITEMS = itemsJson as unknown as ItemPage;
const PROVISIONAL = provisionalJson as unknown as ItemPage;
const SCANNING = scanningJson as unknown as RefreshStatus;
const RANKING = rankingJson as unknown as RefreshStatus;
const FAILED = failedJson as unknown as RefreshStatus;
const FIXTURE_REFRESHES: RefreshStatus[] = [SUMMARY.latest_refresh!, SCANNING, RANKING, FAILED];

/** When this module loaded (the page in the browser): the error scenario's rate limit resets 14 min later. */
const LOADED = Date.now();
/** Duration of one step of a scripted refresh. */
export const SCRIPT_STEP_MS = 500;

/** The state of a scenario before any R2–R4: summary and items. */
interface Base {
  summary: DashboardSummary;
  items: ItemPage;
}

/** Snapshot of the ranking refresh: scanned 09:24, not ranked yet, one item not judged. */
const PROVISIONAL_SNAPSHOT: SnapshotMeta = {
  ...SUMMARY.snapshot!,
  refresh_id: RANKING.id,
  scanned_at: RANKING.scanned_at!,
  ranked_at: null,
  item_counts: { ...SUMMARY.snapshot!.item_counts, not_judged: 1 },
};

type RankingState = NonNullable<RefreshStatus["ranking"]>["state"];

/** The ranking refresh holding the provisional snapshot, its ranking in `state`. */
const ranking = (state: RankingState, message: string | null = null): Base => ({
  summary: {
    ...SUMMARY,
    revision: PROVISIONAL.revision,
    latest_refresh: {
      ...RANKING,
      state: ["running", "waiting_for_analysis", "paused"].includes(state) ? "ranking" : "finished",
      ranking: { ...RANKING.ranking!, state, message },
    },
    snapshot: PROVISIONAL_SNAPSHOT,
  },
  items: PROVISIONAL,
});

/** NC-482 (the review module's fixture PR) with its review link changed. */
const withReview = (review: (current: WorkItem["review"]) => WorkItem["review"]): Base => ({
  summary: SUMMARY,
  items: {
    ...ITEMS,
    items: ITEMS.items.map((item) =>
      item.id === "NC-482" ? { ...item, review: review(item.review) } : item,
    ),
  },
});

/** No time zone stored, so the schedule cannot run yet. */
const noTimeZone = (): Base => ({
  summary: { ...SUMMARY, schedule: { ...SUMMARY.schedule, time_zone: null, next_run_at: null } },
  items: ITEMS,
});

const SCENARIOS = {
  /** A finished manual refresh of 08:12, ranked at 08:15. */
  normal: (): Base => ({ summary: SUMMARY, items: ITEMS }),
  /** A scan running for 47 s (PR details 23 of 41); the list is the 08:12 snapshot. */
  refreshing: (): Base => {
    const started_at = new Date(LOADED - 47_000).toISOString();
    return { summary: { ...SUMMARY, latest_refresh: { ...SCANNING, started_at } }, items: ITEMS };
  },
  /** The scan finished at 09:24; the LLM ranks (10 of 13); changed and new items are provisional. */
  provisional: () => ranking("running"),
  /** As provisional, but the ranking waits for a review analysis holding the LLM credential. */
  waiting: () => ranking("waiting_for_analysis"),
  /** The ranking failed: the estimates stay, "Rank again" (R4) restarts it. */
  "ranking-failed": () => ranking("failed", "The LLM stopped answering"),
  /** Nothing needs the user; three own PRs wait on others. */
  empty: (): Base => {
    const counts = SUMMARY.snapshot!.item_counts;
    const zero = { ...counts, total: 0, pr: 0, issue: 0, todo: 0, p0: 0, p1: 0, p2: 0, p3: 0, not_judged: 0 };
    return {
      summary: {
        ...SUMMARY,
        mentions: [],
        revision: "r-1029-empty",
        snapshot: { ...SUMMARY.snapshot!, item_counts: zero, checked: { pr: 38, issue: 21, todo: 29 } },
      },
      items: {
        mention_coverage: SUMMARY.mention_coverage,
        mentions: [],
        revision: "r-1029-empty",
        items: [],
        next_cursor: null,
      },
    };
  },
  /** The last scan stopped on GitHub's rate limit; the 08:12 snapshot stays; Refresh waits for the reset. */
  error: (): Base => {
    const retry_at = new Date(LOADED + 14 * 60_000).toISOString();
    const started_at = new Date(LOADED - 70_000).toISOString();
    return {
      summary: {
        ...SUMMARY,
        latest_refresh: { ...FAILED, started_at, failure: { ...FAILED.failure!, retry_at } },
      },
      items: ITEMS,
    };
  },
  /** MemCan was unreachable in the latest scan: its TODO rows are kept from a scan 32 min earlier. */
  "memcan-down": (): Base => {
    const snapshot = SUMMARY.snapshot!;
    const read_at = new Date(Date.parse(snapshot.scanned_at) - 32 * 60_000).toISOString();
    return {
      summary: {
        ...SUMMARY,
        snapshot: {
          ...snapshot,
          todos: { status: "not_refreshed", read_at, failed_at: snapshot.scanned_at },
        },
      },
      items: ITEMS,
    };
  },
  /** Quick replies, including one bot, independent of ranked items. */
  mentions: (): Base => ({
    summary: { ...SUMMARY, mentions: MENTIONS },
    items: { ...ITEMS, mentions: MENTIONS },
  }),
  "mention-coverage": (): Base => ({
    summary: {
      ...SUMMARY,
      mention_coverage: {
        unchecked_items: 5,
        unchecked_logins: ["alice-helper"],
        timed_out_logins: ["slow"],
      },
    },
    items: ITEMS,
  }),
  "mention-hide-gone": (): Base => ({
    summary: { ...SUMMARY, mentions: MENTIONS },
    items: ITEMS,
  }),
  "mention-hide-fails": (): Base => ({
    summary: { ...SUMMARY, mentions: MENTIONS },
    items: { ...ITEMS, mentions: MENTIONS },
  }),
  /** No refresh yet: no snapshot, no items. */
  "first-run": (): Base => ({
    summary: { ...SUMMARY, mentions: [], revision: "r-none", latest_refresh: null, snapshot: null },
    items: {
      mention_coverage: SUMMARY.mention_coverage,
      mentions: [],
      revision: "r-none",
      items: [],
      next_cursor: null,
    },
  }),
  /** First dashboard visit: no time zone stored, so the schedule cannot run yet (R6b stores it). */
  "no-time-zone": noTimeZone,
  /** As no-time-zone, but R6b refuses the browser's zone (422): the note points to Settings. */
  "no-time-zone-refused": noTimeZone,
  /** NC-482 without an analysis: its row opens the review start page, which offers a new analysis. */
  "review-new": () => withReview(() => null),
  /** NC-482 analysed at its current head: its row opens the review (fixture state `resume`). */
  "review-found": () => withReview((review) => review && { ...review, head_moved: false }),
  /** NC-482 moved since its analysis: the review start page says so (scenario `head-moved`). */
  "review-moved": () => withReview((review) => review),
};

/** A dashboard scenario name. */
export type DashboardScenario = keyof typeof SCENARIOS;
/** Every dashboard scenario. */
export const DASHBOARD_SCENARIOS = Object.keys(SCENARIOS) as DashboardScenario[];

/** A refresh driven by the script: its status follows the time since `startedMs`, from step `from`. */
interface Run {
  base: RefreshStatus;
  from: number;
  startedMs: number;
}

let scenarioName: DashboardScenario = "normal";
let base: Base = SCENARIOS.normal();
/** The scripted refresh, if R2 or R4 started one; it stays the latest after it finished. */
let run: Run | null = null;
/** The scripted refresh that held the snapshot before `run` replaced it. */
let previous: RefreshStatus | null = null;
/** Refreshes changed by cancel or supersede, by id (they no longer follow the script). */
const changed = new Map<string, RefreshStatus>();
/** Hides persist across mock refreshes. */
const hiddenMentions = new Set<string>();
/** Snoozed item keys with the time they wake (R12); they persist across mock refreshes. */
const snoozed = new Map<string, string>();
/** Pull requests watched from My work (RW1, keys); their rows count the comments the watch read (none). */
const watchedPrs = new Set<string>();
/** The time zone stored through R6b. */
let timeZone: string | null = null;
let runCount = 0;

/** Forgets started and changed refreshes and the stored time zone (the scenario stays). */
export function resetDashboard(): void {
  base = SCENARIOS[scenarioName]();
  run = null;
  previous = null;
  changed.clear();
  hiddenMentions.clear();
  snoozed.clear();
  watchedPrs.clear();
  timeZone = null;
}

/**
 * Selects a dashboard scenario and resets the mock state. The review scenarios also select the matching state of
 * the review module's fixture PR, so the review start page agrees with the row.
 */
export function setDashboardScenario(name: DashboardScenario): void {
  if (!Object.hasOwn(SCENARIOS, name)) throw new Error(`unknown dashboard scenario "${name}"`);
  scenarioName = name;
  resetDashboard();
  if (name === "review-moved") setScenario("head-moved");
  if (name === "review-found") setFixtureState("resume");
}

/**
 * Records the time zone stored through `PUT /api/v1/settings/dashboard/time-zone`; false when the scenario
 * refuses it (the handler answers 422).
 */
export function noteDashboardTimeZone(zone: string): boolean {
  if (scenarioName === "no-time-zone-refused") return false;
  timeZone = zone;
  return true;
}

const requested = new URLSearchParams(globalThis.location?.search ?? "").get("dashboard");
if (requested !== null) setDashboardScenario(requested as DashboardScenario);

const KEYS: ScanStep["key"][] = [
  "search_prs",
  "issues",
  "pr_details",
  "context",
  "history",
  "todos",
  "ranking",
];

/** Steps with step `at` running (with its counts and detail), the ones before done, the rest waiting. */
const stepsAt = (
  at: number,
  detail: string | null,
  done: number | null = null,
  total: number | null = null,
) =>
  KEYS.map((key, i): ScanStep =>
    i < at
      ? { key, state: "done", done: null, total: null, detail: null }
      : i === at
        ? { key, state: "running", done, total, detail }
        : { key, state: "waiting", done: null, total: null, detail: null },
  );

/** The script: scanning (steps 0–4), ranking (5–6), finished (7). */
function scripted(r: Run, now: number): RefreshStatus {
  const at = Math.min(r.from + Math.floor((now - r.startedMs) / SCRIPT_STEP_MS), 7);
  const b = r.base;
  const scannedAt = b.scanned_at ?? new Date(r.startedMs + 5 * SCRIPT_STEP_MS).toISOString();
  const scan = (percent: number, steps: ScanStep[]): RefreshStatus => ({
    ...b,
    state: "scanning",
    percent,
    steps,
    scanned_at: null,
    ranking: null,
  });
  const rank = (done: boolean, judged: number): RefreshStatus => ({
    ...b,
    state: done ? "finished" : "ranking",
    percent: 100,
    steps: stepsAt(done ? 7 : 6, null, judged, ITEMS.items.length),
    scanned_at: scannedAt,
    judged,
    reused: 23,
    ranking: {
      state: done ? "finished" : "running",
      judged,
      total: ITEMS.items.length,
      resumes_at: null,
      message: null,
    },
  });
  switch (at) {
    case 0:
      return scan(10, stepsAt(0, "authored by you · page 1"));
    case 1:
      return scan(30, stepsAt(1, "mentions"));
    case 2:
      return scan(45, stepsAt(2, "acme/platform#1199", 23, 41));
    case 3:
      return scan(70, stepsAt(3, "owned paths · releases"));
    case 4:
      return scan(90, stepsAt(5, "2 MemCan projects", 1, 2));
    case 5:
      return rank(false, 0);
    case 6:
      return rank(false, 7);
    default:
      return rank(true, ITEMS.items.length);
  }
}

const runStatus = (): RefreshStatus | null =>
  run === null ? null : (changed.get(run.base.id) ?? scripted(run, Date.now()));

const isLive = (refresh: RefreshStatus) => refresh.state === "scanning" || refresh.state === "ranking";
const holdsSnapshot = (refresh: RefreshStatus | null): refresh is RefreshStatus =>
  refresh !== null &&
  refresh.scanned_at !== null &&
  (refresh.state === "ranking" || refresh.state === "finished");

function latest(): RefreshStatus | null {
  const own = runStatus();
  if (own) return own;
  const fixture = base.summary.latest_refresh;
  return fixture && (changed.get(fixture.id) ?? fixture);
}

function findRefresh(id: unknown): RefreshStatus | undefined {
  const stored = changed.get(String(id));
  if (stored) return stored;
  if (run !== null && run.base.id === id) return scripted(run, Date.now());
  const fixture = base.summary.latest_refresh;
  if (fixture !== null && fixture.id === id) return fixture;
  return FIXTURE_REFRESHES.find((r) => r.id === id);
}

/** Summary and items of the current snapshot: a scripted refresh's once it scanned, else the scenario's. */
/** The scripted summary and items with the snoozed items moved to Excluded (reason `snoozed`, owner SN1). */
function current(): Base {
  const view = fromScript();
  const now = Date.now();
  for (const [key, until] of snoozed) if (Date.parse(until) <= now) snoozed.delete(key);
  const snapshot = view.summary.snapshot;
  const asleep = view.items.items.filter((item) => snoozed.has(item.key) && item.score.tier !== "p0");
  if (!snapshot || asleep.length === 0) return view;
  const revision = `${view.items.revision}-z${hashOf([...snoozed])}`;
  const exclusions = snapshot.exclusions ?? { total: 0, counts: [], items: [] };
  const listed: ExcludedItem[] = asleep.map((item) => ({
    key: item.key,
    kind: item.kind,
    id: item.id,
    repo: item.repo,
    number: item.number,
    url: item.url,
    title: item.title,
    author: item.author,
    reason: "snoozed",
    merged_into: null,
    snoozed_until: snoozed.get(item.key)!,
  }));
  return {
    summary: {
      ...view.summary,
      revision,
      snapshot: {
        ...snapshot,
        excluded: snapshot.excluded + asleep.length,
        exclusions: {
          total: exclusions.total + asleep.length,
          counts: [...exclusions.counts, { reason: "snoozed", count: asleep.length }],
          items: [...listed, ...(exclusions.items ?? [])],
        },
      },
    },
    items: {
      ...view.items,
      revision,
      items: view.items.items.filter((item) => !asleep.includes(item)),
    },
  };
}

function fromScript(): Base {
  const holder = [runStatus(), previous].find(holdsSnapshot);
  if (!holder) return { summary: { ...base.summary, latest_refresh: latest() }, items: base.items };
  const ranked = holder.ranking?.state === "finished";
  const revision = `r-${holder.id.slice(-4)}-${holder.ranking?.state}-${holder.ranking?.judged}`;
  const snapshot: SnapshotMeta = {
    ...(ranked ? SUMMARY.snapshot! : PROVISIONAL_SNAPSHOT),
    refresh_id: holder.id,
    scanned_at: holder.scanned_at!,
    ranked_at: ranked ? holder.scanned_at : null,
  };
  return {
    summary: { ...base.summary, revision, latest_refresh: latest(), snapshot },
    items: { ...(ranked ? ITEMS : PROVISIONAL), revision },
  };
}

/** Stores a changed refresh; one driven by the script stops following it. */
function store(refresh: RefreshStatus): RefreshStatus {
  changed.set(refresh.id, refresh);
  return refresh;
}

/** `GET /api/v1/dashboard`. */
export const getDashboard = route("getDashboard", () => {
  const { summary } = current();
  const time_zone = timeZone ?? summary.schedule.time_zone;
  const next_run_at = summary.schedule.next_run_at ?? (time_zone ? SUMMARY.schedule.next_run_at : null);
  return HttpResponse.json({
    ...summary,
    mentions: (summary.mentions ?? []).filter((m) => !hiddenMentions.has(m.id)),
    schedule: { ...summary.schedule, time_zone, next_run_at },
  });
});

/** A short hash of a value's JSON. */
function hashOf(body: unknown): string {
  let hash = 5381;
  for (const char of JSON.stringify(body)) hash = ((hash * 33) ^ char.charCodeAt(0)) >>> 0;
  return hash.toString(16);
}

/** Weak ETag of a body: a short hash of its JSON. */
const etagOf = (body: unknown): string => `W/"${hashOf(body)}"`;

/**
 * `GET /api/v1/dashboard/items`: `tier`, `kind` (`pr` | `issue`; absent = both; TODOs are listed whatever it
 * says), `todos=false` (no TODOs), `limit` and `cursor` (`<revision>:<offset>`); weak `ETag`, and 304 for a
 * matching `If-None-Match`.
 */
export const listDashboardItems = route("listDashboardItems", ({ request }) => {
  const { items: page, summary } = current();
  const query = new URL(request.url).searchParams;
  const limit = Math.min(Number(query.get("limit") ?? 100), 500);
  const cursor = query.get("cursor");
  let offset = 0;
  if (cursor) {
    const [revision, at] = cursor.split(":");
    if (revision !== page.revision) {
      return apiError(409, "cursor_expired", "The list changed; load it again from the start.");
    }
    offset = Number(at);
  }
  const keep = (item: WorkItem) =>
    (!query.get("tier") || item.score.tier === query.get("tier")) &&
    (item.kind === "todo"
      ? query.get("todos") !== "false"
      : !query.get("kind") || item.kind === query.get("kind"));
  const matching = page.items.filter(keep);
  const next = offset + limit < matching.length ? `${page.revision}:${offset + limit}` : null;
  const body: ItemPage = {
    mention_coverage: summary.mention_coverage,
    mentions: cursor ? [] : (page.mentions ?? []).filter((m) => !hiddenMentions.has(m.id)),
    revision: page.revision,
    items: matching
      .slice(offset, offset + limit)
      .map((item) =>
        watchedPrs.has(item.key) && !item.comments
          ? { ...item, comments: { total: 0, needs_you: 0 }, watched: true }
          : item,
      ),
    next_cursor: next,
  };
  const headers = { ETag: etagOf(body), "Cache-Control": "no-store" };
  if (request.headers.get("If-None-Match") === headers.ETag) {
    return new HttpResponse(null, { status: 304, headers });
  }
  return HttpResponse.json(body, { headers });
});

/**
 * RW1 PUT from a My work row: the server reads the pull request's comments, then watches it. The review module's
 * fixture PR falls through to its own handler (review-loop.ts).
 */
export const watchDashboardPr = route("watchPr", ({ params }) => {
  const { owner, name, number } = params as { owner: string; name: string; number: string };
  if (owner === "acme" && name === "node-client" && number === "482") return undefined;
  watchedPrs.add(`${owner}~${name}~pr~${number}`);
  return new HttpResponse(null, { status: 204 });
});

/** Starts the script for `refresh` from step `from`; the replaced run keeps its snapshot as `previous`. */
function startRun(refresh: RefreshStatus, from: number): RefreshStatus {
  const own = runStatus();
  if (run && run.base.id !== refresh.id && holdsSnapshot(own)) previous = own;
  changed.delete(refresh.id);
  run = { base: refresh, from, startedMs: Date.now() };
  return runStatus()!;
}

/**
 * `POST /api/v1/dashboard/refreshes` (http-api.md "Refresh states"): 201, superseding a live ranking (it finishes
 * with ranking `cancelled`); 409 `refresh_running` while one scans; 429 `github_rate_limited` before a known
 * rate-limit reset.
 */
export const startDashboardRefresh = route("startDashboardRefresh", () => {
  const known = [latest(), ...changed.values()].filter((r): r is RefreshStatus => r !== null);
  if (known.some((r) => r.state === "scanning")) {
    return apiError(409, "refresh_running", "A refresh is already scanning.");
  }
  const retryAt = latest()?.failure?.retry_at;
  if (retryAt && Date.parse(retryAt) > Date.now()) {
    return apiError(
      429,
      "github_rate_limited",
      "GitHub's rate limit is still reached. Retry after its reset.",
    );
  }
  for (const r of known) {
    if (r.state === "ranking" && r.ranking) {
      store({ ...r, state: "finished", ranking: { ...r.ranking, state: "cancelled" } });
    }
  }
  runCount += 1;
  const id = `8b0d6c1e-1118-4d2a-9c3b-${String(2000 + runCount).padStart(12, "0")}`;
  const started = {
    ...SCANNING,
    id,
    started_at: new Date().toISOString(),
    log: ["refresh started (manual)"],
  };
  return HttpResponse.json(startRun(started, 0), { status: 201 });
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
 * `interrupted` or `cancelled`; 409 `refresh_running` while another refresh is live, else 409 `conflict`. The
 * ranking then follows the script to `finished`.
 */
export const rankDashboardRefresh = route("rankDashboardRefresh", ({ params }) => {
  const refresh = findRefresh(params.id);
  if (!refresh) return apiError(404, "not_found", "This refresh does not exist.");
  const retryable = ["failed", "interrupted", "cancelled"];
  if (refresh.state !== "finished" || !refresh.ranking || !retryable.includes(refresh.ranking.state)) {
    return apiError(409, "conflict", "This ranking cannot be retried.");
  }
  if ([latest(), ...changed.values()].some((r) => r !== null && r.id !== refresh.id && isLive(r))) {
    return apiError(409, "refresh_running", "A refresh is already running.");
  }
  const restarted = startRun({ ...refresh, judged: 0, ranking: { ...refresh.ranking, message: null } }, 5);
  return HttpResponse.json(restarted, { status: 202 });
});

/** `GET …/refreshes/{id}/events` (SSE): the current status and each change of it; a heartbeat every 15 s. */
export const dashboardRefreshEvents = route("dashboardRefreshEvents", ({ params, request }) => {
  if (!findRefresh(params.id)) return apiError(404, "not_found", "This refresh does not exist.");
  const encoder = new TextEncoder();
  let heartbeat: ReturnType<typeof setInterval> | undefined;
  let watch: ReturnType<typeof setInterval> | undefined;
  const stop = () => {
    clearInterval(heartbeat);
    clearInterval(watch);
  };
  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      const send = (event: DashboardEvent) => {
        try {
          controller.enqueue(encoder.encode(`event: ${event.event}\ndata: ${JSON.stringify(event)}\n\n`));
        } catch {
          stop();
        }
      };
      let last = "";
      const update = () => {
        const status = findRefresh(params.id);
        const text = JSON.stringify(status);
        if (status && text !== last) {
          last = text;
          send({ event: "refresh", status });
        }
      };
      update();
      watch = setInterval(update, SCRIPT_STEP_MS / 5);
      heartbeat = setInterval(() => send({ event: "heartbeat", at: new Date().toISOString() }), 15_000);
      request.signal.addEventListener("abort", stop);
    },
    cancel: stop,
  });
  return new HttpResponse(body, {
    headers: { "Content-Type": "text/event-stream", "Cache-Control": "no-store" },
  });
});

/** Session-only hide, persisted for the mock session. */
export const hideDashboardMention = route("hideDashboardMention", ({ params }) => {
  if (scenarioName === "mention-hide-fails") return apiError(500, "internal", "Could not hide this mention");
  hiddenMentions.add(String(params.id));
  if (scenarioName === "mention-hide-gone")
    return apiError(404, "not_found", "The requested item was not found.");
  return new HttpResponse(null, { status: 204 });
});

/** `PUT /api/v1/dashboard/items/{key}/snooze` (R12): 1–365 days for an item of the list, else 400 or 404. */
export const snoozeDashboardItem = route("snoozeDashboardItem", async ({ params, request }) => {
  const key = String(params.key);
  const body = (await request.json().catch(() => null)) as { days?: unknown } | null;
  const days = body?.days;
  if (typeof days !== "number" || !Number.isInteger(days) || days < 1 || days > 365) {
    return apiError(400, "invalid_request", "Snooze for 1 to 365 days.");
  }
  if (!fromScript().items.items.some((item) => item.key === key)) {
    return apiError(404, "not_found", "The requested item was not found.");
  }
  snoozed.set(key, new Date(Date.now() + days * 86_400_000).toISOString());
  return new HttpResponse(null, { status: 204 });
});

/** `DELETE /api/v1/dashboard/items/{key}/snooze` (R13): ends the snooze; 204 also when there was none. */
export const wakeDashboardItem = route("wakeDashboardItem", ({ params }) => {
  snoozed.delete(String(params.key));
  return new HttpResponse(null, { status: 204 });
});
