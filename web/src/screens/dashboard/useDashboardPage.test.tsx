import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook, waitFor } from "@testing-library/react";
import { http, HttpResponse } from "msw";
import { setupServer } from "msw/node";
import type { ReactNode } from "react";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeEventSource } from "../../__tests__/fake-event-source";
import type { RefreshStatus } from "../../api/generated/types";
import { handlers } from "../../mocks/handlers";
import { resetDashboard, setDashboardScenario } from "../../mocks/handlers/dashboard";
import { resetDashboardSettings } from "../../mocks/handlers/dashboard-settings";
import { setScenario } from "../../mocks/scenarios";
import { resetSentTimeZones } from "./queries";
import { FILTER_KEY, useDashboardPage } from "./useDashboardPage";

const server = setupServer(...handlers);
beforeAll(() => server.listen({ onUnhandledFrame: "error" }));
beforeEach(() => {
  FakeEventSource.reset();
  vi.stubGlobal("EventSource", FakeEventSource);
  localStorage.clear();
});
afterEach(() => {
  vi.unstubAllGlobals();
  server.resetHandlers();
  setDashboardScenario("normal");
  setScenario(null);
  resetDashboard();
  resetDashboardSettings();
  resetSentTimeZones();
});
afterAll(() => server.close());

function renderPage() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  return renderHook(() => useDashboardPage(), { wrapper });
}

const emit = (status: RefreshStatus) =>
  act(() =>
    FakeEventSource.latest().emit({ event: "refresh", data: JSON.stringify({ event: "refresh", status }) }),
  );

describe("useDashboardPage", () => {
  it("loads the summary and all items; the list opens with pull requests (and TODOs)", async () => {
    const { result } = renderPage();
    await waitFor(() => expect(result.current.allItems).toHaveLength(14));
    expect(result.current.view).toMatchObject({
      refreshButton: { state: "ready" },
      ranking: null,
      empty: false,
    });
    expect(result.current.filter).toBe("pr");
    expect(new Set(result.current.shownItems!.map((i) => i.kind))).toEqual(new Set(["pr", "todo"]));
    // A finished refresh has no stream.
    expect(FakeEventSource.instances).toHaveLength(0);
  });

  it("remembers the kind filter", async () => {
    const { result } = renderPage();
    await waitFor(() => expect(result.current.allItems).toBeDefined());
    act(() => result.current.setFilter("issue"));
    expect(new Set(result.current.shownItems!.map((i) => i.kind))).toEqual(new Set(["issue", "todo"]));
    expect(localStorage.getItem(FILTER_KEY)).toBe("issue");
    const again = renderPage();
    expect(again.result.current.filter).toBe("issue");
  });

  it("reads nothing while signed out", async () => {
    setScenario("signed-out");
    const { result } = renderPage();
    await waitFor(() => expect(result.current.me.isSuccess).toBe(true));
    expect(result.current.user).toBeNull();
    expect(result.current.summary.fetchStatus).toBe("idle");
  });

  it("Refresh shows the scan and follows it over SSE", async () => {
    const { result } = renderPage();
    await waitFor(() => expect(result.current.view).toBeDefined());
    act(() => result.current.refresh.start());
    await waitFor(() => expect(result.current.view?.scanning).not.toBeNull());
    expect(result.current.view?.refreshButton).toEqual({ state: "scanning" });
    expect(result.current.view?.stale).toBe(true);
    const id = result.current.view!.scanning!.id;
    expect(FakeEventSource.latest().url).toBe(`/api/v1/dashboard/refreshes/${id}/events`);
    act(() => result.current.cancel.run(id));
    await waitFor(() => expect(result.current.view?.scanning).toBeNull());
    expect(FakeEventSource.latest().readyState).toBe(FakeEventSource.CLOSED);
  });

  it("a refused second start says a scan runs", async () => {
    setDashboardScenario("refreshing");
    const { result } = renderPage();
    await waitFor(() => expect(result.current.view?.scanning).not.toBeNull());
    act(() => result.current.refresh.start());
    await waitFor(() =>
      expect(result.current.refresh.error).toBe("A refresh is already scanning; its progress is shown."),
    );
  });

  it("says Ranking finished when the watched ranking ends", async () => {
    setDashboardScenario("provisional");
    const { result } = renderPage();
    await waitFor(() => expect(result.current.view?.ranking).toMatchObject({ kind: "running" }));
    const summary = result.current.summary.data!;
    const latest = summary.latest_refresh!;
    expect(result.current.notice).toBeNull();
    const finished: RefreshStatus = {
      ...latest,
      state: "finished",
      ranking: { ...latest.ranking!, state: "finished", judged: 14 },
    };
    // The server ranked the snapshot; the stream tells.
    server.use(
      http.get("/api/v1/dashboard", () => HttpResponse.json({ ...summary, latest_refresh: finished })),
    );
    emit(finished);
    await waitFor(() => expect(result.current.notice).toBe("Ranking finished: 14 items judged."));
  });

  it("Rank again restarts a failed ranking", async () => {
    setDashboardScenario("ranking-failed");
    const { result } = renderPage();
    await waitFor(() =>
      expect(result.current.view?.ranking).toMatchObject({ kind: "stopped", canRankAgain: true }),
    );
    const ranking = result.current.view!.ranking!;
    act(() => result.current.rankAgain.run(ranking.kind === "stopped" ? ranking.refreshId : ""));
    await waitFor(() => expect(result.current.view?.ranking).toMatchObject({ kind: "running" }));
  });

  it("an empty snapshot and a first visit without a snapshot read no items", async () => {
    setDashboardScenario("empty");
    const empty = renderPage();
    await waitFor(() => expect(empty.result.current.view?.empty).toBe(true));
    setDashboardScenario("first-run");
    const first = renderPage();
    await waitFor(() => expect(first.result.current.view).toMatchObject({ snapshot: null }));
    expect(first.result.current.items.fetchStatus).toBe("idle");
  });
});
