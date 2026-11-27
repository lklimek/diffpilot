import { act, fireEvent, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router";
import userEvent from "@testing-library/user-event";
import { HttpResponse, http } from "msw";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeEventSource } from "../../__tests__/fake-event-source";
import { renderRoute, startMockServer } from "../../__tests__/screen";
import type { AnalysisSummary, AnchoredAskRequest, AskQuestion } from "../../api/generated/types";
import { seedFor, setFixtureState } from "../../mocks/fixtures";
import { setScenario } from "../../mocks/scenarios";
import { AskChatProvider, useAskChat } from "./askChat";

const server = startMockServer();
const ID = (seedFor("default", "analysis") as AnalysisSummary).id;

beforeEach(() => {
  FakeEventSource.reset();
  vi.stubGlobal("EventSource", FakeEventSource);
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  window.localStorage.clear();
});

const user = () => userEvent.setup();
const chat = () => screen.getByRole("dialog", { name: "Ask" });
const inspector = () => screen.getByRole("complementary", { name: "Inspector" });
const launcher = () => screen.getByRole("button", { name: /^Ask.*: open your private chat/ });

async function openTour(hash = "") {
  const router = renderRoute(`/pr/acme/node-client/482/analyses/${ID}${hash}`);
  await within(await screen.findByRole("navigation", { name: "Flows and steps" })).findByRole("button", {
    name: /^Flow 1:/,
  });
  return router;
}

// Records the bodies of new questions.
function captureAsk() {
  const bodies: AnchoredAskRequest[] = [];
  const record = ({ request }: { request: Request }) => {
    if (request.method === "POST" && /\/analyses\/[^/]+\/questions$/.test(request.url))
      void request
        .clone()
        .json()
        .then((body: AnchoredAskRequest) => bodies.push(body));
  };
  server.events.on("request:start", record);
  return { bodies, stop: () => server.events.removeListener("request:start", record) };
}

