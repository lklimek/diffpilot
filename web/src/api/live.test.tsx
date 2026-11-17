import { act, renderHook, waitFor } from "@testing-library/react";
import { http, HttpResponse } from "msw";
import { setupServer } from "msw/node";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeEventSource } from "../__tests__/fake-event-source";
import { seedFor } from "../mocks/fixtures";
import type { AnalysisState, AnalysisSummary } from "./generated/types";
import { ApiError } from "./client";
import { ANALYSIS_RECONNECT_MS, ANALYSIS_SILENCE_MS, useAnalysisLive } from "./queries";

const base = seedFor("default", "analysis") as AnalysisSummary;
const ID = base.id;
const at = (state: AnalysisState): AnalysisSummary => ({ ...base, state });

let getAnalysis: () => Response = () => HttpResponse.json(at({ state: "planning" }));
const server = setupServer(http.get(`/api/v1/analyses/${ID}`, () => getAnalysis()));
beforeAll(() => server.listen({ onUnhandledFrame: "error" }));
beforeEach(() => {
  FakeEventSource.reset();
  vi.stubGlobal("EventSource", FakeEventSource);
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});
afterAll(() => server.close());

describe("useAnalysisLive (sse-events.md)", () => {
  it("opens the contract's event route and applies the snapshot, then live events (last wins)", () => {
    const { result } = renderHook(() => useAnalysisLive(ID));
    const source = FakeEventSource.latest();
    expect(source.url).toBe(`/api/v1/analyses/${ID}/events`);
    expect(result.current.summary).toBeUndefined();

    act(() => {
      source.open();
      source.emit({ event: "analysis", summary: at({ state: "running", done: 1, total: 5 }) });
      source.emit({ event: "analysis", summary: at({ state: "running", done: 2, total: 5 }) });
    });
    expect(result.current.summary?.state).toEqual({ state: "running", done: 2, total: 5 });
    expect(result.current.reconnecting).toBe(false);
  });

  it("ignores question events, other analyses and malformed data", () => {
    const { result } = renderHook(() => useAnalysisLive(ID));
    const source = FakeEventSource.latest();
    act(() => {
      source.emit({ event: "analysis", summary: at({ state: "planning" }) });
      source.emit({ event: "analysis", summary: { ...at({ state: "cancelled" }), id: "other" } });
      source.emit({ event: "analysis", data: "{not json" });
      source.emit({
        event: "question",
        question: {
          id: "q",
          step_id: "f1.1",
          text: "?",
          state: "pending",
          answer: null,
          failure: null,
          created_at: base.created_at,
        },
      });
    });
    expect(result.current.summary?.state).toEqual({ state: "planning" });
  });

  it("keeps the last summary while the browser reconnects; the next snapshot replaces it", () => {
    const { result } = renderHook(() => useAnalysisLive(ID));
    const source = FakeEventSource.latest();
    act(() => source.emit({ event: "analysis", summary: at({ state: "running", done: 1, total: 5 }) }));
    act(() => source.drop());
    expect(result.current.reconnecting).toBe(true);
    expect(result.current.summary?.state).toEqual({ state: "running", done: 1, total: 5 });
    // The same EventSource retries by itself (no new instance); the snapshot after reconnecting wins.
    act(() => {
      source.open();
      source.emit({ event: "analysis", summary: at({ state: "running", done: 3, total: 5 }) });
    });
    expect(FakeEventSource.instances).toHaveLength(1);
    expect(result.current.reconnecting).toBe(false);
    expect(result.current.summary?.state).toEqual({ state: "running", done: 3, total: 5 });
  });

  it("reports the API error when the stream fails for good and the analysis cannot be read", async () => {
    getAnalysis = () =>
      HttpResponse.json({ code: "not_found", message: "This analysis does not exist." }, { status: 404 });
    const { result } = renderHook(() => useAnalysisLive(ID));
    act(() => FakeEventSource.latest().fail());
    await waitFor(() => expect(result.current.error).toBeInstanceOf(ApiError));
    expect(result.current.error).toMatchObject({
      code: "not_found",
      message: "This analysis does not exist.",
    });
    expect(FakeEventSource.instances).toHaveLength(1);
  });

  it("reads the analysis and opens a new stream later when the stream fails but the analysis is readable", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    getAnalysis = () => HttpResponse.json(at({ state: "paused", resumes_at: null, done: 0, total: null }));
    const { result } = renderHook(() => useAnalysisLive(ID));
    act(() => FakeEventSource.latest().fail());
    await waitFor(() =>
      expect(result.current.summary?.state).toEqual({
        state: "paused",
        resumes_at: null,
        done: 0,
        total: null,
      }),
    );
    expect(result.current.reconnecting).toBe(true);
    await act(() => vi.advanceTimersByTimeAsync(ANALYSIS_RECONNECT_MS));
    expect(FakeEventSource.instances).toHaveLength(2);
    expect(FakeEventSource.instances[0]!.readyState).toBe(FakeEventSource.CLOSED);
  });

  it("tracks the last signal (any event, heartbeats too) and the browser−server clock offset from heartbeats", () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    const t0 = Date.parse("2026-11-17T10:00:00Z");
    vi.setSystemTime(t0);
    const { result } = renderHook(() => useAnalysisLive(ID));
    const source = FakeEventSource.latest();
    act(() => source.emit({ event: "analysis", summary: at({ state: "planning" }) }));
    expect(result.current.lastSignalAt).toBe(t0);
    expect(result.current.clockOffset).toBeNull();
    vi.setSystemTime(t0 + 15_000);
    // The server's clock is 2 min behind the browser's.
    act(() => source.emit({ event: "heartbeat", at: new Date(t0 + 15_000 - 120_000).toISOString() }));
    expect(result.current.lastSignalAt).toBe(t0 + 15_000);
    expect(result.current.clockOffset).toBe(120_000);
    expect(result.current.summary?.state).toEqual({ state: "planning" });
  });

  it("ignores an unknown event type and a heartbeat without a valid time", () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(Date.parse("2026-11-17T10:00:00Z"));
    const { result } = renderHook(() => useAnalysisLive(ID));
    const source = FakeEventSource.latest();
    act(() => {
      source.emit({ event: "analysis", summary: at({ state: "planning" }) });
      source.emit({ event: "something-new", data: JSON.stringify({ event: "something-new", x: 1 }) });
      source.emit({ event: "heartbeat", data: JSON.stringify({ event: "heartbeat", at: "not a time" }) });
      source.emit({ event: "analysis", summary: at({ state: "running", done: 1, total: 2 }) });
    });
    expect(result.current.summary?.state).toEqual({ state: "running", done: 1, total: 2 });
    expect(result.current.clockOffset).toBeNull();
  });

  it("reopens a stream that stays silent for longer than ANALYSIS_SILENCE_MS; the new snapshot restores it", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "Date"] });
    const { result } = renderHook(() => useAnalysisLive(ID));
    const first = FakeEventSource.latest();
    act(() => first.emit({ event: "analysis", summary: at({ state: "planning" }) }));
    act(() => vi.advanceTimersByTime(ANALYSIS_SILENCE_MS - 1000));
    act(() => first.emit({ event: "heartbeat", at: new Date().toISOString() }));
    act(() => vi.advanceTimersByTime(ANALYSIS_SILENCE_MS - 1000));
    expect(FakeEventSource.instances).toHaveLength(1);
    act(() => vi.advanceTimersByTime(2000));
    expect(FakeEventSource.instances).toHaveLength(2);
    expect(first.readyState).toBe(FakeEventSource.CLOSED);
    expect(result.current.reconnecting).toBe(true);
    act(() =>
      FakeEventSource.latest().emit({
        event: "analysis",
        summary: at({ state: "running", done: 0, total: 1 }),
      }),
    );
    expect(result.current.reconnecting).toBe(false);
    expect(result.current.summary?.state).toEqual({ state: "running", done: 0, total: 1 });
  });

  it("closes the stream on unmount", () => {
    const { unmount } = renderHook(() => useAnalysisLive(ID));
    const source = FakeEventSource.latest();
    unmount();
    expect(source.readyState).toBe(FakeEventSource.CLOSED);
  });
});

