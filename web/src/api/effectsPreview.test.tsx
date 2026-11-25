// RC16 previews while a card is open: transient failures are planned again with backoff; a refusal is the input's.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook } from "@testing-library/react";
import { http, HttpResponse } from "msw";
import { setupServer } from "msw/node";
import type { ReactNode } from "react";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { DecisionBatchRow, EffectsPreviewRequest, PrRef } from "./generated/types";
import { previewRetryDelay, transientFailure, useEffectsPreview } from "./review-loop";
import { ApiError } from "./client";

const PR: PrRef = { repo: { owner: "acme", name: "node-client" }, number: 482 };
const PREVIEW = "/api/v1/prs/acme/node-client/482/comments/effects-preview";
const ROW: DecisionBatchRow = {
  item_no: 22,
  kind: "fix_here",
  reason: null,
  target: null,
  override: null,
  resolve_ticked: true,
  reply_text: null,
  comment: null,
  item_revision: 1,
};

/** What RC16 answers, call by call (the last one repeats). */
let answers: (() => Response)[] = [];
let calls = 0;
const planned = () => HttpResponse.json([{ item_no: 22, effects: [] }]);
const unavailable = () =>
  HttpResponse.json({ code: "unavailable", message: "The service is unavailable." }, { status: 503 });
const refused = () =>
  HttpResponse.json(
    { code: "invalid_request", message: "C22: this decision needs a reason." },
    { status: 400 },
  );
const server = setupServer(
  http.get("/api/v1/me", () => HttpResponse.json({ user: null, csrf_token: "csrf" })),
  http.post(PREVIEW, async ({ request }) => {
    (await request.json()) as EffectsPreviewRequest;
    const answer = answers[Math.min(calls, answers.length - 1)]!;
    calls += 1;
    return answer();
  }),
);
beforeAll(() => server.listen({ onUnhandledFrame: "error" }));
beforeEach(() => {
  calls = 0;
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
});
afterEach(() => vi.useRealTimers());
afterAll(() => server.close());

function setup() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  return renderHook(() => useEffectsPreview(PR, [ROW]), { wrapper });
}

/** Lets every answer arrive and every timer due within `ms` run (fetch settles on real macrotasks). */
const pass = (ms = 0) =>
  act(async () => {
    const settle = async () => {
      for (let i = 0; i < 10; i++) {
        await new Promise((done) => setImmediate(done));
        await vi.advanceTimersByTimeAsync(0);
      }
    };
    await settle();
    await vi.advanceTimersByTimeAsync(ms);
    await settle();
  });

describe("RC16 preview failures (a failed plan never leaves valid input blocked)", () => {
  it("a 503 is planned again with backoff until the server answers", async () => {
    answers = [unavailable, unavailable, planned];
    const { result } = setup();
    await pass();
    expect(calls).toBe(1);
    expect(result.current.current).toBe(false);
    expect(result.current.failureReason).toBeInstanceOf(ApiError);
    await pass(previewRetryDelay(0));
    expect(calls).toBe(2);
    await pass(previewRetryDelay(1));
    expect(calls).toBe(3);
    expect(result.current.current).toBe(true);
    expect(result.current.data?.items).toEqual([{ item_no: 22, effects: [] }]);
  });

  it("no answer at all (a network failure) is planned again too", async () => {
    answers = [() => HttpResponse.error(), planned];
    const { result } = setup();
    await pass();
    expect(calls).toBe(1);
    await pass(previewRetryDelay(0));
    expect(calls).toBe(2);
    expect(result.current.current).toBe(true);
  });

  it("a 400 refusal is not repeated: it stays an error in the server's words", async () => {
    answers = [refused, planned];
    const { result } = setup();
    await pass();
    await pass(previewRetryDelay(5));
    expect(calls).toBe(1);
    expect(result.current.isError).toBe(true);
    expect(result.current.error?.message).toBe("C22: this decision needs a reason.");
  });

  it("Plan again plans at once, without waiting for the next try", async () => {
    answers = [unavailable, planned];
    const { result } = setup();
    await pass();
    expect(calls).toBe(1);
    await act(async () => void result.current.planAgain());
    await pass();
    expect(calls).toBe(2);
    expect(result.current.current).toBe(true);
  });

  it("backs off up to half a minute; only no answer, 5xx, 408 and 429 count as transient", () => {
    expect([0, 1, 2, 3, 10].map(previewRetryDelay)).toEqual([1000, 2000, 4000, 8000, 30_000]);
    expect(transientFailure(new TypeError("Failed to fetch"))).toBe(true);
    expect(
      [0, 500, 502, 503, 408, 429].map((s) => transientFailure(new ApiError(s, "internal", "x"))),
    ).toEqual([true, true, true, true, true, true]);
    expect(
      [400, 401, 403, 404, 409, 422].map((s) => transientFailure(new ApiError(s, "invalid_request", "x"))),
    ).toEqual([false, false, false, false, false, false]);
  });
});
