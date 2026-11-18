import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook, waitFor } from "@testing-library/react";
import { http, HttpResponse } from "msw";
import { setupServer } from "msw/node";
import type { ReactNode } from "react";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeEventSource } from "../../__tests__/fake-event-source";
import type { DashboardSummary, ItemPage, RefreshStatus, WorkItem } from "../../api/generated/types";
import { handlers } from "../../mocks/handlers";
import { resetDashboard, setDashboardScenario } from "../../mocks/handlers/dashboard";
import { resetDashboardSettings } from "../../mocks/handlers/dashboard-settings";
import itemsJson from "../../../../fixtures/dashboard/api/items.json";
import scanningJson from "../../../../fixtures/dashboard/api/refresh-scanning.json";
import {
  applyRefreshStatus,
  REFRESH_RECONNECT_MS,
  REFRESH_SILENCE_MS,
  SUMMARY_KEY,
  refreshKey,
  loadAllItems,
  resetSentTimeZones,
  useDashboardSummary,
  useRefreshEvents,
  useSaveBrowserTimeZone,
  useStartRefresh,
} from "./queries";

const ITEMS = (itemsJson as unknown as ItemPage).items;
const SCANNING = scanningJson as unknown as RefreshStatus;
const server = setupServer(...handlers);
beforeAll(() => server.listen({ onUnhandledFrame: "error" }));
beforeEach(() => {
  FakeEventSource.reset();
  vi.stubGlobal("EventSource", FakeEventSource);
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  server.resetHandlers();
  setDashboardScenario("normal");
  resetDashboard();
  resetDashboardSettings();
  resetSentTimeZones();
});
afterAll(() => server.close());

function wrapper() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return {
    client,
    wrapper: ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    ),
  };
}

const page = (items: WorkItem[], revision: string, next: string | null): ItemPage => ({
  revision,
  items,
  next_cursor: next,
});

