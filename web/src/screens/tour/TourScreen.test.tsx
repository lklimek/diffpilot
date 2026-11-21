import { act, cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { HttpResponse, delay, http } from "msw";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeEventSource } from "../../__tests__/fake-event-source";
import { renderRoute, startMockServer } from "../../__tests__/screen";
import { expectTrustWording } from "../../__tests__/wording";
import type { AnalysisSummary, AskQuestion, ReviewState, Tour } from "../../api/generated/types";
import { seedFor, setFixtureState } from "../../mocks/fixtures";
import { readStore } from "../../mocks/review-store";
import { setScenario } from "../../mocks/scenarios";
import { FLASH_MS } from "../../components/flash/useFlashTarget";
import { LAYOUT_STORAGE_KEY } from "./layout";

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
const main = () => screen.getByRole("main");
const inspector = () => screen.getByRole("complementary", { name: "Inspector" });
const nav = () => screen.getByRole("navigation", { name: "Flows and steps" });
const inspectorHeading = () =>
  within(inspector())
    .getAllByRole("heading", { level: 2 })
    .find((h) => !h.hasAttribute("data-column-title"))!;
const topBar = () => screen.getAllByRole("banner")[0]!;
const blockHeading = () =>
  within(main())
    .getAllByRole("heading", { level: 2 })
    .find((h) => !h.hasAttribute("data-column-title"))!;

async function open(state?: Parameters<typeof setFixtureState>[0]) {
  if (state) setFixtureState(state);
  const router = renderRoute(`/analyses/${ID}`);
  await waitFor(() => expect(blockHeading()).toBeDefined());
  return router;
}

async function openStep(flow: string, step: string) {
  await user().click(within(nav()).getByRole("button", { name: new RegExp(`^Flow ${flow}:`) }));
  await user().click(within(nav()).getByRole("button", { name: new RegExp(`^Step ${step}:`) }));
}

