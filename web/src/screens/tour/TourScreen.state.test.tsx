// Tour screen: analysis state, drafts, background refresh failures and the user's questions.
import { act, cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { HttpResponse, delay, http } from "msw";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeEventSource } from "../../__tests__/fake-event-source";
import { renderRoute, startMockServer } from "../../__tests__/screen";
import { expectTrustWording } from "../../__tests__/wording";
import type { AnalysisSummary, CreateCommentRequest, MeResponse, Tour } from "../../api/generated/types";
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
  window.sessionStorage.clear();
});

const user = () => userEvent.setup();
const main = () => screen.getByRole("main");
const nav = () => screen.getByRole("navigation", { name: "Flows and steps" });
const stateBanner = () => within(main()).findByRole("region", { name: "Analysis state" });

async function open(scenario: Parameters<typeof setScenario>[0], hash = "") {
  setScenario(scenario);
  renderRoute(`/analyses/${ID}${hash}`);
  await within(await screen.findByRole("navigation", { name: "Flows and steps" })).findByRole("button", {
    name: /^Flow 1:/,
  });
}

describe("Tour: state of the analysis (paused, usage limit, failed)", () => {
  it("user pause: one line with who paused it and Resume; pending flows say the analysis is paused", async () => {
    await open("lazy-paused");
    expect(await stateBanner()).toHaveTextContent("Analysis paused by alice");
    expect(within(nav()).getAllByText("Not analysed yet — analysis paused").length).toBeGreaterThan(0);
    expect(within(nav()).queryByText("Queued")).toBeNull();
    expectTrustWording(document.body.textContent ?? "");
    await user().click(within(await stateBanner()).getByRole("button", { name: "Resume" }));
    await waitFor(() => expect(within(main()).queryByRole("region", { name: "Analysis state" })).toBeNull());
  });

  it("usage limit: the reason and the time it resumes, no Resume", async () => {
    await open("lazy-limit");
    const banner = await stateBanner();
    expect(banner).toHaveTextContent("The LLM provider's usage limit was reached.");
    expect(banner).toHaveTextContent(/It resumes at .*14:20|It resumes at \d\d:\d\d/);
    expect(within(banner).queryByRole("button", { name: "Resume" })).toBeNull();
  });

  it("failed: the failure sentence and Retry; pending flows never read Queued", async () => {
    await open("lazy-failed");
    const banner = await stateBanner();
    expect(banner).toHaveTextContent("The analysis failed. The repository mirror could not be updated.");
    expect(within(nav()).getAllByText("Not analysed — the analysis failed").length).toBeGreaterThan(0);
    await user().click(within(nav()).getByRole("button", { name: /^Flow 2:/ }));
    expect(main()).toHaveTextContent("Not analysed — the analysis failed");
    expect(main()).not.toHaveTextContent(/\bQueued\b/);
    await user().click(within(banner).getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(within(main()).queryByRole("region", { name: "Analysis state" })).toBeNull());
  });

  it("a failed analysis opened at a resumed step still shows its failure and Retry (Home's Show details and retry)", async () => {
    await open("lazy-failed", "#f1.1");
    const banner = await stateBanner();
    expect(within(banner).getByRole("button", { name: "Retry" })).toBeVisible();
  });
});

