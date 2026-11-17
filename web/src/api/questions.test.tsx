import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook, waitFor } from "@testing-library/react";
import { http, HttpResponse } from "msw";
import { setupServer } from "msw/node";
import type { ReactNode } from "react";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeEventSource } from "../__tests__/fake-event-source";
import { seedFor } from "../mocks/fixtures";
import type { AnalysisSummary, AskQuestion, MeResponse } from "./generated/types";
import {
  ANALYSIS_RECONNECT_MS,
  mergeFetchedQuestions,
  useAskQuestion,
  useQuestionEvents,
  useQuestions,
} from "./queries";

const ID = (seedFor("default", "analysis") as AnalysisSummary).id;
const q = (id: string, state: AskQuestion["state"]): AskQuestion => ({
  id,
  step_id: "f1.5",
  text: "Twice?",
  state,
  answer: state === "answered" ? { answer: "No.", evidence: [] } : null,
  failure: null,
  created_at: "2026-11-17T10:00:00Z",
});

let listed: () => Promise<AskQuestion[]> = async () => [];
let asked: () => Promise<AskQuestion> = async () => q("new", "pending");
let lists = 0;
const server = setupServer(
  http.get("/api/v1/me", () => HttpResponse.json(seedFor("default", "me") as MeResponse)),
  http.get(`/api/v1/analyses/${ID}/questions`, async () => {
    lists += 1;
    return HttpResponse.json(await listed());
  }),
  http.post(`/api/v1/analyses/${ID}/steps/:step/questions`, async () =>
    HttpResponse.json(await asked(), { status: 202 }),
  ),
);
beforeAll(() => server.listen({ onUnhandledFrame: "error" }));
beforeEach(() => {
  lists = 0;
  FakeEventSource.reset();
  vi.stubGlobal("EventSource", FakeEventSource);
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});
afterAll(() => server.close());

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => (resolve = r));
  return { promise, resolve };
}

function setup() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  return renderHook(
    () => ({ list: useQuestions(ID), stream: useQuestionEvents(ID), ask: useAskQuestion(ID) }),
    { wrapper },
  );
}

const emit = (question: AskQuestion) =>
  act(() => FakeEventSource.latest().emit({ event: "question", question }));

describe("question cache", () => {
  it("merges by arrival: newer cached copies win, others take the fetched copy", () => {
    const merged = mergeFetchedQuestions(
      [q("a", "pending"), q("b", "answered")],
      [q("a", "answered"), q("b", "answering"), q("c", "pending")],
      (id) => id !== "b",
    );
    expect(merged.map((x) => [x.id, x.state])).toEqual([
      ["a", "answered"],
      ["b", "answered"],
      ["c", "pending"],
    ]);
  });

  it("a stream snapshot that arrives while the list is read wins over the list's older copy", async () => {
    const reply = deferred<AskQuestion[]>();
    listed = () => reply.promise;
    const { result } = setup();
    emit(q("a", "answered"));
    reply.resolve([q("a", "pending")]);
    await waitFor(() => expect(result.current.list.isSuccess).toBe(true));
    expect(result.current.list.data!.map((x) => x.state)).toEqual(["answered"]);
  });

  it("a slow POST response never replaces the snapshot the stream already delivered", async () => {
    listed = async () => [];
    const reply = deferred<AskQuestion>();
    asked = () => reply.promise;
    const { result } = setup();
    await waitFor(() => expect(result.current.list.isSuccess).toBe(true));
    const asking = act(() => result.current.ask.mutateAsync({ step: "f1.5", text: "Twice?" }));
    await waitFor(() => expect(FakeEventSource.instances.length).toBe(1));
    emit(q("new", "answered"));
    reply.resolve(q("new", "pending"));
    await asking;
    expect(result.current.list.data!.map((x) => [x.id, x.state])).toEqual([["new", "answered"]]);
  });

  it("a stream retry (answered → answering) after the answer is applied in arrival order", async () => {
    listed = async () => [q("a", "answered")];
    const { result } = setup();
    await waitFor(() => expect(result.current.list.isSuccess).toBe(true));
    emit(q("a", "answering"));
    expect(result.current.list.data!.map((x) => x.state)).toEqual(["answering"]);
  });
});

describe("question stream", () => {
  it("when the browser gives up: says so, reads the list again and reopens the stream", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    listed = async () => [];
    const { result } = setup();
    await waitFor(() => expect(result.current.list.isSuccess).toBe(true));
    const before = lists;
    act(() => FakeEventSource.latest().fail());
    expect(result.current.stream.lost).toBe(true);
    await waitFor(() => expect(lists).toBe(before + 1));
    await act(() => vi.advanceTimersByTimeAsync(ANALYSIS_RECONNECT_MS));
    expect(FakeEventSource.instances).toHaveLength(2);
    emit(q("a", "answered"));
    expect(result.current.stream.lost).toBe(false);
    expect(result.current.list.data!.map((x) => x.state)).toEqual(["answered"]);
  });

  it("stops reconnecting after unmount", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const { unmount } = setup();
    act(() => FakeEventSource.latest().fail());
    unmount();
    await act(() => vi.advanceTimersByTimeAsync(ANALYSIS_RECONNECT_MS * 2));
    expect(FakeEventSource.instances).toHaveLength(1);
  });
});

it("refreshes Tour usage from the existing Ask event stream", async () => {
  const client = new QueryClient();
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  const tour = seedFor("default", "tour");
  client.setQueryData(["tour", ID], tour);
  renderHook(() => useQuestionEvents(ID), { wrapper });
  const usage = {
    input_tokens: 100,
    cached_input_tokens: 40,
    output_tokens: 12,
    reasoning_output_tokens: null,
    jobs: 1,
  };
  act(() =>
    FakeEventSource.latest().emit({
      event: "analysis",
      summary: { ...(seedFor("default", "analysis") as AnalysisSummary), usage },
    }),
  );
  expect(client.getQueryData<{ analysis: { usage: unknown } }>(["tour", ID])?.analysis.usage).toEqual(usage);
});
