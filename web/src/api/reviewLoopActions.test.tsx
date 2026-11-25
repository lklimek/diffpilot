import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook, waitFor } from "@testing-library/react";
import { http, HttpResponse } from "msw";
import { setupServer } from "msw/node";
import type { ReactNode } from "react";
import { afterAll, afterEach, beforeAll, describe, expect, it } from "vitest";
import { listItems, resetReviewLoop } from "../mocks/review-loop";
import type { CommentItem, ItemActionRequest, PrRef } from "./generated/types";
import { useAssessAgain, useUndoDecision } from "./review-loop";

const PR: PrRef = { repo: { owner: "acme", name: "node-client" }, number: 482 };
const ITEMS = "/api/v1/prs/acme/node-client/482/comments/items";

let stored: CommentItem[] = [];
const undone: ItemActionRequest[] = [];
let assessStatus = 202;
const server = setupServer(
  http.get("/api/v1/me", () => HttpResponse.json({ user: null, csrf_token: "csrf" })),
  http.get(ITEMS, () => HttpResponse.json(stored)),
  http.post(`${ITEMS}/C14/undo`, async ({ request }) => {
    undone.push((await request.json()) as ItemActionRequest);
    return HttpResponse.json(stored.find((i) => i.no === 14));
  }),
  http.post(`${ITEMS}/C14/assess`, () =>
    assessStatus === 202
      ? new HttpResponse(null, { status: 202 })
      : HttpResponse.json(
          {
            code: "conflict",
            message: "An assessment of this pull request is queued or running; assess again once it ends.",
          },
          { status: 409 },
        ),
  ),
);
beforeAll(() => server.listen({ onUnhandledFrame: "error" }));
afterEach(() => {
  undone.length = 0;
  assessStatus = 202;
  resetReviewLoop();
});
afterAll(() => server.close());

function setup() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  return renderHook(() => ({ undo: useUndoDecision(PR), assess: useAssessAgain(PR) }), { wrapper });
}

/** C14 after the reader's decision `id`: its revision moved on from what the page showed. */
const decided = (id: string, revision: number): CommentItem[] => {
  const c14 = listItems().find((i) => i.no === 14)!;
  const decision = { ...(c14.decisions.at(-1) ?? ({} as CommentItem["decisions"][number])), id };
  return [{ ...c14, revision, decisions: [...c14.decisions, decision] }];
};

describe("Undo (RC12, F5)", () => {
  it("reads the item again and sends its current revision", async () => {
    stored = decided("d-new", 7);
    const { result } = setup();
    await act(() => result.current.undo.mutateAsync({ no: 14, decision: "d-new" }));
    expect(undone).toEqual([{ item_revision: 7 }]);
  });

  it("sends nothing when the item's latest decision is no longer the reader's", async () => {
    stored = decided("d-other", 8);
    const { result } = setup();
    act(() => result.current.undo.mutate({ no: 14, decision: "d-new" }));
    await waitFor(() => expect(result.current.undo.isError).toBe(true));
    expect(result.current.undo.error?.message).toBe("C14 changed since your decision. Read it again.");
    expect(undone).toEqual([]);
  });
});

describe("Assess again (RC6, F2)", () => {
  it("is refused in plain words while a pass of the PR runs", async () => {
    assessStatus = 409;
    const { result } = setup();
    act(() => result.current.assess.mutate(14));
    await waitFor(() => expect(result.current.assess.isError).toBe(true));
    expect(result.current.assess.error?.message).toMatch(/queued or running; assess again once it ends/);
  });
});
