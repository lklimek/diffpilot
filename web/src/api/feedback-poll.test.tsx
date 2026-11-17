import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook, waitFor } from "@testing-library/react";
import { http, HttpResponse } from "msw";
import { setupServer } from "msw/node";
import type { ReactNode } from "react";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import type { Feedback, FeedbackMemory } from "./generated/types";
import { FEEDBACK_POLL_MS, FEEDBACK_RETRY_POLL_MS, useFeedback } from "./queries";

const ID = "6d3a2f1e-0482-4c5e-9a7b-000000000482";
const entry = (memory: FeedbackMemory): Feedback => ({
  id: "fb1",
  target: { target: "step", step_id: "f1.5" },
  value: "overrated",
  note: null,
  created_at: "2026-11-17T08:00:00Z",
  memory,
});

let stored: Feedback[] = [];
let reads = 0;
const server = setupServer(
  http.get(`/api/v1/analyses/${ID}/feedback`, () => {
    reads += 1;
    return HttpResponse.json(stored);
  }),
);
beforeAll(() => server.listen({ onUnhandledFrame: "error" }));
afterEach(() => {
  vi.useRealTimers();
  reads = 0;
});
afterAll(() => server.close());

function render() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  return renderHook(() => useFeedback(ID), { wrapper });
}

describe("useFeedback polling of the MemCan write", () => {
  it("re-reads a failed write until the server reports it written, then stops", async () => {
    vi.useFakeTimers({
      toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval"],
      shouldAdvanceTime: true,
    });
    stored = [entry("failed")];
    const { result } = render();
    await waitFor(() => expect(result.current.data?.[0]?.memory).toBe("failed"));
    // A failed write is retried by the server on its schedule; the page reads again, slower than for pending.
    expect(FEEDBACK_RETRY_POLL_MS).toBeGreaterThan(FEEDBACK_POLL_MS);
    stored = [entry("written")];
    await act(() => vi.advanceTimersByTimeAsync(FEEDBACK_RETRY_POLL_MS));
    await waitFor(() => expect(result.current.data?.[0]?.memory).toBe("written"));
    const settled = reads;
    await act(() => vi.advanceTimersByTimeAsync(5 * FEEDBACK_RETRY_POLL_MS));
    expect(reads).toBe(settled);
  });

  it("does not poll when every entry is written", async () => {
    vi.useFakeTimers({
      toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval"],
      shouldAdvanceTime: true,
    });
    stored = [entry("written")];
    const { result } = render();
    await waitFor(() => expect(result.current.data).toHaveLength(1));
    const first = reads;
    await act(() => vi.advanceTimersByTimeAsync(5 * FEEDBACK_RETRY_POLL_MS));
    expect(reads).toBe(first);
  });

  it("re-reads a pending write at the faster interval", async () => {
    vi.useFakeTimers({
      toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval"],
      shouldAdvanceTime: true,
    });
    stored = [entry("pending")];
    const { result } = render();
    await waitFor(() => expect(result.current.data?.[0]?.memory).toBe("pending"));
    stored = [entry("written")];
    await act(() => vi.advanceTimersByTimeAsync(FEEDBACK_POLL_MS));
    await waitFor(() => expect(result.current.data?.[0]?.memory).toBe("written"));
  });
});
