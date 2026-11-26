import { useMutationState } from "@tanstack/react-query";
// Data of the dashboard screen (plan §E, docs/contracts/http-api.md "Dashboard", sse-events.md): the summary (R1),
// every item of its revision (R1b, cursor paging, ETag), live refresh events (R5), Refresh (R2), Cancel (R3),
// re-judgement (v1.4 C2) and the browser's time zone on the first visit (R6b).
import { type QueryClient, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { ApiError, api } from "../../api/client";
import type { DashboardSummary, PrRef, RefreshId, RefreshStatus, WorkItem } from "../../api/generated/types";

/** Query key of the summary (R1). */
export const SUMMARY_KEY = ["dashboard"] as const;
/** Query key of one refresh's status (R2b). */
export const refreshKey = (id: RefreshId | null) => ["dashboard-refresh", id] as const;
/** Query key of the items of one revision (R1b). */
export const itemsKey = (revision: string | null) => ["dashboard-items", revision] as const;

/** Page size of the item list: the server's maximum, so a list usually is one request. */
export const ITEM_PAGE_LIMIT = 500;
/** Restarts of the paging when the revision changes under it (409 `cursor_expired`). */
const CURSOR_RESTARTS = 2;
/** Delay before a new event stream when the browser gave up on the previous one. */
export const REFRESH_RECONNECT_MS = 5000;
/** A stream without any event (heartbeats come every 15 s) for this long is reopened. */
export const REFRESH_SILENCE_MS = 45_000;

const isLive = (refresh: RefreshStatus) => refresh.state === "scanning" || refresh.state === "ranking";

/** Order of a refresh's progress: scan, ranking, end; then scan percent, items judged, log lines. */
const progress = (status: RefreshStatus) => [
  status.state === "scanning" ? 0 : status.state === "ranking" ? 1 : 2,
  status.percent,
  status.ranking?.judged ?? 0,
  status.judged + status.reused,
  status.log.length,
];

/**
 * Whether `next` is older than `kept`, a status of the same live refresh: the wire carries no sequence, so the
 * progress orders them (a read started before a stream event can land after it). After an ended refresh any status
 * can be newer if ranking restarted.
 */
export function isOlderStatus(next: RefreshStatus, kept: RefreshStatus | null | undefined): boolean {
  if (!kept || next.id !== kept.id || !isLive(kept)) return false;
  const [a, b] = [progress(next), progress(kept)];
  const at = a.findIndex((value, i) => value !== b[i]);
  return at >= 0 && a[at]! < b[at]!;
}

/**
 * How often the summary is read again while the tab is visible: it picks up changes the server applied from GitHub
 * events and keeps plug-in statements current (ADR-022). Reading it starts no scan.
 */
export const SUMMARY_REFETCH_MS = 60_000;

/** The quicker re-read while a listed plug-in is not current (the server polls it once a reader is back). */
export const PLUGIN_RETRY_MS = 5_000;
/** Quick re-reads in a row before falling back to every minute (a source that stays down). */
export const PLUGIN_RETRY_READS = 6;

/** A listed plug-in's statements are not applied now (not read yet, or stale). */
const pluginNotCurrent = (summary: DashboardSummary | undefined): boolean =>
  summary?.plugins?.some((plugin) => plugin.state !== "current") ?? false;

/**
 * The summary's re-read interval: while a listed plug-in is not current, every `PLUGIN_RETRY_MS` for the first
 * `PLUGIN_RETRY_READS` such reads in a row (`notCurrentReads`); otherwise every minute.
 */
export const summaryRefetchInterval = (summary: DashboardSummary | undefined, notCurrentReads = 0): number =>
  pluginNotCurrent(summary) && notCurrentReads <= PLUGIN_RETRY_READS ? PLUGIN_RETRY_MS : SUMMARY_REFETCH_MS;

/**
 * The user's dashboard summary; disabled while signed out. A latest refresh older than the shown one is kept. It is
 * read again every minute in visible tabs to pick up snapshot and plug-in changes. While a plug-in is not current,
 * the summary is read sooner so a returning reader sees its statements within seconds (`summaryRefetchInterval`).
 */
export function useDashboardSummary(enabled: boolean) {
  const client = useQueryClient();
  const [visible, setVisible] = useState(() => document.visibilityState === "visible");
  useEffect(() => {
    const changed = () => setVisible(document.visibilityState === "visible");
    document.addEventListener("visibilitychange", changed);
    return () => document.removeEventListener("visibilitychange", changed);
  }, []);
  const notCurrentReads = useRef(0);
  return useQuery({
    queryKey: SUMMARY_KEY,
    queryFn: async () => {
      const summary = await api.getDashboard();
      notCurrentReads.current = pluginNotCurrent(summary) ? notCurrentReads.current + 1 : 0;
      const kept = client.getQueryData<DashboardSummary>(SUMMARY_KEY)?.latest_refresh;
      return summary.latest_refresh && isOlderStatus(summary.latest_refresh, kept)
        ? { ...summary, latest_refresh: kept! }
        : summary;
    },
    enabled: enabled && visible,
    refetchInterval: (query) => summaryRefetchInterval(query.state.data, notCurrentReads.current),
  });
}

/**
 * The refresh holding the shown snapshot when a newer refresh exists; provides the scan's counts.
 */
export function useSnapshotRefresh(summary: DashboardSummary | undefined) {
  const client = useQueryClient();
  const snapshot = summary?.snapshot;
  const id = snapshot && summary.latest_refresh?.id !== snapshot.refresh_id ? snapshot.refresh_id : null;
  return useQuery({
    queryKey: refreshKey(id),
    queryFn: async () => {
      const status = await api.getDashboardRefresh(id!);
      const kept = client.getQueryData<RefreshStatus>(refreshKey(id));
      return isOlderStatus(status, kept) ? kept! : status;
    },
    enabled: id !== null,
  });
}

/** Every item of one revision with the first page's ETag. */
export interface LoadedItems {
  revision: string;
  items: WorkItem[];
  etag: string | null;
}

/**
 * Reads every page of the ranked list. `previous` (the same query's last answer) is sent as `If-None-Match`: a
 * 304 keeps it. Review-loop counts can change within a revision. A cursor of an
 * older revision (409 `cursor_expired`) restarts the paging.
 */
export async function loadAllItems(previous: LoadedItems | undefined): Promise<LoadedItems> {
  for (let attempt = 0; ; attempt += 1) {
    try {
      const first = await api.listDashboardItems({ limit: ITEM_PAGE_LIMIT }, previous?.etag ?? null);
      if (first.unchanged) {
        if (previous) return previous;
        throw new ApiError(304, "internal", "The server gave an unexpected answer. Retry later.");
      }
      const items = [...first.body.items];
      for (let cursor = first.body.next_cursor; cursor !== null;) {
        const page = await api.listDashboardItems({ limit: ITEM_PAGE_LIMIT, cursor });
        if (page.unchanged) throw new ApiError(304, "internal", "The server gave an unexpected answer.");
        items.push(...page.body.items);
        cursor = page.body.next_cursor;
      }
      return { revision: first.body.revision, items, etag: first.etag };
    } catch (error) {
      if (error instanceof ApiError && error.code === "cursor_expired" && attempt < CURSOR_RESTARTS) continue;
      throw error;
    }
  }
}

/** The items of `revision` (none to read without a snapshot); the previous list stays shown while a new loads. */
export function useDashboardItems(revision: string | undefined, hasSnapshot: boolean) {
  const client = useQueryClient();
  return useQuery({
    queryKey: itemsKey(revision ?? null),
    queryFn: ({ queryKey }) => loadAllItems(client.getQueryData<LoadedItems>(queryKey)),
    enabled: revision !== undefined && hasSnapshot,
    placeholderData: (previous) => previous,
    refetchInterval: (query) =>
      query.state.data?.items.some(({ judgement }) => judgement?.state === "judging")
        ? 5_000
        : SUMMARY_REFETCH_MS,
  });
}

/** Changes of a refresh that can change the stored snapshot or its ranking, so the summary's revision. */
const revisionMayChange = (before: RefreshStatus, after: RefreshStatus) =>
  before.state !== after.state ||
  before.scanned_at !== after.scanned_at ||
  before.ranking?.state !== after.ranking?.state ||
  before.ranking?.judged !== after.ranking?.judged;

/**
 * Applies a refresh status from the stream or a mutation to the cached summary and refresh unless it is older than
 * the cached one; the summary is read again when the change can move its revision (progress alone does not).
 */
export function applyRefreshStatus(client: QueryClient, status: RefreshStatus): void {
  let reread = false;
  client.setQueryData<DashboardSummary>(SUMMARY_KEY, (summary) => {
    if (!summary || summary.latest_refresh?.id !== status.id) return summary;
    if (isOlderStatus(status, summary.latest_refresh)) return summary;
    reread = revisionMayChange(summary.latest_refresh, status);
    return { ...summary, latest_refresh: status };
  });
  const cached = client.getQueryData<RefreshStatus>(refreshKey(status.id));
  if (cached && !isOlderStatus(status, cached)) {
    reread ||= revisionMayChange(cached, status);
    client.setQueryData(refreshKey(status.id), status);
  }
  if (reread) void client.invalidateQueries({ queryKey: SUMMARY_KEY });
}

/**
 * Follows `refresh` over SSE while it is live: every `refresh` event updates the cache. The browser retries dropped
 * connections itself; when it gives up, or no event arrived for `REFRESH_SILENCE_MS`, the summary is read again
 * and a new stream opens after `REFRESH_RECONNECT_MS`. The stream closes when the refresh ends.
 */
export function useRefreshEvents(refresh: RefreshStatus | null | undefined): void {
  const client = useQueryClient();
  const id = refresh && isLive(refresh) ? refresh.id : null;
  useEffect(() => {
    if (id === null) return;
    let active = true;
    let close = () => {};
    let timer: ReturnType<typeof setTimeout> | undefined;
    let watchdog: ReturnType<typeof setTimeout> | undefined;
    const resync = () => {
      close();
      void Promise.all([
        client.invalidateQueries({ queryKey: SUMMARY_KEY }),
        client.invalidateQueries({ queryKey: refreshKey(id) }),
      ]).finally(() => {
        if (active) timer = setTimeout(connect, REFRESH_RECONNECT_MS);
      });
    };
    const watch = () => {
      clearTimeout(watchdog);
      watchdog = setTimeout(resync, REFRESH_SILENCE_MS);
    };
    function connect() {
      watch();
      close = api.subscribeDashboardRefresh(id!, {
        onEvent: (event) => {
          watch();
          if (event.event === "refresh" && event.status.id === id) applyRefreshStatus(client, event.status);
        },
        onError: (closed) => {
          if (closed) {
            clearTimeout(watchdog);
            resync();
          }
        },
      });
    }
    connect();
    return () => {
      active = false;
      clearTimeout(timer);
      clearTimeout(watchdog);
      close();
    };
  }, [client, id]);
}

/** Starts a refresh (R2); the summary is read again whatever the answer (409 `refresh_running` shows the scan). */
export function useStartRefresh() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (rejudgeOutdated: boolean) =>
      rejudgeOutdated ? api.startDashboardRefresh({ rejudge_outdated: true }) : api.startDashboardRefresh(),
    onSuccess: (status) =>
      client.setQueryData<DashboardSummary>(SUMMARY_KEY, (s) => s && { ...s, latest_refresh: status }),
    onSettled: () => client.invalidateQueries({ queryKey: SUMMARY_KEY }),
  });
}