describe("Ask chat in the review (proposal AC1, AC4, AC6, AC8, AC11)", () => {
  it("is hidden until the first Ask, then stays open from Flows to Files with its subject following the view", async () => {
    const router = await openTour("#f1.5");
    expect(screen.queryByRole("dialog", { name: "Ask" })).toBeNull();
    expect(launcher()).toBeVisible();
    await user().click(within(screen.getByRole("main")).getByRole("button", { name: "Ask about this step" }));
    const panel = chat();
    await user().type(within(panel).getByRole("textbox", { name: "Your question" }), "draft");
    await act(() =>
      router.navigate(
        `/pr/acme/node-client/482/analyses/${ID}/files#lines?file=src/rpc/client.rs&side=head&line=100&end=102`,
      ),
    );
    await waitFor(() => expect(panel).toHaveTextContent(/About:\s*Step 1\.6 · client\.rs 100–102/));
    expect(chat()).toBe(panel);
    expect(within(panel).getByRole("textbox", { name: "Your question" })).toHaveValue("draft");
    // A reload keeps it open (remembered per browser).
    cleanup();
    await openTour("#f1.5");
    expect(chat()).toBeVisible();
  });

  it("a follow-up keeps its thread; the Inspector copy opens the thread in the chat", async () => {
    setFixtureState("resume");
    const seen = captureAsk();
    try {
      await openTour("#f1.5");
      const copy = await within(inspector()).findByRole("region", {
        name: /^Your questions about this step \(1\)/,
      });
      expect(within(inspector()).queryByRole("textbox", { name: /Ask/ })).toBeNull();
      await user().click(within(copy).getByRole("button", { name: /^Open in chat/ }));
      const thread = chat().querySelector<HTMLElement>("[data-thread]")!;
      expect(thread).toHaveTextContent("Does the node deduplicate broadcast_tx by transaction id?");
      await user().click(within(thread).getByRole("button", { name: "Follow up on Q1" }));
      await user().type(
        within(thread).getByRole("textbox", { name: /^Follow-up to Q1/ }),
        "And on a retry?{Enter}",
      );
      await waitFor(() => expect(seen.bodies).toHaveLength(1));
      expect(seen.bodies[0]).toMatchObject({
        text: "And on a retry?",
        parent_id: thread.dataset.thread,
        anchor: { kind: "step", step_id: "f1.5", lines: null },
      });
      expect(await within(thread).findByRole("list", { name: "Follow-ups" })).toHaveTextContent(
        "And on a retry?",
      );
    } finally {
      seen.stop();
    }
  });

  it("the launcher counts answers that arrived while the chat was hidden until they are in view", async () => {
    setScenario("ask-thread");
    await openTour("#f1.2");
    expect(launcher()).toHaveAccessibleName(
      /^Ask: open your private chat about this PR, an answer is being written$/,
    );
    const answered: AskQuestion = {
      id: "t4",
      mine: true,
      anchor: { kind: "step", step_id: "f1.2", lines: null },
      step_id: "f1.2",
      text: "Why 5 s?",
      state: "answered",
      answer: { answer: "A product decision.", evidence: [] },
      failure: null,
      created_at: "2026-11-16T10:15:00Z",
    };
    act(() =>
      FakeEventSource.latest(`/api/v1/analyses/${ID}/events`).emit({ event: "question", question: answered }),
    );
    await waitFor(() => expect(launcher()).toHaveAccessibleName(/^Ask 1 new answer: open your private chat/));
    await user().click(launcher());
    const last = () => chat().querySelector<HTMLElement>('[data-thread="t4"]')!;
    expect(within(last()).getByText("new", { exact: true })).toBeVisible();
    await user().click(within(chat()).getByRole("button", { name: "Hide the Ask chat" }));
    expect(launcher()).toHaveAccessibleName(/^Ask 1 new answer: open your private chat/);
    await user().click(launcher());
    const conversation = within(chat()).getByRole("region", { name: "Conversation, oldest first" });
    conversation.getBoundingClientRect = () => ({ top: 0, bottom: 200 }) as DOMRect;
    const outcome = last().querySelector<HTMLElement>("[data-ask-outcome]")!;
    outcome.getBoundingClientRect = () => ({ top: -100, bottom: -40, height: 60 }) as DOMRect;
    fireEvent.scroll(conversation);
    const jump = await within(chat()).findByRole("button", { name: "↓ New answer" });
    outcome.scrollIntoView = vi.fn(() => {
      outcome.getBoundingClientRect = () => ({ top: 20, bottom: 80, height: 60 }) as DOMRect;
    });
    await user().click(jump);
    expect(outcome.scrollIntoView).toHaveBeenCalledOnce();
    await waitFor(() => expect(jump).not.toBeInTheDocument());
    await user().click(within(chat()).getByRole("button", { name: "Hide the Ask chat" }));
    expect(launcher()).toHaveAccessibleName(/^Ask: open your private chat about this PR$/);
  });

  it("announces a lost answer stream under the header and keeps the draft", async () => {
    await openTour("#f1.2");
    await user().keyboard("a");
    const field = within(chat()).getByRole("textbox", { name: "Your question" });
    await user().type(field, "Draft");
    act(() => FakeEventSource.latest(`/api/v1/analyses/${ID}/events`).fail());
    expect(within(chat()).getByRole("status")).toHaveTextContent(
      "The connection that delivers answers was lost.",
    );
    expect(field).toHaveValue("Draft");
  });

  it("a flow introduction is the subject once the flow is analysed; before that, the whole PR", async () => {
    setScenario("lazy-flows");
    await openTour("#f2");
    await user().keyboard("a");
    expect(chat()).toHaveTextContent(/About:\s*Whole PR/);
    cleanup();
    setScenario(null);
    await openTour("#f2");
    await user().click(within(screen.getByRole("main")).getByRole("button", { name: "Ask about this flow" }));
    expect(chat()).toHaveTextContent(/About:\s*Flow 2/);
  });

  it("Ask again on a failed first question asks it again as a first question, which replaces the failed one", async () => {
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
    const later: AskQuestion = {
      ...failed,
      id: "later",
      text: "What happened later?",
      state: "answered",
      answer: { answer: "Another answer.", evidence: [] },
      failure: null,
      created_at: "2026-11-16T10:02:00Z",
    };
    let listed: AskQuestion[] = [failed, later];
    server.use(http.get("/api/v1/analyses/:id/questions", () => HttpResponse.json(listed)));
    const seen = captureAsk();
    try {
      await openTour("#f1.5");
      await user().keyboard("a");
      const thread = await waitFor(() => {
        const t = chat().querySelector<HTMLElement>('[data-thread="f1"]');
        expect(t).not.toBeNull();
        return t!;
      });
      expect(within(thread).getByRole("button", { name: "Ask again" })).toHaveAccessibleDescription(
        /its new answer takes the place of this one/,
      );
      expect(within(thread).getByRole("button", { name: "Hide Q1" })).toBeVisible();
      await user().click(within(thread).getByRole("button", { name: "Ask again" }));
      await waitFor(() => expect(seen.bodies).toHaveLength(1));
      expect(seen.bodies[0]).toEqual({ text: "Does it retry twice?", anchor: failed.anchor });
      listed = [
        failed,
        later,
        { ...failed, id: "f2", state: "pending", failure: null, created_at: "2026-11-16T10:05:00Z" },
      ];
      await waitFor(() => expect(chat().querySelector('[data-thread="f1"]')).toBeNull());
      const checkNumbers = () => {
        const [first, second] = chat().querySelectorAll<HTMLElement>("[data-thread]");
        expect(first).toHaveTextContent(failed.text);
        expect(within(first!).getByRole("button", { name: "Hide Q1" })).toBeVisible();
        expect(second).toHaveTextContent(later.text);
        expect(within(second!).getByRole("button", { name: "Hide Q2" })).toBeVisible();
        expect(chat().querySelectorAll("[data-thread]")).toHaveLength(2);
      };
      checkNumbers();
      cleanup();
      await openTour("#f1.5");
      await waitFor(checkNumbers);
    } finally {
      seen.stop();
    }
  });

  it("a send failure keeps the question and says why", async () => {
    server.use(
      http.post("/api/v1/analyses/:id/questions", () =>
        HttpResponse.json({ code: "conflict", message: "You have 50 questions." }, { status: 409 }),
      ),
    );
    await openTour("#f1.5");
    await user().keyboard("a");
    await user().type(within(chat()).getByRole("textbox", { name: "Your question" }), "Why?{Enter}");
    expect(await within(chat()).findByRole("alert")).toHaveTextContent(/Not sent: You have 50 questions\.$/);
    expect(within(chat()).getByRole("textbox", { name: "Your question" })).toHaveValue("Why?");
  });

  it("an idle chat does not re-render the screens: no clock runs and the context stays the same", async () => {
    let renders = 0;
    function Probe() {
      useAskChat();
      renders += 1;
      return null;
    }
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <MemoryRouter>
          <AskChatProvider analysisId={ID}>
            <Probe />
          </AskChatProvider>
        </MemoryRouter>
      </QueryClientProvider>,
    );
    await screen.findByRole("button", { name: /^Ask.*: open your private chat/ });
    await act(() => new Promise((resolve) => setTimeout(resolve, 100)));
    const before = renders;
    try {
      await act(() => vi.advanceTimersByTimeAsync(30_000));
    } finally {
      vi.useRealTimers();
    }
    expect(renders).toBe(before);
  });
});