describe("loadAllItems (R1b: cursor paging, ETag)", () => {
  it("follows next_cursor to the last page", async () => {
    const queries: string[] = [];
    server.use(
      http.get("/api/v1/dashboard/items", ({ request }) => {
        const url = new URL(request.url);
        queries.push(url.search);
        return url.searchParams.get("cursor") === "r1:2"
          ? HttpResponse.json(page(ITEMS.slice(2, 3), "r1", null))
          : HttpResponse.json(page(ITEMS.slice(0, 2), "r1", "r1:2"), { headers: { ETag: 'W/"a"' } });
      }),
    );
    const loaded = await loadAllItems(undefined);
    expect(loaded).toEqual({ revision: "r1", items: ITEMS.slice(0, 3), etag: 'W/"a"' });
    expect(queries).toEqual(["?limit=500", "?limit=500&cursor=r1%3A2"]);
  });

  it("starts again from the first page when the revision changed under the cursor", async () => {
    let calls = 0;
    server.use(
      http.get("/api/v1/dashboard/items", ({ request }) => {
        calls += 1;
        const cursor = new URL(request.url).searchParams.get("cursor");
        if (cursor === "r1:1") {
          return HttpResponse.json({ code: "cursor_expired", message: "The list changed." }, { status: 409 });
        }
        return calls === 1
          ? HttpResponse.json(page(ITEMS.slice(0, 1), "r1", "r1:1"))
          : HttpResponse.json(page(ITEMS.slice(0, 4), "r2", null));
      }),
    );
    expect(await loadAllItems(undefined)).toMatchObject({ revision: "r2", items: ITEMS.slice(0, 4) });
  });

  it("keeps the previous list on 304 for its ETag (the mock's weak ETag)", async () => {
    const first = await loadAllItems(undefined);
    expect(first.items).toHaveLength(ITEMS.length);
    expect(first.etag).toMatch(/^W\//);
    let status = 0;
    server.events.on("response:mocked", ({ response }) => {
      status = response.status;
    });
    expect(await loadAllItems(first)).toBe(first);
    expect(status).toBe(304);
    server.events.removeAllListeners();
  });
});

describe("useRefreshEvents (R5)", () => {
  const summaryWith = (latest: RefreshStatus | null): DashboardSummary =>
    ({ revision: "r", latest_refresh: latest, snapshot: null }) as DashboardSummary;

  it("opens the live refresh's stream, applies its events and closes when it ends", async () => {
    const { client, wrapper: w } = wrapper();
    client.setQueryData(SUMMARY_KEY, summaryWith(SCANNING));
    const invalidate = vi.spyOn(client, "invalidateQueries");
    const { rerender } = renderHook(({ refresh }) => useRefreshEvents(refresh), {
      wrapper: w,
      initialProps: { refresh: SCANNING as RefreshStatus | null },
    });
    const source = FakeEventSource.latest();
    expect(source.url).toBe(`/api/v1/dashboard/refreshes/${SCANNING.id}/events`);

    const progress = { ...SCANNING, percent: 60 };
    act(() =>
      source.emit({ event: "refresh", data: JSON.stringify({ event: "refresh", status: progress }) }),
    );
    expect(client.getQueryData<DashboardSummary>(SUMMARY_KEY)?.latest_refresh?.percent).toBe(60);
    // Progress alone keeps the revision: no re-read.
    expect(invalidate).not.toHaveBeenCalled();

    const other = { ...progress, id: "other", percent: 99 };
    act(() => source.emit({ event: "refresh", data: JSON.stringify({ event: "refresh", status: other }) }));
    expect(client.getQueryData<DashboardSummary>(SUMMARY_KEY)?.latest_refresh?.percent).toBe(60);

    const ranking = { ...progress, state: "ranking" as const, scanned_at: "2026-11-18T09:24:00Z" };
    act(() => source.emit({ event: "refresh", data: JSON.stringify({ event: "refresh", status: ranking }) }));
    expect(invalidate).toHaveBeenCalledWith({ queryKey: SUMMARY_KEY });

    rerender({ refresh: { ...ranking, state: "finished" } });
    expect(source.readyState).toBe(FakeEventSource.CLOSED);
    expect(FakeEventSource.instances).toHaveLength(1);
  });

  it("opens nothing for an ended refresh", () => {
    renderHook(() => useRefreshEvents({ ...SCANNING, state: "failed" }), { wrapper: wrapper().wrapper });
    expect(FakeEventSource.instances).toHaveLength(0);
  });

  it("re-reads the summary and reopens the stream after the browser gave up", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const { client, wrapper: w } = wrapper();
    const invalidate = vi.spyOn(client, "invalidateQueries");
    renderHook(() => useRefreshEvents(SCANNING), { wrapper: w });
    act(() => FakeEventSource.latest().fail());
    expect(invalidate).toHaveBeenCalledWith({ queryKey: SUMMARY_KEY });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(REFRESH_RECONNECT_MS);
    });
    expect(FakeEventSource.instances).toHaveLength(2);
  });
});

describe("useRefreshEvents silence watchdog", () => {
  it("reopens a stream that stays silent for REFRESH_SILENCE_MS; any event restarts the wait", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const { client, wrapper: w } = wrapper();
    const invalidate = vi.spyOn(client, "invalidateQueries");
    renderHook(() => useRefreshEvents(SCANNING), { wrapper: w });
    const first = FakeEventSource.latest();

    // A heartbeat before the deadline keeps the stream.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(REFRESH_SILENCE_MS - 1000);
    });
    act(() =>
      first.emit({
        event: "heartbeat",
        data: JSON.stringify({ event: "heartbeat", at: "2026-11-18T09:27:00Z" }),
      }),
    );
    await act(async () => {
      await vi.advanceTimersByTimeAsync(REFRESH_SILENCE_MS - 1000);
    });
    expect(invalidate).not.toHaveBeenCalled();
    expect(first.readyState).not.toBe(FakeEventSource.CLOSED);

    // Silence for the whole window: close, re-read summary and refresh, reopen after the delay.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1000);
    });
    expect(first.readyState).toBe(FakeEventSource.CLOSED);
    expect(invalidate).toHaveBeenCalledWith({ queryKey: SUMMARY_KEY });
    expect(invalidate).toHaveBeenCalledWith({ queryKey: refreshKey(SCANNING.id) });
    expect(FakeEventSource.instances).toHaveLength(1);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(REFRESH_RECONNECT_MS);
    });
    expect(FakeEventSource.instances).toHaveLength(2);
    expect(FakeEventSource.latest().url).toBe(first.url);
  });
});

