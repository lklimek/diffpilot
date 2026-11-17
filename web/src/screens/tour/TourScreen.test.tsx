import { act, cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { HttpResponse, delay, http } from "msw";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeEventSource } from "../../__tests__/fake-event-source";
import { renderRoute, startMockServer } from "../../__tests__/screen";
import { expectTrustWording } from "../../__tests__/wording";
import type { AnalysisSummary, AskQuestion, ReviewState } from "../../api/generated/types";
import { seedFor, setFixtureState } from "../../mocks/fixtures";
import { readStore } from "../../mocks/review-store";
import { setScenario } from "../../mocks/scenarios";
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
    expect(topBar()).toHaveTextContent("flows reviewed 0 / 5");
  });

  it("x ticks the current step; a flow with every step ticked counts in the top bar", async () => {
    await open("resume");
    await openStep("2", "2");
    await act(() => user().keyboard("x"));
    expect(topBar()).toHaveTextContent("flows reviewed 1 / 5");
    expect(within(nav()).getByRole("button", { name: /^Flow 2: .*, reviewed$/ })).toBeInTheDocument();
    await waitFor(() => expect(readStore("reviewState").ticked_steps).toEqual(["f2.1", "f2.2"]));
    await user().click(within(main()).getByRole("button", { name: /Mark reviewed/ }));
    expect(topBar()).toHaveTextContent("flows reviewed 0 / 5");
  });

  it("Mark flow reviewed ticks every step of the flow", async () => {
    await open();
    await user().click(within(nav()).getByRole("button", { name: /^Flow 3:/ }));
    await user().click(within(main()).getByRole("button", { name: /Mark flow reviewed/ }));
    expect(topBar()).toHaveTextContent("flows reviewed 1 / 5");
    await waitFor(() => expect(readStore("reviewState").ticked_steps).toEqual(["f3.1", "f3.2"]));
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
    expect(within(inspector()).queryByRole("group", { name: "Ask, feedback and comment" })).toBeNull();
    expect(JSON.parse(window.localStorage.getItem(LAYOUT_STORAGE_KEY)!).hidden.inspector).toBe(true);
    await user().click(within(inspector()).getByRole("button", { name: "Show Inspector column" }));
    expect(within(inspector()).getByRole("group", { name: "Ask, feedback and comment" })).toBeVisible();
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

  it("a focuses Ask; the question is pinned and its answer arrives on the event stream", async () => {
    await open();
    await openStep("1", "5");
    await user().keyboard("a");
    const field = within(inspector()).getByRole("textbox", { name: /Ask the LLM about Step 5/ });
    expect(field).toHaveFocus();
    await user().type(field, "Is the retry idempotent?{Enter}");
    const answers = await within(inspector()).findByRole("list", { name: "Questions asked about this step" });
    expect(answers).toHaveTextContent("Is the retry idempotent?");
    const asked = readStore("questions").at(-1)!;
    const answered: AskQuestion = {
      ...asked,
      state: "answered",
      answer: { answer: "It is not.", evidence: [] },
    };
    act(() => FakeEventSource.latest().emit({ event: "question", question: answered }));
    expect(await within(answers).findByText("It is not.")).toBeInTheDocument();
  });

  it("a lost answer stream is announced at the pinned answers", async () => {
    await open();
    await openStep("1", "5");
    act(() => FakeEventSource.latest().fail());
    expect(
      await within(inspector()).findByText(/The connection that delivers answers was lost/),
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