describe("Tour: flows are analysed on request only", () => {
  const work = () => within(nav()).getByRole("region", { name: "Analysis work" });

  it("opening a flow not analysed yet requests nothing; Analyse this flow does", async () => {
    const requested: string[] = [];
    server.events.on("request:start", ({ request }) => {
      if (request.method === "POST" && /\/flows\/[^/]+\/request$/.test(new URL(request.url).pathname)) {
        requested.push(new URL(request.url).pathname);
      }
    });
    await open("lazy-flows");
    await user().click(within(nav()).getByRole("button", { name: /^Flow 2:/ }));
    expect(await within(main()).findByRole("button", { name: "Analyse this flow" })).toBeVisible();
    expect(requested).toEqual([]);
    await user().click(within(main()).getByRole("button", { name: "Analyse this flow" }));
    await waitFor(() => expect(requested).toHaveLength(1));
    // The requested flow is queued: the left panel no longer says that nothing is being analysed.
    await waitFor(() => expect(within(work()).queryByRole("status")).toBeNull());
  });

  it("a refused Analyse this flow is said in the flow, next to the button", async () => {
    server.use(
      http.post("/api/v1/analyses/:id/flows/:flow/request", () =>
        HttpResponse.json({ code: "pr_head_changed", message: "The PR has moved." }, { status: 409 }),
      ),
    );
    await open("lazy-flows");
    await user().click(within(nav()).getByRole("button", { name: /^Flow 2:/ }));
    await user().click(await within(main()).findByRole("button", { name: "Analyse this flow" }));
    expect(await within(main()).findByRole("alert")).toHaveTextContent(
      "This flow was not requested: The PR has moved.",
    );
  });

  it("with background off (the default) the left panel says nothing runs and offers Analyse all remaining flows", async () => {
    await open("lazy-flows");
    expect(await within(work()).findByRole("status")).toHaveTextContent(
      "Nothing is being analysed. Choose a flow to analyse, or analyse all remaining flows.",
    );
    await user().click(within(work()).getByRole("button", { name: "Analyse all remaining flows" }));
    expect(
      await within(work()).findByRole("button", { name: "Analysing all remaining flows — Stop" }),
    ).toBeVisible();
    // Like the server, switching it on queues the next flow by rank.
    await waitFor(() => expect(within(work()).queryByRole("status")).toBeNull());
  });

  it("reads the switch from the analysis: on, it offers Stop", async () => {
    await open("lazy-background");
    const stop = await within(work()).findByRole("button", { name: "Analysing all remaining flows — Stop" });
    await user().click(stop);
    expect(await within(work()).findByRole("button", { name: "Analyse all remaining flows" })).toBeVisible();
  });
});

describe("Tour: unsent comment drafts survive navigation", () => {
  const inspector = () => screen.getByRole("complementary", { name: "Inspector" });
  const commentOn95 = async () => {
    const code = await within(main()).findByRole("region", { name: /lines 88 to 96/ });
    await user().click(within(code).getByRole("button", { name: "Comment on line 95" }));
    return within(inspector()).getByRole("textbox", { name: "Comment on src/rpc/client.rs:95" });
  };

  it("Next and back to the line restore the text; a reload of the tab too", async () => {
    await open(null, "#f1.2");
    await user().type(await commentOn95(), "Is the deadline respected?");
    await user().click(within(main()).getByRole("button", { name: /^Next/ }));
    expect(within(inspector()).queryByRole("textbox", { name: /^Comment on/ })).toBeNull();
    await user().click(within(main()).getByRole("button", { name: /^← Previous/ }));
    expect(await commentOn95()).toHaveValue("Is the deadline respected?");
    expect(within(inspector()).getByText("Draft")).toBeVisible();
    const login = (seedFor("default", "me") as MeResponse).user!.login;
    await waitFor(() =>
      expect(window.sessionStorage.getItem(`smart-review.comment-drafts.${login}.${ID}`)).toContain(
        "Is the deadline respected?",
      ),
    );
  });

  it("keeps the drafts when the session ended (401); the same user gets them back after signing in", async () => {
    const login = (seedFor("default", "me") as MeResponse).user!.login;
    const key = `smart-review.comment-drafts.${login}.${ID}`;
    await open(null, "#f1.2");
    await user().type(await commentOn95(), "kept over a timeout");
    await waitFor(() => expect(window.sessionStorage.getItem(key)).toContain("kept over a timeout"));
    server.use(
      http.get("/api/v1/analyses/:id/tour", () =>
        HttpResponse.json(
          { code: "unauthenticated", message: "Sign in again to continue." },
          { status: 401 },
        ),
      ),
    );
    act(() => FakeEventSource.latest().fail());
    expect(await screen.findByText("Sign in again to continue.")).toBeVisible();
    expect(window.sessionStorage.getItem(key)).toContain("kept over a timeout");
    cleanup();
    server.resetHandlers();
    await open(null, "#f1.2");
    expect(await commentOn95()).toHaveValue("kept over a timeout");
  });

  it("after an unknown outcome and a reload, the post comes back locked; Retry sends the same key", async () => {
    const posted: CreateCommentRequest[] = [];
    server.use(
      http.post("/api/v1/analyses/:id/pending-review/comments", async ({ request }) => {
        posted.push((await request.json()) as CreateCommentRequest);
        return HttpResponse.json({ code: "internal", message: "GitHub did not answer." }, { status: 500 });
      }),
    );
    await open(null, "#f1.2");
    await user().type(await commentOn95(), "maybe added");
    await user().click(within(inspector()).getByRole("button", { name: "Add to GitHub pending review" }));
    await within(inspector()).findByRole("button", { name: "Retry" });
    cleanup();
    await open(null, "#f1.2");
    const field = await commentOn95();
    expect(field).toHaveValue("maybe added");
    expect(field).toHaveAttribute("readonly");
    expect(inspector()).toHaveTextContent("The comment may already be in your GitHub pending review");
    await user().click(within(inspector()).getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(posted).toHaveLength(2));
    expect(posted[1]!.idempotency_key).toBe(posted[0]!.idempotency_key);
  });
});