describe("useStartRefresh (R2)", () => {
  it("shows the started scan at once", async () => {
    const { client, wrapper: w } = wrapper();
    const { result } = renderHook(() => ({ summary: useDashboardSummary(true), start: useStartRefresh() }), {
      wrapper: w,
    });
    await waitFor(() => expect(result.current.summary.data).toBeDefined());
    act(() => result.current.start.mutate());
    await waitFor(() => expect(result.current.start.isSuccess).toBe(true));
    expect(client.getQueryData<DashboardSummary>(SUMMARY_KEY)?.latest_refresh?.state).toBe("scanning");
  });

  it("409 refresh_running: the error is kept and the summary shows the running scan", async () => {
    setDashboardScenario("refreshing");
    const { client, wrapper: w } = wrapper();
    client.setQueryData(SUMMARY_KEY, {
      revision: "old",
      latest_refresh: null,
    } as unknown as DashboardSummary);
    const { result } = renderHook(() => ({ summary: useDashboardSummary(true), start: useStartRefresh() }), {
      wrapper: w,
    });
    act(() => result.current.start.mutate());
    await waitFor(() => expect(result.current.start.isError).toBe(true));
    expect(result.current.start.error).toMatchObject({ status: 409, code: "refresh_running" });
    await waitFor(() => expect(result.current.summary.data?.latest_refresh?.state).toBe("scanning"));
  });
});

describe("useSaveBrowserTimeZone (R6b)", () => {
  it("sends the browser's zone once while none is stored, then reads the summary again", async () => {
    setDashboardScenario("no-time-zone");
    const zone = Intl.DateTimeFormat().resolvedOptions().timeZone;
    const sent: unknown[] = [];
    server.events.on("request:start", async ({ request }) => {
      if (request.method === "PUT" && request.url.endsWith("/settings/dashboard/time-zone")) {
        sent.push(await request.clone().json());
      }
    });
    const useHook = () => {
      const summary = useDashboardSummary(true);
      useSaveBrowserTimeZone(summary.data);
      return summary;
    };
    const first = renderHook(useHook, { wrapper: wrapper().wrapper });
    await waitFor(() => expect(first.result.current.data?.schedule.time_zone).toBe(zone));
    expect(sent).toEqual([{ time_zone: zone }]);

    // Another visit in the same page load with the zone still missing sends nothing again.
    resetDashboard();
    const second = renderHook(useHook, { wrapper: wrapper().wrapper });
    await waitFor(() => expect(second.result.current.data?.schedule.time_zone).toBeNull());
    expect(sent).toHaveLength(1);
    server.events.removeAllListeners();
  });

  it("reports a refused zone (422), so the note can point to Settings", async () => {
    setDashboardScenario("no-time-zone-refused");
    const { result } = renderHook(
      () => {
        const summary = useDashboardSummary(true);
        return useSaveBrowserTimeZone(summary.data);
      },
      { wrapper: wrapper().wrapper },
    );
    expect(result.current).toEqual({ failed: false });
    await waitFor(() => expect(result.current).toEqual({ failed: true }));
    // A later visit in the same page load knows it failed without sending again.
    const again = renderHook(
      () => {
        const summary = useDashboardSummary(true);
        return useSaveBrowserTimeZone(summary.data);
      },
      { wrapper: wrapper().wrapper },
    );
    expect(again.result.current).toEqual({ failed: true });
  });

  it("sends nothing when a zone is stored", async () => {
    let puts = 0;
    server.events.on("request:start", ({ request }) => {
      if (request.method === "PUT") puts += 1;
    });
    const { result } = renderHook(
      () => {
        const summary = useDashboardSummary(true);
        useSaveBrowserTimeZone(summary.data);
        return summary;
      },
      { wrapper: wrapper().wrapper },
    );
    await waitFor(() => expect(result.current.data?.schedule.time_zone).toBe("Europe/Warsaw"));
    expect(puts).toBe(0);
    server.events.removeAllListeners();
  });
});

