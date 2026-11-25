// Independent QA checks of the Ask chat fix round 1 (feat/ask-chat-web, 8fc1c18).
import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { HttpResponse, http } from "msw";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeEventSource } from "../../__tests__/fake-event-source";
import { renderRoute, startMockServer } from "../../__tests__/screen";
import type { AnalysisSummary, AskQuestion } from "../../api/generated/types";
import { seedFor } from "../../mocks/fixtures";
import { setScenario } from "../../mocks/scenarios";

const server = startMockServer();
const ID = (seedFor("default", "analysis") as AnalysisSummary).id;

beforeEach(() => {
  FakeEventSource.reset();
  vi.stubGlobal("EventSource", FakeEventSource);
});
afterEach(() => {
  vi.unstubAllGlobals();
  window.localStorage.clear();
});

const user = () => userEvent.setup();
const chat = () => screen.getByRole("dialog", { name: "Ask" });
const inspector = () => screen.getByRole("complementary", { name: "Inspector" });
const thread = (id: string) =>
  waitFor(() => {
    const found = chat().querySelector<HTMLElement>(`[data-thread="${id}"]`);
    expect(found).not.toBeNull();
    return found!;
  });

async function openTour(hash = "") {
  const router = renderRoute(`/analyses/${ID}${hash}`);
  await within(await screen.findByRole("navigation", { name: "Flows and steps" })).findByRole("button", {
    name: /^Flow 1:/,
  });
  return router;
}

describe("Ask chat fix round 1 (Marvin verification)", () => {
  it("a failed question asked again counts once everywhere: chat, Inspector copy and the Flows entry", async () => {
    const failed: AskQuestion = {
      id: "f1",
      mine: true,
      anchor: { kind: "step", step_id: "f1.5", lines: null },
      step_id: "f1.5",
      text: "Does it retry twice?",
      state: "failed",
      answer: null,
      failure: "the LLM did not finish.",
      created_at: "2026-11-16T10:00:00Z",
    };
    const retry: AskQuestion = {
      ...failed,
      id: "f2",
      state: "pending",
      failure: null,
      created_at: "2026-11-16T10:05:00Z",
    };
    server.use(http.get("/api/v1/analyses/:id/questions", () => HttpResponse.json([failed, retry])));
    await openTour("#f1.5");
    await user().keyboard("a");
    await thread("f2");
    expect(chat().querySelectorAll("[data-thread]")).toHaveLength(1);
    // The Inspector copy is "a copy from your Ask chat": the same single thread.
    expect(
      await within(inspector()).findByRole("region", { name: /^Your questions about this step \(1\)/ }),
    ).toBeVisible();
    expect(screen.getByRole("button", { name: /^Your questions and answers: / })).toHaveAccessibleName(
      "Your questions and answers: 1",
    );
  });

  it("a draft handed from Flows to Files is taken once: back and forward do not open it again", async () => {
    server.use(
      http.get("/api/v1/analyses/:id/questions", () =>
        HttpResponse.json([
          {
            id: "l1",
            mine: true,
            anchor: {
              kind: "lines",
              path: "src/config.rs",
              side: "head",
              range: { start_line: 3, end_line: 4 },
            },
            text: "Why here?",
            state: "answered",
            answer: { answer: "Because.", evidence: [] },
            failure: null,
            created_at: "2026-11-16T10:00:00Z",
          },
        ]),
      ),
    );
    const router = await openTour("#f1.5");
    await user().keyboard("a");
    await user().click(within(await thread("l1")).getByRole("button", { name: "Use as review comment" }));
    const composer = await screen.findByRole("textbox", { name: /^Comment on src\/config\.rs:3–4/ });
    expect((composer as HTMLTextAreaElement).value).toContain("Because.");
    expect(router.state.location.state).toBeNull();
    await user().click(within(inspector()).getByRole("button", { name: "Close" }));
    await act(() => router.navigate(-1));
    await waitFor(() => expect(router.state.location.pathname).not.toMatch(/\/files$/));
    await act(() => router.navigate(1));
    await waitFor(() => expect(router.state.location.pathname).toMatch(/\/files$/));
    await screen.findByRole("complementary", { name: "Inspector" });
    await new Promise((resolve) => setTimeout(resolve, 200));
    expect(screen.queryByRole("textbox", { name: /^Comment on src\/config\.rs:3–4/ })).toBeNull();
  });

  it("a jump to another step does not say the draft is in the composer when its lines could not be read", async () => {
    setScenario("ask-thread");
    const router = await openTour("#f1.5");
    // Step 5's lines are read (and cached); every later read fails.
    await screen.findAllByText(/should_retry/);
    server.use(
      http.get("/api/v1/analyses/:id/lines", () =>
        HttpResponse.json(
          { code: "internal", message: "Something went wrong on the server. Try again later." },
          { status: 500 },
        ),
      ),
    );
    await user().keyboard("a");
    const card = within(await thread("t2"));
    await user().click(card.getByRole("button", { name: "Show Q2" }));
    await user().click(card.getByRole("button", { name: "Use as review comment" }));
    await waitFor(() => expect(router.state.location.hash).toBe("#f1.2"));
    await new Promise((resolve) => setTimeout(resolve, 200));
    const composer = screen.queryByRole("textbox", { name: /^Comment on / });
    const claim = screen.queryByText(/the draft is in the Comment composer/);
    // Either the draft is there, or nothing claims it is.
    expect(composer !== null || claim === null).toBe(true);
  });
});