/** Cancels a refresh (R3): a scan stops, a ranking ends; 409 `conflict` means it had ended already. */
export function useCancelRefresh() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: api.cancelDashboardRefresh,
    onSuccess: (status) => applyRefreshStatus(client, status),
    onSettled: () => client.invalidateQueries({ queryKey: SUMMARY_KEY }),
  });
}

/** Retry ranking the current snapshot via R4. */
export function useRetryRanking() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: api.rankDashboardRefresh,
    onSuccess: (status) => {
      applyRefreshStatus(client, status);
      client.setQueryData<RefreshStatus>(refreshKey(status.id), (kept) => kept ?? status);
    },
    onSettled: (_, error, id) =>
      Promise.all([
        client.invalidateQueries({ queryKey: SUMMARY_KEY }),
        error && client.invalidateQueries({ queryKey: refreshKey(id) }),
      ]),
  });
}

/** Requests a fresh LLM judgement; subsequent item pages carry the state and new judgement. */
export function useRejudgeItem() {
  const client = useQueryClient();
  const [error, setError] = useState<Error | null>(null);
  const pendingKeys = usePendingKeys("rejudge-item");
  const mutation = useMutation({
    mutationKey: ["rejudge-item"],
    mutationFn: ({ key }: { key: string }) => api.rejudgeDashboardItem(key),
    onError: setError,
    onSuccess: () => {
      setError(null);
      return Promise.all([
        client.invalidateQueries({ queryKey: SUMMARY_KEY }),
        client.invalidateQueries({ queryKey: ["dashboard-items"] }),
      ]);
    },
  });
  return { ...mutation, pendingKeys, error, dismissError: () => setError(null) };
}