describe("Tour screen (brief §4.3)", () => {
  it("each step shows a collapsible Why this flow strip; collapsed it keeps one line and is remembered", async () => {
    await open();
    await openStep("1", "5");
    const strip = within(main()).getByRole("region", { name: "Why this flow" });
    const toggle = within(strip).getByRole("button", { name: /^Why this flow/ });
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    expect(strip).toHaveTextContent("Problem:");
    await user().click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(strip).not.toHaveTextContent("→ Change:");
    // Collapsed, the LLM's text still carries its tag.
    expect(strip.querySelector("[data-provenance=llm]")).toHaveTextContent("LLM");
    await openStep("1", "6");
    expect(
      within(within(main()).getByRole("region", { name: "Why this flow" })).getByRole("button", {
        name: /^Why this flow/,
      }),
    ).toHaveAttribute("aria-expanded", "false");
    await user().click(within(nav()).getByRole("button", { name: /^Flow 1:/ }));
    expect(within(main()).queryByRole("region", { name: "Why this flow" })).toBeNull();
  });

  it("Your progress opens the flows with their analysis and review state; a flow opens its introduction", async () => {
    await open();
    await user().click(within(topBar()).getByRole("button", { name: /^Your progress 0 \/ 5/ }));
    const flows = screen.getByRole("region", { name: "Flows: analysis and review" });
    expect(within(flows).getAllByRole("listitem")).toHaveLength(5);
    await user().click(within(flows).getByRole("link", { name: /^2 / }));
    expect(blockHeading()).toHaveTextContent("Default timeout 30 s → 5 s");
  });

  it("the flows dropdown names a refused action; the next action that succeeds clears it", async () => {
    setScenario("lazy-flows");
    server.use(
      http.post("/api/v1/analyses/:id/flows/:flow/request", () =>
        HttpResponse.json({ code: "internal", message: "Down." }, { status: 500 }),
      ),
    );
    await open();
    await user().click(within(topBar()).getByRole("button", { name: /^Your progress/ }));
    const flows = screen.getByRole("region", { name: "Flows: analysis and review" });
    const play = within(flows).getAllByRole("button", { name: /^Analyse flow \d+$/ })[0]!;
    const name = play.getAttribute("aria-label")!;
    await user().click(play);
    await waitFor(() => expect(flows).toHaveTextContent(`${name} not done: Down.`));
    await user().click(within(flows).getByRole("button", { name: "Analyse all remaining flows" }));
    await waitFor(() => expect(flows).not.toHaveTextContent("not done"));
  });

  it("the step header says when the file's Viewed mark was not read from GitHub", async () => {
    setScenario("viewed-not-read");
    await open();
    await openStep("2", "1");
    await waitFor(() =>
      expect(within(main()).getByText(/Viewed not read/, { selector: "[data-viewed]" })).toBeVisible(),
    );
  });

  it("s opens the Files view at the step's lines; the step header shows the file's GitHub Viewed mark", async () => {
    const router = await open();
    await openStep("2", "1");
    await waitFor(() =>
      expect(within(main()).getByText(/viewed/, { selector: "[data-viewed]" })).toHaveTextContent("☑ viewed"),
    );
    await user().keyboard("s");
    await waitFor(() => expect(router.state.location.pathname).toBe(`/analyses/${ID}/files`));
    expect(decodeURIComponent(router.state.location.hash)).toBe(
      "#lines?file=src/config.rs&side=head&line=23&step=f2.1",
    );
  });

  it("does not reveal a hidden comment just because it is the step's first changed line", async () => {
    server.use(
      http.get("/api/v1/analyses/:id/lines", () =>
        HttpResponse.json({
          path: "src/rpc/client.rs",
          side: "head",
          from: 88,
          to: 94,
          total_lines: 100,
          lines: ["fn send() {", "// changed comment", "send_request();", "}", "", "", ""],
          changed: [89, 90],
          context_only: [89],
          deleted: [],
          moved: [],
          comments: { scope: "in_diff", line_thread_ranges: [] },
        }),
      ),
    );
    await open();
    await openStep("1", "2");
    expect(
      await within(main()).findByText("1 comment/import line hidden — in the classic diff"),
    ).toBeVisible();
    expect(within(main()).queryByText("// changed comment")).toBeNull();
  });

  it("c comments on the first visible changed line, never on a hidden comment line", async () => {
    server.use(
      http.get("/api/v1/analyses/:id/lines", () =>
        HttpResponse.json({
          path: "src/rpc/client.rs",
          side: "head",
          from: 88,
          to: 94,
          total_lines: 100,
          lines: ["fn send() {", "// changed comment", "send_request();", "}", "", "", ""],
          changed: [89, 90],
          context_only: [89],
          deleted: [],
          moved: [],
          comments: { scope: "in_diff", line_thread_ranges: [] },
        }),
      ),
    );
    await open();
    await openStep("1", "2");
    await within(main()).findByText("1 comment/import line hidden — in the classic diff");
    await user().keyboard("c");
    expect(
      within(inspector()).getByRole("textbox", { name: "Comment on src/rpc/client.rs:90" }),
    ).toHaveFocus();
  });

  it("Show in Inspector on a pinned remark scrolls the Inspector to it and highlights it", async () => {
    await open();
    await openStep("1", "5");
    const code = await within(main()).findByRole("region", { name: /lines 99 to 99/ });
    await user().click(within(code).getByRole("button", { name: "LLM remark at line 99: Red flag" }));
    await user().click(screen.getByRole("button", { name: /^Show in Inspector: Red flag/ }));
    const flags = within(inspector()).getByRole("region", { name: /^Flags/ });
    const item = flags.querySelector('[data-annotation-id="f1.5.a3"]');
    await waitFor(() => expect(item).toHaveAttribute("data-flash"));
  });

  it("Show in Inspector on a pinned GitHub thread highlights the thread in the Inspector", async () => {
    await open();
    await openStep("1", "5");
    const code = await within(main()).findByRole("region", { name: /lines 99 to 99/ });
    await user().click(within(code).getByRole("button", { name: "GitHub thread by bob at line 99" }));
    await user().click(screen.getByRole("button", { name: /^Show in Inspector: GitHub thread by bob/ }));
    const item = inspector().querySelector('[data-thread-id="PRRT_fixture482_t1"]');
    await waitFor(() => expect(item).toHaveAttribute("data-flash"));
  });

  it("Show in Inspector flashes once: hiding and showing the Inspector afterwards does not flash again", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      await open();
      await openStep("1", "5");
      const code = await within(main()).findByRole("region", { name: /lines 99 to 99/ });
      await user().click(within(code).getByRole("button", { name: "LLM remark at line 99: Red flag" }));
      await user().click(screen.getByRole("button", { name: /^Show in Inspector: Red flag/ }));
      const flashed = () => inspector().querySelector('[data-annotation-id="f1.5.a3"]');
      await waitFor(() => expect(flashed()).toHaveAttribute("data-flash"));
      act(() => vi.advanceTimersByTime(FLASH_MS));
      expect(flashed()).not.toHaveAttribute("data-flash");
      await user().click(screen.getByRole("button", { name: "Hide Inspector column" }));
      await user().click(within(inspector()).getByRole("button", { name: "Show Inspector column" }));
      await waitFor(() => expect(flashed()).not.toBeNull());
      act(() => vi.advanceTimersByTime(1000));
      expect(flashed()).not.toHaveAttribute("data-flash");
    } finally {
      vi.useRealTimers();
    }
  });

  it("a finished analysis offers Analyse this flow again for a stored flow that fails today's checks", async () => {
    const tour = structuredClone(seedFor("default", "tour") as Tour);
    expect(tour.analysis.state.state).toBe("finished");
    const f2 = tour.flows.find((f) => f.id === "f2")!;
    f2.steps = [];
    f2.analysis_status = {
      state: "failed",
      started_at: null,
      message:
        "The stored analysis of this flow does not pass the server's current checks, so its changes are listed as not analysed. Analyse this flow again.",
    };
    const requested: string[] = [];
    server.use(
      http.get("/api/v1/analyses/:id/tour", () => HttpResponse.json(tour)),
      http.post("/api/v1/analyses/:id/flows/:flow/request", ({ params }) => {
        requested.push(String(params.flow));
        return HttpResponse.json(tour.analysis);
      }),
    );
    await open();
    await user().click(within(nav()).getByRole("button", { name: /^Flow 2:/ }));
    await user().click(await within(main()).findByRole("button", { name: "Analyse this flow again" }));
    await waitFor(() => expect(requested).toEqual(["f2"]));
  });

  it("a GitHub thread of another step offers Show in step N.M, which switches to that step", async () => {
    const tour = structuredClone(seedFor("default", "tour") as Tour);
    tour.threads = tour.threads.map((t) => (t.id === "PRRT_fixture482_t1" ? { ...t, step_id: "f1.4" } : t));
    server.use(http.get("/api/v1/analyses/:id/tour", () => HttpResponse.json(tour)));
    await open();
    await openStep("1", "5");
    const code = await within(main()).findByRole("region", { name: /lines 99 to 99/ });
    await user().click(within(code).getByRole("button", { name: "GitHub thread by bob at line 99" }));
    expect(screen.queryByRole("button", { name: /^Show in Inspector: GitHub thread by bob/ })).toBeNull();
    await user().click(screen.getByRole("button", { name: "Show in step 1.4: GitHub thread by bob" }));
    await waitFor(() => expect(blockHeading()).toHaveTextContent(/^4 Return the response/));
  });

  it("an evidence link highlights the line it jumps to", async () => {
    renderRoute(`/analyses/${ID}#lines?file=src%2Frpc%2Fclient.rs&side=head&line=99`);
    const code = await screen.findByRole("region", { name: /^src\/rpc\/client\.rs, lines/ });
    const row = await waitFor(() => code.querySelector('[data-line="99"][data-side="head"]')!);
    await waitFor(() => expect(row).toHaveAttribute("data-flash"));
  });

  it("walks beyond the last step into quiet rows and pending review", async () => {
    setScenario("real-small");
    await open();
    await openStep("1", "6");
    await user().keyboard("n");
    expect(blockHeading()).toHaveTextContent("Tests");
    await user().keyboard("j");
    expect(blockHeading()).toHaveTextContent("Pending review");
    expect(within(main()).getByRole("button", { name: "Next →" })).toHaveAttribute("aria-disabled", "true");
  });

  it("opens Not analysed ranges with line commenting and no premature dock hint", async () => {
    setScenario("real-tiny");
    await open();
    await user().click(within(nav()).getByRole("button", { name: /Docs & comments/ }));
    expect(inspector()).not.toHaveTextContent("Click a line number");
    expect(inspector()).not.toHaveTextContent("count only");
    await user().click(within(main()).getByRole("button", { name: "Show lines 24–27" }));
    const line = await within(main()).findByRole("button", { name: "Comment on line 24" });
    await user().click(line);
    expect(within(inspector()).getByRole("textbox", { name: /^Comment on/ })).toBeVisible();
  });

  it("stores blocks in the URL and follows Back and Forward", async () => {
    const router = await open();
    await user().keyboard("jj");
    expect(router.state.location.hash).toBe("#f1.1");
    await act(() => router.navigate(-1));
    expect(blockHeading()).toHaveTextContent("Send an RPC request with retry");
    await act(() => router.navigate(1));
    expect(blockHeading()).toHaveTextContent("Trigger: callers of send");
  });

  it("a deep link takes precedence over the server resume position", async () => {
    setFixtureState("resume");
    renderRoute(`/analyses/${ID}#f1.2`);
    await waitFor(() => expect(blockHeading()).toHaveTextContent("2"));
    expect(blockHeading()).not.toHaveTextContent("Retry allowed?");
  });

  it("opens structured fit evidence as code when no step shows it", async () => {
    await open();
    await user().click(within(main()).getByRole("button", { name: "Show src/config.rs:23" }));
    expect(await within(main()).findByRole("button", { name: "Comment on line 23" })).toBeVisible();
  });

  it("sends an analysis that has not finished to its progress screen", async () => {
    setScenario("running");
    const router = renderRoute(`/analyses/${ID}`);
    await waitFor(() => expect(router.state.location.pathname).toBe(`/analyses/${ID}/progress`));
  });

  it("starts at PR context the first time and stores it as the last viewed block", async () => {
    await open();
    expect(blockHeading()).toHaveTextContent("PR context: acme/node-client #482");
    await waitFor(() => expect(readStore("reviewState").last_viewed).toEqual({ block: "pr_context" }));
  });

  it("returns to the last viewed block with the stored ticks (interrupted review)", async () => {
    await open("resume");
    expect(blockHeading()).toHaveTextContent("5 Retry allowed?");
    expect(topBar()).toHaveTextContent("Your progress 0 / 5");
  });

  it("x ticks the current step; a flow with every step ticked counts in the top bar", async () => {
    await open("resume");
    await openStep("2", "2");
    await act(() => user().keyboard("x"));
    expect(topBar()).toHaveTextContent("Your progress 1 / 5");
    expect(within(nav()).getByRole("button", { name: /^Flow 2: .*, reviewed$/ })).toBeInTheDocument();
    await waitFor(() => expect(readStore("reviewState").ticked_steps).toEqual(["f2.1", "f2.2"]));
    await user().click(within(main()).getByRole("button", { name: /Mark reviewed/ }));
    expect(topBar()).toHaveTextContent("Your progress 0 / 5");
  });

  it("Next and j tick the current step and move on; Previous and the flow introduction never tick", async () => {
    await open();
    await openStep("1", "1");
    await user().click(within(main()).getByRole("button", { name: /^Next: / }));
    await waitFor(() => expect(readStore("reviewState").ticked_steps).toEqual(["f1.1"]));
    expect(blockHeading()).not.toHaveTextContent("Trigger: callers of send");
    await user().keyboard("j");
    await waitFor(() => expect(readStore("reviewState").ticked_steps).toEqual(["f1.1", "f1.2"]));
    const reviewedSteps = () => within(nav()).queryAllByRole("button", { name: /^Step \d+: .*, reviewed$/ });
    expect(reviewedSteps()).toHaveLength(2);
    await user().click(within(main()).getByRole("button", { name: /^← Previous/ }));
    await user().keyboard("k");
    expect(reviewedSteps()).toHaveLength(2);
    expect(within(nav()).queryByRole("button", { name: /^Step 3: .*, reviewed$/ })).toBeNull();
    await user().click(within(nav()).getByRole("button", { name: /^Flow 3:/ }));
    await user().keyboard("j");
    expect(reviewedSteps()).toHaveLength(0);
  });

  it("the menu's questions entry opens the Ask chat and the block stays where it was", async () => {
    await open();
    await openStep("1", "5");
    await user().click(within(nav()).getByRole("button", { name: /^Your questions and answers/ }));
    expect(screen.getByRole("dialog", { name: "Ask" })).toBeVisible();
    expect(blockHeading()).toHaveTextContent("5 Retry allowed?");
  });

  it("a failed save on Next still moves on, keeps the tick on the page and says it is not saved", async () => {
    server.use(
      http.put("/api/v1/analyses/:id/review-state", () =>
        HttpResponse.json({ code: "internal", message: "Down." }, { status: 500 }),
      ),
    );
    await open();
    await openStep("1", "1");
    await user().click(within(main()).getByRole("button", { name: /^Next: / }));
    expect(blockHeading()).not.toHaveTextContent("Trigger: callers of send");
    expect(await within(main()).findByRole("alert")).toHaveTextContent(
      "Your ticks and last viewed block are not saved",
    );
    expect(within(nav()).getByRole("button", { name: /^Step 1: .*, reviewed$/ })).toBeInTheDocument();
  });

  it("Next keeps an already ticked step ticked", async () => {
    await open();
    await openStep("1", "1");
    await act(() => user().keyboard("x"));
    await waitFor(() => expect(readStore("reviewState").ticked_steps).toEqual(["f1.1"]));
    await user().keyboard("j");
    await user().keyboard("k");
    await waitFor(() => expect(readStore("reviewState").ticked_steps).toEqual(["f1.1"]));
    expect(within(nav()).getByRole("button", { name: /^Step 1: .*, reviewed$/ })).toBeInTheDocument();
  });

  it("Mark flow reviewed ticks every step of the flow", async () => {
    await open();
    await user().click(within(nav()).getByRole("button", { name: /^Flow 3:/ }));
    await user().click(within(main()).getByRole("button", { name: /Mark flow reviewed/ }));
    expect(topBar()).toHaveTextContent("Your progress 1 / 5");
    await waitFor(() => expect(readStore("reviewState").ticked_steps).toEqual(["f3.1", "f3.2"]));
  });

  it("the menu's flow toggle ticks every step of the flow, and unticks them when all are ticked", async () => {
    await open();
    const toggle = () => within(nav()).getByRole("button", { name: "Mark flow 3 reviewed" });
    await user().click(toggle());
    expect(topBar()).toHaveTextContent("Your progress 1 / 5");
    await waitFor(() => expect(readStore("reviewState").ticked_steps).toEqual(["f3.1", "f3.2"]));
    expect(toggle()).toHaveAttribute("aria-pressed", "true");
    await user().click(toggle());
    expect(topBar()).toHaveTextContent("Your progress 0 / 5");
    await waitFor(() => expect(readStore("reviewState").ticked_steps).toEqual([]));
  });

  it("shows the step's and the flow's LLM flags next to their rating pills above the block", async () => {
    await open();
    await user().click(within(nav()).getByRole("button", { name: /^Flow 4:/ }));
    expect(
      within(main()).getByRole("button", { name: "Go to the first red flag in this flow" }),
    ).toHaveTextContent("⚑ red");
    await openStep("2", "1");
    expect(
      within(main()).getByRole("button", { name: "Go to the first red flag on this step" }),
    ).toHaveTextContent("⚑ red");
  });

  it("a flow's flag marker in the menu opens the step of its first strongest flag and flashes that flag", async () => {
    await open();
    // Flow 4's one step has a yellow flag (line 52) before its red flag (line 53): the red one is the target.
    await user().click(within(nav()).getByRole("button", { name: "Go to the first red flag in flow 4" }));
    expect(await within(main()).findByRole("heading", { level: 2, name: /^1/ })).toBeVisible();
    await waitFor(() =>
      expect(inspector().querySelector('[data-annotation-id="f4.1.a2"]')).toHaveAttribute("data-flash"),
    );
    expect(inspector().querySelector('[data-annotation-id="f4.1.a3"]')).not.toHaveAttribute("data-flash");
  });

  it("j / k walk the blocks, n / p the flow introductions", async () => {
    await open();
    await user().keyboard("j");
    expect(blockHeading()).toHaveTextContent("Send an RPC request with retry");
    await user().keyboard("j");
    expect(blockHeading()).toHaveTextContent("Trigger: callers of send");
    await user().keyboard("n");
    expect(blockHeading()).toHaveTextContent(/^2/);
    await user().keyboard("p");
    expect(blockHeading()).toHaveTextContent("Send an RPC request with retry");
    await user().keyboard("k");
    expect(blockHeading()).toHaveTextContent("PR context");
  });

  it("? opens the help with the keyboard map; Esc closes it", async () => {
    await open();
    await user().keyboard("?");
    const dialog = screen.getByRole("dialog", { name: "Keyboard shortcuts" });
    expect(dialog).toHaveTextContent("next flow");
    await user().keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("hides a column to a strip that restores it on click, and remembers it", async () => {
    await open();
    await user().click(screen.getByRole("button", { name: "Hide Inspector column" }));
    expect(within(inspector()).queryByRole("group", { name: "Feedback and comment" })).toBeNull();
    expect(JSON.parse(window.localStorage.getItem(LAYOUT_STORAGE_KEY)!).hidden.inspector).toBe(true);
    await user().click(within(inspector()).getByRole("button", { name: "Show Inspector column" }));
    expect(within(inspector()).getByRole("group", { name: "Feedback and comment" })).toBeVisible();
  });

  it("names the three columns with level-2 headings", async () => {
    await open();
    expect(within(nav()).getByRole("heading", { level: 2, name: "Flows" })).toBeInTheDocument();
    expect(within(main()).getByRole("heading", { level: 2, name: "Review" })).toBeInTheDocument();
    expect(within(inspector()).getByRole("heading", { level: 2, name: "Inspector" })).toBeInTheDocument();
  });

  it("Hide on the last visible column is disabled and says why", async () => {
    await open();
    await user().click(screen.getByRole("button", { name: "Hide Flows column" }));
    await user().click(screen.getByRole("button", { name: "Hide Inspector column" }));
    const hide = screen.getByRole("button", { name: "Hide Review column" });
    expect(hide).toHaveAttribute("aria-disabled", "true");
    expect(hide).toHaveAccessibleDescription("At least one column stays visible");
    await user().click(hide);
    expect(screen.getByRole("button", { name: "Hide Review column" })).toBeInTheDocument();
    await user().click(within(nav()).getByRole("button", { name: "Show Flows column" }));
    expect(screen.getByRole("button", { name: "Hide Review column" })).not.toHaveAttribute(
      "aria-disabled",
      "true",
    );
  });

  it("says once that the review resumed where it was left; Dismiss and moving on remove it", async () => {
    await open("resume");
    expect(blockHeading()).toHaveTextContent("5 Retry allowed?");
    expect(within(main()).getByText("Resumed where you left off: flow 1, step 5.")).toHaveAttribute(
      "role",
      "status",
    );
    await user().click(within(main()).getByRole("button", { name: "Dismiss" }));
    expect(within(main()).queryByText(/Resumed where you left off/)).toBeNull();
    cleanup();
    await open("resume");
    await user().keyboard("j");
    expect(within(main()).queryByText(/Resumed where you left off/)).toBeNull();
  });

  it("says nothing about resuming when the review starts at PR context", async () => {
    await open();
    expect(blockHeading()).toHaveTextContent("PR context");
    expect(within(main()).queryByText(/Resumed where you left off/)).toBeNull();
  });

  it("full screen shows one column; Esc returns", async () => {
    await open();
    await user().click(screen.getByRole("button", { name: "Full screen Review column" }));
    expect(within(nav()).getByRole("button", { name: "Show Flows column" })).toBeInTheDocument();
    expect(within(inspector()).getByRole("button", { name: "Show Inspector column" })).toBeInTheDocument();
    await user().keyboard("{Escape}");
    expect(within(nav()).getByRole("button", { name: "PR context" })).toBeInTheDocument();
  });

  it("draws a rated step's highlight with its risk band, never 'not rated'", async () => {
    await open();
    await openStep("1", "5");
    const code = await within(main()).findByRole("region", { name: /src\/rpc\/client\.rs, lines 99 to 99/ });
    expect(code.querySelector("[data-band=high]")).not.toBeNull();
    expect(code).not.toHaveTextContent("not rated");
  });

  it("a highlight narrows the inspector to its annotation; feedback then targets it; Back to step returns", async () => {
    await open();
    await openStep("1", "5");
    const code = await within(main()).findByRole("region", { name: /lines 99 to 99/ });
    await user().click(within(code).getByRole("button", { name: /HIGH · LLM remark/ }));
    expect(inspectorHeading()).toHaveTextContent(/line 99/);
    await user().click(within(inspector()).getByRole("button", { name: "wrong" }));
    await waitFor(() =>
      expect(readStore("feedback").map((f) => f.target)).toEqual([
        { target: "annotation", annotation_id: expect.stringMatching(/^f1\.5\.a/) },
      ]),
    );
    await user().click(within(inspector()).getByRole("button", { name: "← Back to step" }));
    expect(inspectorHeading()).toHaveTextContent("Step 5 · Retry allowed?");
  });

  it("d shows the old code of the step as a classic diff", async () => {
    await open();
    await openStep("1", "2");
    await within(main()).findByRole("region", { name: /src\/rpc\/client\.rs, lines 88 to 96/ });
    await user().keyboard("d");
    const diff = await within(main()).findByRole("region", { name: "Classic diff of src/rpc/client.rs" });
    expect(diff.querySelector("[data-diff-line=removed]")).not.toBeNull();
    await user().keyboard("d");
    expect(await within(main()).findByRole("region", { name: /lines 88 to 96/ })).toBeInTheDocument();
  });

  it("comments on a clicked line into the pending review; the count and the marker follow", async () => {
    await open("missed");
    await openStep("1", "2");
    expect(topBar()).toHaveTextContent("Pending review 1");
    const code = await within(main()).findByRole("region", { name: /lines 88 to 96/ });
    await user().click(within(code).getByRole("button", { name: "Comment on line 95" }));
    const field = within(inspector()).getByRole("textbox", { name: "Comment on src/rpc/client.rs:95" });
    expect(field).toHaveFocus();
    await user().type(field, "Is the deadline of the caller respected?");
    await user().click(within(inspector()).getByRole("button", { name: "Add to GitHub pending review" }));
    expect(await within(inspector()).findByText(/Added to your GitHub pending review/)).toBeInTheDocument();
    await waitFor(() => expect(topBar()).toHaveTextContent("Pending review 2"));
    expect(readStore("pendingReview").comments.map((c) => c.line)).toEqual([96, 95]);
  });

  it("c starts a comment on the step's first changed line; Esc leaves it", async () => {
    await open();
    await openStep("1", "5");
    await within(main()).findByRole("region", { name: /lines 99 to 99/ });
    await user().keyboard("c");
    const field = within(inspector()).getByRole("textbox", { name: "Comment on src/rpc/client.rs:99" });
    expect(field).toHaveFocus();
    await user().keyboard("{Escape}");
    expect(within(inspector()).queryByRole("textbox", { name: /^Comment on/ })).toBeNull();
  });

  it("a opens the Ask chat about the step; the question joins it and its answer arrives on the event stream", async () => {
    await open();
    await openStep("1", "5");
    expect(screen.queryByRole("dialog", { name: "Ask" })).toBeNull();
    await user().keyboard("a");
    const chat = screen.getByRole("dialog", { name: "Ask" });
    const field = within(chat).getByRole("textbox", { name: "Your question" });
    await waitFor(() => expect(field).toHaveFocus());
    expect(chat).toHaveTextContent(
      /About:\s*Flows src\/rpc\/client\.rs:99 flow 1 · step 5 · Retry allowed\?/,
    );
    await user().type(field, "Is the retry idempotent?{Enter}");
    const thread = await within(chat).findByRole("listitem", { name: "Question: Is the retry idempotent?" });
    const asked = readStore("questions").at(-1)!;
    expect(asked.anchor).toEqual({ kind: "step", step_id: "f1.5", lines: null });
    const answered: AskQuestion = {
      ...asked,
      state: "answered",
      answer: { answer: "It is not.", evidence: [] },
    };
    act(() => FakeEventSource.latest().emit({ event: "question", question: answered }));
    expect(await within(thread).findByText("It is not.")).toBeInTheDocument();
    // The Inspector keeps a read-only copy of the step's threads.
    const copy = within(inspector()).getByRole("region", { name: /^Your questions about this step \(\d+\)/ });
    expect(copy).toHaveTextContent("Is the retry idempotent?");
    expect(within(inspector()).queryByRole("textbox", { name: /Ask/ })).toBeNull();
    // Right after the step's Flags (proposal AC11), not at the end of the Inspector.
    expect(copy.previousElementSibling?.textContent).toMatch(/Flags \(\d+\)/);
  });

  it("rating pills: the flow intro and step show them; a pill jumps to the first remark and flashes it", async () => {
    await open();
    await user().click(within(nav()).getByRole("button", { name: /^Flow 1:/ }));
    const flowPill = await within(main()).findByRole("button", {
      name: "LLM flags in this flow: 1 red flag, 4 yellow flags",
    });
    expect(flowPill).toHaveTextContent(/^$/);
    await user().click(flowPill);
    expect(await within(main()).findByRole("heading", { level: 2, name: /Retry allowed/ })).toBeVisible();
    const stepPill = within(main()).getByRole("button", { name: "LLM rated this step HIGH (not checked)" });
    expect(stepPill).toHaveTextContent(/^▲ HIGH$/);
    expect(stepPill).toHaveAccessibleDescription(/^LLM flags on this step: 1 red flag\./);
    // The code line of the first remark and its Inspector entry flash.
    await waitFor(() => expect(main().querySelector("[data-flash]")).not.toBeNull());
    await waitFor(() => expect(inspector().querySelector("[data-flash]")).not.toBeNull());
  });

  it("r goes to the next step with LLM remarks after the current position", async () => {
    await open();
    await openStep("1", "4");
    await user().keyboard("r");
    expect(await within(main()).findByRole("heading", { level: 2, name: /Retry allowed/ })).toBeVisible();
  });

  it("r from a flow introduction starts at that flow; after the last step it wraps to the first", async () => {
    await open();
    await user().click(within(nav()).getByRole("button", { name: /^Flow 2:/ }));
    await user().keyboard("r");
    expect(
      await within(main()).findByRole("heading", { level: 2, name: /Default timeout 30 s/ }),
    ).toBeVisible();
    expect(within(main()).getByRole("button", { name: /^LLM rated this step MEDIUM/ })).toBeVisible();
    await openStep("5", "2");
    await user().keyboard("r");
    expect(
      await within(main()).findByRole("heading", { level: 2, name: /Trigger: callers of send/ }),
    ).toBeVisible();
  });

  it("r says so when no step has LLM remarks", async () => {
    server.use(
      http.get("/api/v1/analyses/:id/tour", () => {
        const tour = structuredClone(seedFor("default", "tour") as Tour);
        for (const s of tour.flows.flatMap((f) => f.steps)) s.llm.annotations = [];
        return HttpResponse.json(tour);
      }),
    );
    await open();
    await user().keyboard("r");
    expect(await screen.findByText("No more steps with remarks")).toBeInTheDocument();
  });

  it("the chat stays open across steps and its subject follows the step; × asks about the whole PR", async () => {
    await open();
    await openStep("1", "5");
    await user().keyboard("a");
    const chat = screen.getByRole("dialog", { name: "Ask" });
    await user().type(within(chat).getByRole("textbox", { name: "Your question" }), "draft");
    await openStep("1", "6");
    expect(screen.getByRole("dialog", { name: "Ask" })).toBe(chat);
    expect(within(chat).getByRole("textbox", { name: "Your question" })).toHaveValue("draft");
    expect(chat).toHaveTextContent(/About:\s*Flows src\/rpc\/client\.rs:100–103 flow 1 · step 6/);
    await user().click(
      within(chat).getByRole("button", { name: "Remove the subject: ask about the whole PR" }),
    );
    expect(chat).toHaveTextContent(/About:\s*Whole PR/);
    await user().click(within(chat).getByRole("button", { name: "about this step instead" }));
    expect(chat).toHaveTextContent(/About:\s*Flows src\/rpc\/client\.rs:100–103/);
    // Esc hides it; the launcher brings it back with the draft.
    await user().click(within(chat).getByRole("textbox", { name: "Your question" }));
    await user().keyboard("{Escape}");
    expect(screen.queryByRole("dialog", { name: "Ask" })).toBeNull();
    await user().click(screen.getByRole("button", { name: /^Ask: open your private chat/ }));
    expect(
      within(screen.getByRole("dialog", { name: "Ask" })).getByRole("textbox", { name: "Your question" }),
    ).toHaveValue("draft");
  });

  it("a lost answer stream is announced in the chat", async () => {
    await open();
    await openStep("1", "5");
    await user().keyboard("a");
    act(() => FakeEventSource.latest().fail());
    expect(
      await within(screen.getByRole("dialog", { name: "Ask" })).findByText(
        /The connection that delivers answers was lost/,
      ),
    ).toBeInTheDocument();
  });

  it("the flow diagram is hidden by default, loads on demand and opens a clicked step", async () => {
    await open();
    await user().click(within(nav()).getByRole("button", { name: /^Flow 1:/ }));
    const toggle = within(main()).getByRole("button", { name: /Show flow diagram/ });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    await user().click(toggle);
    const node = await within(main()).findByRole("button", { name: /^Step 5, .*Retry allowed\?/ });
    await user().click(node);
    expect(blockHeading()).toHaveTextContent("5 Retry allowed?");
  });

  it("the pending review list edits and deletes on GitHub", async () => {
    await open("missed");
    await user().click(within(topBar()).getByRole("button", { name: /Pending review/ }));
    const block = within(main());
    await user().click(block.getByRole("button", { name: /^Delete comment on src\/rpc\/client\.rs:96/ }));
    await waitFor(() => expect(readStore("pendingReview").comments).toEqual([]));
    await waitFor(() => expect(topBar()).toHaveTextContent("Pending review 0"));
  });

  it("a review state that cannot be read: the tour opens at PR context and says changes stay on this page", async () => {
    server.use(
      http.get("/api/v1/analyses/:id/review-state", () =>
        HttpResponse.json({ code: "not_found", message: "Not here." }, { status: 404 }),
      ),
    );
    await open("resume");
    expect(blockHeading()).toHaveTextContent("PR context");
    expect(within(main()).getByRole("alert")).toHaveTextContent(
      "Your ticks and last viewed block could not be read. Changes stay on this page and are saved once they can be read: Not here.",
    );
    await user().keyboard("j");
    expect(readStore("reviewState").last_viewed).toEqual({ block: "step", step_id: "f1.5" });
  });

  it("leaving and reopening while a save is slow keeps one queue: the newest block is stored last", async () => {
    const finished: ReviewState[] = [];
    server.use(
      http.put("/api/v1/analyses/:id/review-state", async ({ request }) => {
        const body = (await request.json()) as ReviewState;
        if (body.last_viewed?.block === "flow_intro") await delay(150);
        finished.push(body);
        return new HttpResponse(null, { status: 204 });
      }),
    );
    const router = await open();
    await user().keyboard("j");
    await act(() => router.navigate("/"));
    await act(() => router.navigate(`/analyses/${ID}`));
    await waitFor(() => expect(blockHeading()).toHaveTextContent("Send an RPC request with retry"));
    await user().keyboard("j");
    await waitFor(() => expect(finished.at(-1)?.last_viewed).toEqual({ block: "step", step_id: "f1.1" }));
    await new Promise((r) => setTimeout(r, 200));
    expect(finished.map((b) => b.last_viewed?.block)).toEqual(["pr_context", "flow_intro", "step"]);
  });

  it("after a failed read, reopening never writes over the stored ticks", async () => {
    const written: ReviewState[] = [];
    server.use(
      http.get(
        "/api/v1/analyses/:id/review-state",
        () => HttpResponse.json({ code: "internal", message: "Down." }, { status: 500 }),
        { once: true },
      ),
      http.put("/api/v1/analyses/:id/review-state", async ({ request }) => {
        written.push((await request.json()) as ReviewState);
        return new HttpResponse(null, { status: 204 });
      }),
    );
    const router = await open("resume");
    await user().keyboard("j");
    await act(() => router.navigate("/"));
    await act(() => router.navigate(`/analyses/${ID}`));
    // The block moved to before the read succeeded wins over the stored one.
    await waitFor(() => expect(blockHeading()).toHaveTextContent("Send an RPC request with retry"));
    expect(within(main()).queryByRole("alert")).toBeNull();
    await user().keyboard("jx");
    await waitFor(() => expect(written.length).toBeGreaterThan(0));
    expect(written.every((w) => w.ticked_steps.includes("f2.1"))).toBe(true);
  });

  it("after a failed read, a change made meanwhile is merged with the stored state and saved", async () => {
    const written: ReviewState[] = [];
    server.use(
      http.get(
        "/api/v1/analyses/:id/review-state",
        () => HttpResponse.json({ code: "internal", message: "Down." }, { status: 500 }),
        { once: true },
      ),
      http.put("/api/v1/analyses/:id/review-state", async ({ request }) => {
        written.push((await request.json()) as ReviewState);
        return new HttpResponse(null, { status: 204 });
      }),
    );
    await open("resume");
    expect(blockHeading()).toHaveTextContent("PR context");
    await user().keyboard("j");
    await waitFor(() => expect(written.length).toBeGreaterThan(0));
    // The stored tick is kept and the block the user moved to wins over the stored one.
    expect(written.at(-1)).toMatchObject({ ticked_steps: expect.arrayContaining(["f2.1"]) });
    expect(written.at(-1)!.last_viewed?.block).toBe("flow_intro");
    await waitFor(() => expect(within(main()).queryByRole("alert")).toBeNull());
    expect(blockHeading()).not.toHaveTextContent("Retry allowed?");
  });

  it("offers feedback on a line only for a changed line of the diff; elsewhere feedback stays on the step", async () => {
    await open();
    await openStep("1", "2");
    const code = await within(main()).findByRole("region", { name: /lines 88 to 96/ });
    await user().click(within(code).getByRole("button", { name: "Comment on line 96" }));
    expect(within(inspector()).getByRole("group", { name: "Feedback on line 96" })).toBeInTheDocument();
    await user().click(within(code).getByRole("button", { name: "Comment on line 88" }));
    expect(within(inspector()).queryByRole("group", { name: /Feedback on line/ })).toBeNull();
    expect(within(inspector()).getByRole("group", { name: "Feedback on step 2" })).toBeInTheDocument();
  });

  it("a comment published meanwhile: the composer stays open with the server's sentence (comment_published)", async () => {
    setScenario("comment-published");
    await open();
    await openStep("1", "2");
    const code = await within(main()).findByRole("region", { name: /lines 88 to 96/ });
    await user().click(within(code).getByRole("button", { name: "Comment on line 96" }));
    await user().type(
      within(inspector()).getByRole("textbox", { name: /^Comment on/ }),
      "Full timeout per attempt.",
    );
    await user().click(within(inspector()).getByRole("button", { name: "Add to GitHub pending review" }));
    expect(await within(inspector()).findByRole("alert")).toHaveTextContent(
      "Analyse the pull request again to comment.",
    );
    expect(within(inspector()).getByRole("textbox", { name: /^Comment on/ })).toHaveValue(
      "Full timeout per attempt.",
    );
    expect(within(inspector()).queryByRole("button", { name: "Retry" })).toBeNull();
  });

  it("an incomplete pending-review list is marked in the top bar and the menu", async () => {
    setScenario("pending-incomplete");
    await open();
    await waitFor(() => expect(topBar()).toHaveTextContent("Pending review 0+"));
    expect(
      within(nav()).getByRole("button", { name: /Pending review on GitHub: 0\+ comments/ }),
    ).toBeInTheDocument();
  });

  it.each([null, "unchecked-facts"] as const)(
    "the menu's Unexplained count equals the ranges the Unexplained block shows (scenario %s)",
    async (name) => {
      setScenario(name);
      await open();
      const row = within(nav()).getByRole("button", { name: /^Unexplained/ });
      const [, lines, files] = /(\d+) changed lines? in (\d+) files?/.exec(row.textContent ?? "")!;
      await user().click(row);
      const items = within(main()).getAllByRole("listitem");
      let shown = 0;
      const shownFiles = new Set<string>();
      for (const item of items) {
        const [file, ranges] = item.querySelectorAll("p:first-child > *");
        shownFiles.add(file!.textContent ?? "");
        for (const [, from, to] of (ranges!.textContent ?? "").matchAll(/(\d+)(?:–(\d+))?/g)) {
          shown += Number(to ?? from) - Number(from) + 1;
        }
      }
      expect(shown).toBeGreaterThan(0);
      expect(shown).toBe(Number(lines));
      expect(shownFiles.size).toBe(Number(files));
    },
  );

  it("keeps the trust wording in every fixture state", async () => {
    for (const state of ["default", "wrong", "missed", "noq", "noprofile", "resume"] as const) {
      await open(state);
      expectTrustWording(document.body.textContent ?? "");
      cleanup();
    }
  });
});