describe("Tour: a background refresh that cannot succeed", () => {
  it("an event stream that ended and cannot reopen (session over) leads to the error page, not stale data", async () => {
    await open(null, "#f1.2");
    server.use(
      http.get("/api/v1/analyses/:id/tour", () =>
        HttpResponse.json(
          { code: "unauthenticated", message: "Sign in again to continue." },
          { status: 401 },
        ),
      ),
    );
    act(() => FakeEventSource.latest().fail());
    expect(await screen.findByText("Sign in again to continue.")).toBeVisible();
    expect(screen.queryByRole("navigation", { name: "Flows and steps" })).toBeNull();
  });

  it.each([
    [401, "unauthenticated", "Sign in again to continue."],
    [404, "not_found", "This analysis does not exist."],
  ])("a %s on refresh shows the error page instead of the old data", async (status, code, message) => {
    await open(null, "#f1.2");
    server.use(http.get("/api/v1/analyses/:id/tour", () => HttpResponse.json({ code, message }, { status })));
    const summary = seedFor("default", "analysis") as AnalysisSummary;
    act(() =>
      FakeEventSource.latest().emit({ event: "analysis", summary: { ...summary, review_ready: true } }),
    );
    expect(await screen.findByText(message)).toBeVisible();
    expect(screen.queryByRole("navigation", { name: "Flows and steps" })).toBeNull();
    expect(screen.getByRole("link", { name: "My work" })).toHaveAttribute("href", "/");
  });
});

describe("Tour: a flow analysed again", () => {
  it("forgets the old ticks of that flow when its new steps arrive, keeps other flows' ticks, reads feedback again", async () => {
    await open(null, "#f1.2");
    await user().keyboard("x");
    await user().click(within(nav()).getByRole("button", { name: /^Flow 2:/ }));
    await user().click(within(nav()).getByRole("button", { name: /^Step 1:/ }));
    await user().keyboard("x");
    await waitFor(() =>
      expect(within(nav()).getByRole("button", { name: /^Step 1: .*, reviewed$/ })).toBeVisible(),
    );
    let feedbackReads = 0;
    server.events.on("request:start", ({ request }) => {
      if (request.method === "GET" && new URL(request.url).pathname.endsWith("/feedback")) feedbackReads += 1;
    });
    const seed = seedFor("default", "tour") as Tour;
    const summary = seedFor("default", "analysis") as AnalysisSummary;
    // Flow 1 is analysed again: its steps are gone while it runs, then new steps with the same ids arrive.
    const running = structuredClone(seed);
    running.flows[0]!.steps = [];
    running.flows[0]!.analysis_status = { state: "analysing", started_at: null, message: null };
    server.use(http.get("/api/v1/analyses/:id/tour", () => HttpResponse.json(running)));
    act(() =>
      FakeEventSource.latest().emit({ event: "analysis", summary: { ...summary, review_ready: true } }),
    );
    await within(nav()).findByRole("button", { name: /^Flow 1:/ });
    await waitFor(() => expect(feedbackReads).toBeGreaterThan(0));
    server.use(http.get("/api/v1/analyses/:id/tour", () => HttpResponse.json(seed)));
    act(() =>
      FakeEventSource.latest().emit({ event: "analysis", summary: { ...summary, review_ready: true } }),
    );
    await user().click(within(nav()).getByRole("button", { name: /^Flow 1:/ }));
    const step2 = await within(nav()).findByRole("button", { name: /^Step 2: / });
    expect(step2.getAttribute("aria-label")).not.toMatch(/reviewed/);
    await user().click(within(nav()).getByRole("button", { name: /^Flow 2:/ }));
    expect(within(nav()).getByRole("button", { name: /^Step 1: .*, reviewed$/ })).toBeVisible();
  });
});