/** The browser's IANA time zone, or null when it does not tell. */
export function browserTimeZone(): string | null {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || null;
  } catch {
    return null;
  }
}

/** Zones sent during this page load: a refused one is not sent again until the page reloads. */
const sentZones = new Set<string>();
/** Zones whose save failed during this page load. */
const failedZones = new Set<string>();

/** Forgets the zones sent (tests). */
export function resetSentTimeZones(): void {
  sentZones.clear();
  failedZones.clear();
}

/** The browser's zone cannot be saved in this page load: unknown to the browser, or refused or failed before. */
const cannotSave = () => {
  const zone = browserTimeZone();
  return zone === null || failedZones.has(zone);
};

/**
 * Sends the browser's time zone once while the summary has none (R6b, plan §A.5); then reads the summary again.
 * `failed`: the browser gives no zone, or the server refused (422) or could not be reached.
 */
export function useSaveBrowserTimeZone(summary: DashboardSummary | undefined): { failed: boolean } {
  const client = useQueryClient();
  const [failed, setFailed] = useState(cannotSave);
  const missing = summary !== undefined && summary.schedule.time_zone === null;
  useEffect(() => {
    if (!missing) return;
    const zone = browserTimeZone();
    if (zone === null || sentZones.has(zone)) return;
    sentZones.add(zone);
    api.setDashboardTimeZone({ time_zone: zone }).then(
      () => client.invalidateQueries({ queryKey: SUMMARY_KEY }),
      () => {
        failedZones.add(zone);
        setFailed(true);
      },
    );
  }, [client, missing]);
  return { failed };
}