describe("refresh statuses are applied in order (never an older one over a newer)", () => {
  const summaryWith = (latest: RefreshStatus | null): DashboardSummary =>
    ({ revision: "r", latest_refresh: latest, snapshot: null }) as DashboardSummary;
  const RANKING_0: RefreshStatus = {
    ...SCANNING,
    state: "ranking",
    percent: 100,
    scanned_at: "2026-11-18T09:24:00Z",
    ranking: { state: "running", judged: 0, total: 18, resumes_at: null, message: null },
  };
  const FAILED_RANKING: RefreshStatus = {
    ...RANKING_0,
    state: "finished",
    ranking: { ...RANKING_0.ranking!, state: "failed", judged: 7, message: "The LLM stopped answering" },
  };
  const latest = (client: QueryClient) => client.getQueryData<DashboardSummary>(SUMMARY_KEY)?.latest_refresh;

  /** R1 answers with `body` once `release` is called, counting the requests. */
  function heldSummary() {
    let answer: (() => void) | null = null;
    let body: DashboardSummary = summaryWith(SCANNING);
    const requests = { count: 0 };
    server.use(
      http.get("/api/v1/dashboard", async () => {
        requests.count += 1;
        const sent = body;
        await new Promise<void>((resolve) => (answer = resolve));
        return HttpResponse.json(sent);
      }),
    );
    return {
      requests,
      answerWith: (next: DashboardSummary) => (body = next),
      release: () => answer?.(),
    };
  }

  it("a summary read before a stream event lands after it: the event's status stays", async () => {
    const { client, wrapper: w } = wrapper();
    client.setQueryData(SUMMARY_KEY, summaryWith({ ...SCANNING, percent: 40 }));
    const held = heldSummary();
    held.answerWith(summaryWith({ ...SCANNING, percent: 40 }));
    renderHook(() => useDashboardSummary(true), { wrapper: w });
    await act(() => client.invalidateQueries({ queryKey: SUMMARY_KEY, refetchType: "none" }));
    const reading = client.refetchQueries({ queryKey: SUMMARY_KEY });
    await waitFor(() => expect(held.requests.count).toBeGreaterThan(0));
    act(() => applyRefreshStatus(client, { ...SCANNING, percent: 60 }));
    held.release();
    await act(() => reading);
    expect(latest(client)?.percent).toBe(60);
  });

  it("the server still answering scanning after the first ranking event does not undo the ranking", async () => {
    const { client, wrapper: w } = wrapper();
    client.setQueryData(SUMMARY_KEY, summaryWith(SCANNING));
    const held = heldSummary();
    renderHook(() => useDashboardSummary(true), { wrapper: w });
    await waitFor(() => expect(held.requests.count).toBe(1));
    held.release();
    await waitFor(() => expect(client.isFetching({ queryKey: SUMMARY_KEY })).toBe(0));
    // The event moves to ranking and re-reads the summary; that answer still has the scan.
    held.answerWith(summaryWith({ ...SCANNING, percent: 100 }));
    act(() => applyRefreshStatus(client, RANKING_0));
    expect(latest(client)?.ranking?.judged).toBe(0);
    await waitFor(() => expect(held.requests.count).toBe(2));
    held.release();
    await waitFor(() => expect(client.isFetching({ queryKey: SUMMARY_KEY })).toBe(0));
    expect(latest(client)?.state).toBe("ranking");
    expect(latest(client)?.ranking?.judged).toBe(0);
    // A later event and a later answer move on.
    act(() => applyRefreshStatus(client, { ...RANKING_0, ranking: { ...RANKING_0.ranking!, judged: 3 } }));
    expect(latest(client)?.ranking?.judged).toBe(3);
  });

  it("an older stream status after a newer one is ignored", () => {
    const { client } = wrapper();
    client.setQueryData(SUMMARY_KEY, summaryWith(RANKING_0));
    applyRefreshStatus(client, { ...SCANNING, percent: 90 });
    expect(latest(client)?.state).toBe("ranking");
  });

  it("Rank again restarts an ended ranking of the same refresh", () => {
    const { client } = wrapper();
    client.setQueryData(SUMMARY_KEY, summaryWith(FAILED_RANKING));
    applyRefreshStatus(client, RANKING_0);
    expect(latest(client)?.state).toBe("ranking");
    expect(latest(client)?.ranking?.judged).toBe(0);
  });

  it("a ranking restarted elsewhere (another tab) is taken from the summary", async () => {
    const { client, wrapper: w } = wrapper();
    client.setQueryData(SUMMARY_KEY, summaryWith(FAILED_RANKING));
    server.use(http.get("/api/v1/dashboard", () => HttpResponse.json(summaryWith(RANKING_0))));
    renderHook(() => useDashboardSummary(true), { wrapper: w });
    await waitFor(() => expect(latest(client)?.state).toBe("ranking"));
  });
});