describe("Tour: the user's questions", () => {
  it("the questions page keeps the block the user came from as current, and announces itself", async () => {
    setFixtureState("resume");
    await open(null, "#f1.5");
    const inspector = screen.getByRole("complementary", { name: "Inspector" });
    const before = within(inspector).getAllByRole("heading", { level: 2 }).at(-1)!.textContent;
    await user().click(within(nav()).getByRole("button", { name: "Your questions and answers: 1" }));
    await within(main()).findByRole("article", { name: /Your questions and answers/ });
    expect(within(inspector).getAllByRole("heading", { level: 2 }).at(-1)).toHaveTextContent(before!);
    expect(within(main()).getByRole("button", { name: /^Next/ })).toHaveTextContent(/6/);
    expect(document.querySelector('[aria-live="polite"].sr-only')).toHaveTextContent(
      "Your questions and answers",
    );
  });

  it("the questions entry says while the questions load", async () => {
    server.use(
      http.get("/api/v1/analyses/:id/questions", async () => {
        await delay(300);
        return HttpResponse.json([]);
      }),
    );
    await open(null, "#f1.5");
    expect(
      within(nav()).getByRole("button", { name: "Your questions and answers: loading" }),
    ).toHaveTextContent("…");
    expect(await within(nav()).findByRole("button", { name: "Your questions and answers: 0" })).toBeVisible();
  });

  it("the questions entry says when the questions cannot be read", async () => {
    server.use(
      http.get("/api/v1/analyses/:id/questions", () =>
        HttpResponse.json({ code: "internal", message: "Broken." }, { status: 500 }),
      ),
    );
    await open(null, "#f1.5");
    expect(
      await within(nav()).findByRole("button", { name: "Your questions and answers: could not be read" }),
    ).toBeVisible();
  });

  it("steps with questions are marked in the menu; one entry opens every question with its answer and step", async () => {
    setFixtureState("resume");
    await open(null, "#f1.5");
    const marker = await within(nav()).findByText("?1");
    expect(marker).toHaveAccessibleDescription("1 question asked on this step");
    await user().click(within(nav()).getByRole("button", { name: "Your questions and answers: 1" }));
    const page = await within(main()).findByRole("article", { name: "Your questions and answers (1)" });
    expect(page).toHaveTextContent("Does the node deduplicate broadcast_tx by transaction id?");
    expect(page).toHaveTextContent("LLM answer");
    await user().click(within(page).getByRole("button", { name: /^Flow 1 · step 5/ }));
    expect(within(main()).queryByRole("article", { name: /Your questions/ })).toBeNull();
    expect(within(nav()).getByRole("button", { name: /^Step 5: .*1 question asked/ })).toHaveAttribute(
      "aria-current",
      "step",
    );
  });
});

describe("Tour: a failed background refresh keeps the open review", () => {
  it("keeps the last good data and a typed draft; says it could not refresh, with Retry", async () => {
    await open(null, "#f1.2");
    const inspector = () => screen.getByRole("complementary", { name: "Inspector" });
    const code = await within(main()).findByRole("region", { name: /lines 88 to 96/ });
    await user().click(within(code).getByRole("button", { name: "Comment on line 95" }));
    await user().type(within(inspector()).getByRole("textbox", { name: /^Comment on/ }), "half written");
    server.use(
      http.get("/api/v1/analyses/:id/tour", () =>
        HttpResponse.json({ code: "internal", message: "Tour could not be built." }, { status: 500 }),
      ),
    );
    const summary = seedFor("default", "analysis") as AnalysisSummary;
    act(() =>
      FakeEventSource.latest().emit({ event: "analysis", summary: { ...summary, review_ready: true } }),
    );
    const alert = await within(main()).findByRole("alert");
    expect(alert).toHaveTextContent("Could not refresh the review: Tour could not be built.");
    expect(within(inspector()).getByRole("textbox", { name: /^Comment on/ })).toHaveValue("half written");
    expect(nav()).toBeVisible();
    server.resetHandlers();
    await user().click(within(alert).getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(within(main()).queryByText(/Could not refresh the review/)).toBeNull());
  });
});