/**
 * The user's Slack names (Settings › GitHub sync) for Copy for Slack (K19). A cache entry of its own: the Settings form
 * starts from its own read, never from this one.
 */
export const useSlackNames = (enabled: boolean) =>
  useQuery({
    queryKey: ["dashboard-slack-names"],
    queryFn: api.getDashboardSettings,
    select: (view) => view.settings.slack_names,
    enabled,
  });

/** Hide persists server-side; invalidation also refreshes the navigation count. */
export function useHideMention() {
  const client = useQueryClient();
  const [error, setError] = useState<Error | null>(null);
  const pendingIds = useMutationState({
    filters: { mutationKey: ["hide-mention"], status: "pending" },
    select: (m) => m.state.variables as string,
  });
  const mutation = useMutation({
    mutationKey: ["hide-mention"],
    mutationFn: async (id: string) => {
      try {
        await api.hideDashboardMention(id);
      } catch (error) {
        if (!(error instanceof ApiError && error.status === 404)) throw error;
      }
    },
    onError: setError,
    onSuccess: () => {
      setError(null);
      return client.invalidateQueries({ queryKey: SUMMARY_KEY });
    },
  });
  return { ...mutation, pendingIds, error, isError: error !== null, dismissError: () => setError(null) };
}

/** Pending item keys of a mutation keyed `key` (its variables start with the item key). */
function usePendingKeys(key: string): string[] {
  return useMutationState({
    filters: { mutationKey: [key], status: "pending" },
    select: (m) => (m.state.variables as { key: string }).key,
  });
}

/** Update Watch from a dashboard row, then read its comment counts and saved Watch state. */
export function useWatchItem() {
  const client = useQueryClient();
  const [failure, setFailure] = useState<{ key: string; error: Error; watching: boolean } | null>(null);
  const pendingKeys = usePendingKeys("watch-item");
  const mutation = useMutation({
    mutationKey: ["watch-item"],
    mutationFn: async ({ pr, watching }: { key: string; pr: PrRef; watching: boolean }) => {
      if (watching) await api.watchPr(pr);
      else await api.unwatchPr(pr);
    },
    onError: (error, { key, watching }) => setFailure({ key, error, watching }),
    onSuccess: (_, { key }) => {
      setFailure((current) => (current?.key === key ? null : current));
      return Promise.all([
        client.invalidateQueries({ queryKey: ["dashboard-items"] }),
        client.invalidateQueries({ queryKey: ["watches"] }),
        client.invalidateQueries({ queryKey: ["pr-history"] }),
      ]);
    },
  });
  return { ...mutation, pendingKeys, failure, dismissError: () => setFailure(null) };
}

/** Snooze of a ranked item (R12, owner SN1): persists server-side; the summary is read again. */
export function useSnoozeItem() {
  const client = useQueryClient();
  const [error, setError] = useState<Error | null>(null);
  const pendingKeys = usePendingKeys("snooze-item");
  const mutation = useMutation({
    mutationKey: ["snooze-item"],
    mutationFn: ({ key, days }: { key: string; days: number }) => api.snoozeDashboardItem(key, days),
    onError: setError,
    onSuccess: () => {
      setError(null);
      return client.invalidateQueries({ queryKey: SUMMARY_KEY });
    },
  });
  return { ...mutation, pendingKeys, error, isError: error !== null, dismissError: () => setError(null) };
}

/** "Wake now" of a snoozed item (R13): ends the snooze; the summary is read again. */
export function useWakeItem() {
  const client = useQueryClient();
  const [error, setError] = useState<Error | null>(null);
  const pendingKeys = usePendingKeys("wake-item");
  const mutation = useMutation({
    mutationKey: ["wake-item"],
    mutationFn: ({ key }: { key: string }) => api.wakeDashboardItem(key),
    onError: setError,
    onSuccess: () => {
      setError(null);
      return client.invalidateQueries({ queryKey: SUMMARY_KEY });
    },
  });
  return { ...mutation, pendingKeys, error, isError: error !== null, dismissError: () => setError(null) };
}
