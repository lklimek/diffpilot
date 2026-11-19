import { act, cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { HttpResponse, http } from "msw";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeEventSource } from "../../__tests__/fake-event-source";
import { renderRoute, startMockServer } from "../../__tests__/screen";
import type { ChatAskRequest } from "../../api/askChat";
import type { AnalysisSummary, AskQuestion } from "../../api/generated/types";
import { seedFor, setFixtureState } from "../../mocks/fixtures";
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
const launcher = () => screen.getByRole("button", { name: /^Ask.*: open your private chat/ });

async function openTour(hash = "") {
  const router = renderRoute(`/analyses/${ID}${hash}`);
  await within(await screen.findByRole("navigation", { name: "Flows and steps" })).findByRole("button", {
    name: /^Flow 1:/,
  });
  return router;
}

// Records the bodies of new questions.
function captureAsk() {
  const bodies: ChatAskRequest[] = [];
  const record = ({ request }: { request: Request }) => {
    if (request.method === "POST" && /\/analyses\/[^/]+\/questions$/.test(request.url))
      void request
        .clone()
        .json()
        .then((body: ChatAskRequest) => bodies.push(body));
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
      router.navigate(`/analyses/${ID}/files#lines?file=src/rpc/client.rs&side=head&line=100&end=102`),
    );
    await waitFor(() =>
      expect(panel).toHaveTextContent(/About:\s*Files src\/rpc\/client\.rs:100–102 flow 1 · step 6/),
    );
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
      await user().click(within(thread).getByRole("button", { name: "Ask a follow-up" }));
      await user().type(
        within(thread).getByRole("textbox", { name: /^Follow-up in this thread/ }),
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

  it("the launcher counts answers that arrived while the chat was hidden; opening marks them seen", async () => {
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
    act(() => FakeEventSource.latest().emit({ event: "question", question: answered }));
    await waitFor(() => expect(launcher()).toHaveAccessibleName(/^Ask 1 new answer: open your private chat/));
    await user().click(launcher());
    expect(chat().querySelector('[data-thread="t4"]')).toHaveTextContent(/^new/);
    await user().click(within(chat()).getByRole("button", { name: "Hide the Ask chat" }));
    expect(launcher()).toHaveAccessibleName(/^Ask: open your private chat about this PR$/);
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
    expect(await within(chat()).findByRole("alert")).toHaveTextContent(
      "Not sent: You have 50 questions. Try again.",
    );
    expect(within(chat()).getByRole("textbox", { name: "Your question" })).toHaveValue("Why?");
  });
});