describe("Use as review comment (owner request F31): anchors and availability", () => {
  const chat = () => screen.getByRole("dialog", { name: "Ask" });
  const useButtons = () => within(chat()).queryAllByRole("button", { name: "Use as review comment" });
  const thread = (id: string) =>
    waitFor(() => {
      const found = chat().querySelector(`[data-thread="${id}"]`);
      expect(found).not.toBeNull();
      return found as HTMLElement;
    });
  const composer = () => within(inspector()).getByRole("textbox", { name: /^Comment on / });

  it("an answer with no comment open goes on the step's first changed line", async () => {
    setScenario("ask-thread");
    await open();
    await openStep("1", "2");
    await user().keyboard("a");
    await user().click(within(await thread("t2")).getByRole("button", { name: "Use as review comment" }));
    expect(composer()).toHaveAccessibleName("Comment on src/rpc/client.rs:89");
    expect((composer() as HTMLTextAreaElement).value).toContain("In `RpcConfig::default`.");
  });

  it("an answer about lines of a file opens them in the Files view and drafts there", async () => {
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
    const router = await open();
    await openStep("1", "5");
    await user().keyboard("a");
    await user().click(within(await thread("l1")).getByRole("button", { name: "Use as review comment" }));
    await waitFor(() => expect(router.state.location.pathname).toMatch(/\/files$/));
    expect(decodeURIComponent(router.state.location.hash)).toBe(
      "#lines?file=src/config.rs&side=head&line=3&end=4",
    );
    // The draft is waiting there, focused, and the move is said.
    const files = await screen.findByRole("textbox", { name: /^Comment on src\/config\.rs:3–4/ });
    await waitFor(() => expect(files).toHaveFocus());
    expect((files as HTMLTextAreaElement).value).toContain("Because.");
    await waitFor(() =>
      expect(
        within(screen.getByRole("dialog", { name: "Ask" })).getByText(
          /^Opened src\/config\.rs:3–4 to comment on its lines/,
        ),
      ).toBeVisible(),
    );
  });

  it("an answer about another step opens that step and drafts there", async () => {
    setScenario("ask-thread");
    await open();
    await openStep("1", "5");
    await user().keyboard("a");
    await user().click(within(await thread("t2")).getByRole("button", { name: "Use as review comment" }));
    await waitFor(() => expect(blockHeading()).toHaveTextContent(/^2 /));
    expect(screen.getByRole("dialog", { name: "Ask" })).toBeVisible();
    // The draft goes on that step's first changed line, focused, and the move is said.
    await waitFor(() => expect(composer()).toHaveFocus());
    expect(composer()).toHaveAccessibleName("Comment on src/rpc/client.rs:89");
    expect((composer() as HTMLTextAreaElement).value).toContain("In `RpcConfig::default`.");
    // Said once the draft is there, aloud and visibly in the chat.
    expect(
      within(screen.getByRole("dialog", { name: "Ask" })).getByText(
        /^Opened flow 1 · step 2 to comment on its lines/,
      ),
    ).toBeVisible();
  });

  it("an answer about another step whose lines cannot be read says so plainly", async () => {
    setScenario("ask-thread");
    await open();
    await openStep("1", "5");
    await screen.findAllByText(/should_retry/);
    server.use(
      http.get("/api/v1/analyses/:id/lines", () =>
        HttpResponse.json(
          { code: "internal", message: "Something went wrong on the server." },
          { status: 500 },
        ),
      ),
    );
    await user().keyboard("a");
    await user().click(within(await thread("t2")).getByRole("button", { name: "Use as review comment" }));
    expect(
      await within(screen.getByRole("dialog", { name: "Ask" })).findByText(
        "The answer's lines could not be read: Something went wrong on the server.",
      ),
    ).toBeVisible();
    expect(screen.queryByText(/the draft is in the Comment composer/)).toBeNull();
  });

  it.each([
    ["5", "f1.5.a3", "src/rpc/client.rs:99", null, 99],
    ["7", "f1.7.a3", "src/rpc/client.rs:104–106", 104, 106],
  ])(
    "step %s: the flag %s anchors on its lines (%s); Add posts that target",
    async (step, flag, where, start, end) => {
      const posted: unknown[] = [];
      const record = ({ request }: { request: Request }) => {
        if (request.method === "POST" && request.url.endsWith("/pending-review/comments"))
          void request
            .clone()
            .json()
            .then((body) => posted.push(body));
      };
      server.events.on("request:start", record);
      try {
        await open();
        await openStep("1", step);
        const card = await waitFor(() => {
          const found = inspector().querySelector(`[data-annotation-id="${flag}"]`);
          expect(found).not.toBeNull();
          return found as HTMLElement;
        });
        await user().click(within(card).getByRole("button", { name: "Use as review comment" }));
        expect(composer()).toHaveAccessibleName(`Comment on ${where}`);
        await user().click(within(inspector()).getByRole("button", { name: "Add to GitHub pending review" }));
        await waitFor(() => expect(posted).toHaveLength(1));
        expect(posted[0]).toMatchObject({
          target: { path: "src/rpc/client.rs", side: "head", start_line: start, line: end },
        });
      } finally {
        server.events.removeListener("request:start", record);
      }
    },
  );

  it("no buttons when commenting into the pending review is not available", async () => {
    setScenario("ask-thread");
    server.use(
      http.get("/api/v1/analyses/:id/pending-review", () =>
        HttpResponse.json({ ...readStore("pendingReview"), commenting_available: false }),
      ),
    );
    await open();
    await openStep("1", "5");
    await user().keyboard("a");
    await waitFor(() => expect(chat()).toHaveTextContent("Potential double send"));
    await waitFor(() => expect(inspector()).toHaveTextContent("Potential double broadcast on a late reply"));
    expect(useButtons()).toHaveLength(0);
  });
});