describe("useAnalysisLive with another id", () => {
  it("does not show the previous analysis's state", () => {
    vi.stubGlobal("EventSource", FakeEventSource);
    const { result, rerender } = renderHook(({ id }) => useAnalysisLive(id), { initialProps: { id: ID } });
    act(() => FakeEventSource.latest().emit({ event: "analysis", summary: at({ state: "planning" }) }));
    rerender({ id: "other" });
    expect(result.current.summary).toBeUndefined();
    expect(FakeEventSource.instances[0]!.readyState).toBe(FakeEventSource.CLOSED);
  });
});

describe("useAnalysisLive never carries one analysis's state over to another", () => {
  function snapshotAThenSwitchToB() {
    vi.stubGlobal("EventSource", FakeEventSource);
    const hook = renderHook(({ id }) => useAnalysisLive(id), { initialProps: { id: ID } });
    act(() =>
      FakeEventSource.latest().emit({
        event: "analysis",
        summary: at({ state: "running", done: 2, total: 5 }),
      }),
    );
    hook.rerender({ id: "other" });
    return hook;
  }

  it("a dropped connection of B before its first snapshot shows no state of A", () => {
    const { result } = snapshotAThenSwitchToB();
    act(() => FakeEventSource.latest().drop());
    expect(result.current.reconnecting).toBe(true);
    expect(result.current.summary).toBeUndefined();
  });

  it("a failed fallback read of B before its first snapshot shows B's error and no state of A", async () => {
    server.use(
      http.get("/api/v1/analyses/other", () =>
        HttpResponse.json({ code: "not_found", message: "This analysis does not exist." }, { status: 404 }),
      ),
    );
    const { result } = snapshotAThenSwitchToB();
    act(() => FakeEventSource.latest().fail());
    await waitFor(() => expect(result.current.error).toBeInstanceOf(ApiError));
    expect(result.current.summary).toBeUndefined();
  });
});
