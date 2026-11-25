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
import { FILTER_KEY, LEGACY_FILTER_KEY, useDashboardPage } from "./useDashboardPage";

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
  it("loads the summary and all items; the list opens with every kind on", async () => {
    const { result } = renderPage();
    await waitFor(() => expect(result.current.allItems).toHaveLength(13));
    expect(result.current.view).toMatchObject({ scanning: null, empty: false });
    expect(result.current.filter).toEqual(["pr", "issue", "todo"]);
    // A finished refresh has no stream.
    expect(FakeEventSource.instances).toHaveLength(0);
  });

  it("remembers the kind filter", async () => {
    const { result } = renderPage();
    await waitFor(() => expect(result.current.allItems).toBeDefined());
    act(() => result.current.setFilter(["pr", "issue"]));
    expect(result.current.filter).toEqual(["pr", "issue"]);
    const again = renderPage();
    expect(again.result.current.filter).toEqual(["pr", "issue"]);
  });

  it("maps a stored single choice of the earlier switch to the matching toggles (TODOs were listed with each)", () => {
    const stored = (value: string) => {
      localStorage.setItem(LEGACY_FILTER_KEY, value);
      return renderPage().result.current.filter;
    };
    expect(stored("pr")).toEqual(["pr", "todo"]);
    expect(stored("issue")).toEqual(["issue", "todo"]);
    expect(stored("both")).toEqual(["pr", "issue", "todo"]);
    expect(stored("nonsense")).toEqual(["pr", "issue", "todo"]);
    expect(stored("constructor")).toEqual(["pr", "issue", "todo"]);
    // A saved choice replaces the earlier value.
    const { result } = renderPage();
    act(() => result.current.setFilter(["pr"]));
    expect(localStorage.getItem(LEGACY_FILTER_KEY)).toBeNull();
    expect(JSON.parse(localStorage.getItem(FILTER_KEY) ?? "null")).toEqual(["pr"]);
  });

  it("remembers a single kind choice as chosen (PRs only, Issues only)", async () => {
    for (const only of [["pr"], ["issue"]] as const) {
      localStorage.clear();
      const { result } = renderPage();
      act(() => result.current.setFilter([...only]));
      expect(renderPage().result.current.filter).toEqual(only);
    }
  });

  it("a corrupt stored value naming an Object.prototype key falls back to every kind", () => {
    for (const value of ["constructor", "toString", "__proto__"]) {
      localStorage.setItem(FILTER_KEY, value);
      expect(renderPage().result.current.filter).toEqual(["pr", "issue", "todo"]);
    }
    localStorage.removeItem(FILTER_KEY);
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
    await waitFor(() => expect(result.current.view?.scanning).toBeTruthy());
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
    await waitFor(() => expect(result.current.view?.scanning).toBeTruthy());
    act(() => result.current.refresh.start());
    await waitFor(() =>
      expect(result.current.refresh.error).toBe("A refresh is already scanning; its progress is shown."),
    );
    // The message belongs to that scan: once it ended, it goes.
    act(() => result.current.cancel.run(result.current.view!.scanning!.id));
    await waitFor(() => expect(result.current.view?.scanning).toBeNull());
    expect(result.current.refresh.error).toBeNull();
  });

  it("says Ranking finished when the watched ranking ends", async () => {
    setDashboardScenario("provisional");
    const { result } = renderPage();
    await waitFor(() => expect(result.current.summary.data?.latest_refresh?.ranking?.state).toBe("running"));
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

  it("requests one re-judgement, then reads the running state from the item page", async () => {
    setDashboardScenario("v14");
    const { result } = renderPage();
    const key = "acme~node-client~pr~482";
    const state = () => result.current.allItems?.find((item) => item.key === key)?.judgement?.state;
    await waitFor(() => expect(state()).toBe("outdated"));
    act(() => result.current.rejudge.mutate({ key }));
    await waitFor(() => expect(state()).toBe("judging"));
    expect(result.current.rejudge.error).toBeNull();
  });

  it("keeps the outdated judgement when the request fails", async () => {
    setDashboardScenario("v14-rejudge-error");
    const { result } = renderPage();
    const key = "acme~node-client~pr~482";
    const state = () => result.current.allItems?.find((item) => item.key === key)?.judgement?.state;
    await waitFor(() => expect(state()).toBe("outdated"));
    act(() => result.current.rejudge.mutate({ key }));
    await waitFor(() => expect(result.current.rejudge.error?.message).toBe("LLM ranking is unavailable."));
    expect(state()).toBe("outdated");
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
