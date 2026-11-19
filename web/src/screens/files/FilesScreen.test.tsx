import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { HttpResponse, http } from "msw";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeEventSource } from "../../__tests__/fake-event-source";
import { renderRoute, startMockServer } from "../../__tests__/screen";
import { expectTrustWording } from "../../__tests__/wording";
import type { AnalysisSummary, AnchoredAskRequest, FilesResponse } from "../../api/generated/types";
import seed from "../../mocks/files-482.json";
import { seedFor } from "../../mocks/fixtures";
import { MOVED_HEAD, setScenario } from "../../mocks/scenarios";

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
const inspector = () => screen.getByRole("complementary", { name: "Inspector" });
const tree = () => screen.getByRole("navigation", { name: "Files" });
const inspectorHeading = () =>
  within(inspector())
    .getAllByRole("heading", { level: 2 })
    .find((h) => !h.hasAttribute("data-column-title"))!;
const fileHeading = () =>
  within(screen.getByRole("main"))
    .getAllByRole("heading", { level: 2 })
    .find((h) => !h.hasAttribute("data-column-title"))!;
const hashOf = (router: ReturnType<typeof renderRoute>) => decodeURIComponent(router.state.location.hash);

async function open(hash = "") {
  const router = renderRoute(`/analyses/${ID}/files${hash}`);
  await waitFor(() => expect(fileHeading()).toBeDefined());
  return router;
}

describe("Files view (brief §4.5)", () => {
  it("opens the first file in tree order with the file summary, and writes the file into the URL", async () => {
    const router = await open();
    expect(fileHeading()).toHaveTextContent("src/config.rs");
    expect(within(tree()).getByRole("button", { name: /config\.rs/ })).toHaveAttribute(
      "aria-current",
      "page",
    );
    expect(inspectorHeading()).toHaveTextContent("src/config.rs");
    expect(within(inspector()).getByRole("region", { name: /Flows in this file \(1\)/ })).toHaveTextContent(
      "0 of 1 ticked in Flows",
    );
    await waitFor(() => expect(hashOf(router)).toBe("#lines?file=src/config.rs&side=head"));
    expect(screen.getAllByRole("banner")[0]).toHaveTextContent("Your progress");
    expect(screen.getByRole("button", { name: "View: Files" })).toBeVisible();
    expect(inspector()).toHaveTextContent("Select lines by their numbers in the code");
    expectTrustWording(document.body.textContent ?? "");
  });

  it("a permalink to lines opens them as the reader's selection; Ask is anchored to them and their one step", async () => {
    let asked: AnchoredAskRequest | null = null;
    server.use(
      http.post("/api/v1/analyses/:id/questions", async ({ request }) => {
        asked = (await request.json()) as AnchoredAskRequest;
        return HttpResponse.json(
          {
            id: "q-new",
            anchor: asked.anchor,
            step_id: "f1.6",
            text: asked.text,
            state: "pending",
            mine: true,
            answer: null,
            failure: null,
            created_at: new Date().toISOString(),
          },
          { status: 202 },
        );
      }),
    );
    await open("#lines?file=src%2Frpc%2Fclient.rs&side=head&line=100&end=102");
    expect(inspectorHeading()).toHaveTextContent("Your selection · client.rs lines 100–102");
    expect(within(inspector()).getByRole("group", { name: /^Flow 1/ })).toHaveTextContent("Step 6");
    await waitFor(() => expect(document.querySelectorAll("[data-line][data-selected-line]")).toHaveLength(3));
    await user().click(within(inspector()).getByRole("button", { name: "Ask about these lines" }));
    const field = within(inspector()).getByRole("textbox", { name: /Ask the LLM about/ });
    await waitFor(() => expect(field).toHaveFocus());
    expect(field).toHaveAttribute("placeholder", "Ask about these lines…");
    await user().type(field, "Why sleep here?{Enter}");
    await waitFor(() => expect(asked).not.toBeNull());
    expect(asked!.anchor).toEqual({
      kind: "step",
      step_id: "f1.6",
      lines: { start_line: 100, end_line: 102 },
    });
  });

  it("lines over several steps are asked about as lines, without a step (A4)", async () => {
    let asked: AnchoredAskRequest | null = null;
    server.use(
      http.post("/api/v1/analyses/:id/questions", async ({ request }) => {
        asked = (await request.json()) as AnchoredAskRequest;
        return HttpResponse.json({}, { status: 500 });
      }),
    );
    await open("#lines?file=src%2Frpc%2Fclient.rs&side=head&line=98&end=101");
    await user().click(within(inspector()).getByRole("button", { name: "Ask about these lines" }));
    await user().type(within(inspector()).getByRole("textbox", { name: /Ask the LLM about/ }), "Q?{Enter}");
    await waitFor(() => expect(asked).not.toBeNull());
    expect(asked!.anchor).toEqual({
      kind: "lines",
      path: "src/rpc/client.rs",
      side: "head",
      range: { start_line: 98, end_line: 101 },
    });
  });

  it("a hunk lists its flows and steps; Details narrows to a step; Esc goes back to the hunk, then the file", async () => {
    const router = await open("#lines?file=src%2Frpc%2Fclient.rs&side=head&line=96&end=109");
    expect(inspectorHeading()).toHaveTextContent("client.rs · hunk, lines 96–109 (+ 2 removed)");
    const flow = within(inspector()).getByRole("group", { name: /^Flow 1/ });
    expect(within(flow).getAllByRole("listitem")).toHaveLength(7);
    await user().click(within(flow).getByRole("button", { name: /^Details: Step 5/ }));
    expect(inspectorHeading()).toHaveTextContent("Step 5 of flow 1 · Retry allowed?");
    expect(within(inspector()).getByRole("button", { name: "← Back to hunk" })).toBeVisible();
    expect(hashOf(router)).toContain("step=f1.5");
    await user().keyboard("{Escape}");
    expect(inspectorHeading()).toHaveTextContent("client.rs · hunk");
    await user().keyboard("{Escape}");
    expect(inspectorHeading()).toHaveTextContent("src/rpc/client.rs");
    expect(hashOf(router)).toBe("#lines?file=src/rpc/client.rs&side=head");
  });

  it("clicking a line number selects the line; Shift+click extends; the URL holds the range", async () => {
    const router = await open("#lines?file=src%2Frpc%2Fclient.rs&side=head");
    const number = async (n: number) =>
      within(screen.getByRole("main")).findByRole("button", { name: `Select line ${n}` });
    await user().click(await number(100));
    expect(inspectorHeading()).toHaveTextContent("Your selection · client.rs line 100");
    const u = user();
    await u.keyboard("{Shift>}");
    await u.click(await number(102));
    await u.keyboard("{/Shift}");
    expect(inspectorHeading()).toHaveTextContent("Your selection · client.rs lines 100–102");
    expect(hashOf(router)).toBe("#lines?file=src/rpc/client.rs&side=head&line=100&end=102");
    // The comment composer targets the selection's last line, without taking the focus.
    expect(
      within(inspector()).getByRole("textbox", { name: "Comment on src/rpc/client.rs:102" }),
    ).not.toHaveFocus();
  });

  it("v marks the file Viewed on GitHub after confirmation, then opens the next file with focus on its header", async () => {
    await open("#lines?file=src%2Flib.rs&side=head");
    await user().keyboard("v");
    expect(screen.getByRole("button", { name: "Viewed" })).toHaveTextContent("Saving to GitHub…");
    await waitFor(() => expect(fileHeading()).toHaveTextContent("src/rpc/client.rs"));
    expect(fileHeading()).toHaveFocus();
    expect(
      screen.getByText("Marked lib.rs as viewed on GitHub. Opened the next file, client.rs."),
    ).toHaveAttribute("role", "status");
    expect(within(tree()).getByRole("button", { name: /lib\.rs/ })).toHaveTextContent("viewed on GitHub");
  });

  it("a refusal by GitHub keeps the mark and the file, and says so with Retry", async () => {
    setScenario("viewed-refused");
    await open("#lines?file=src%2Flib.rs&side=head");
    await user().click(screen.getByRole("button", { name: "Viewed" }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(/^Not saved on GitHub\. GitHub refused/);
    expect(fileHeading()).toHaveTextContent("src/lib.rs");
    expect(screen.getByRole("button", { name: "Viewed" })).toHaveAttribute("aria-pressed", "false");
  });

  it("the toggle is disabled with the reason when the PR head moved", async () => {
    setScenario("viewed-head-moved");
    await open("#lines?file=src%2Flib.rs&side=head");
    const toggle = screen.getByRole("button", { name: "Viewed" });
    expect(toggle).toHaveAttribute("aria-disabled", "true");
    expect(toggle).toHaveAccessibleDescription(/the PR is now at d41b7a0/);
  });

  it("pr_head_changed on save re-reads the file list, so the toggle explains that the PR moved on", async () => {
    let moved = false;
    server.use(
      http.put("/api/v1/analyses/:id/files/viewed", () => {
        moved = true;
        return HttpResponse.json(
          { code: "pr_head_changed", message: "The pull request has new commits." },
          { status: 409 },
        );
      }),
      http.get("/api/v1/analyses/:id/files", () => {
        const files = structuredClone(seed) as FilesResponse;
        if (moved) files.current_head = MOVED_HEAD;
        return HttpResponse.json(files);
      }),
    );
    await open("#lines?file=src%2Flib.rs&side=head");
    await user().click(screen.getByRole("button", { name: "Viewed" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("The pull request has new commits.");
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Viewed" })).toHaveAttribute("aria-disabled", "true"),
    );
  });

  it("a link to a file outside the PR says so and opens the first file", async () => {
    await open("#lines?file=src%2Fnope.rs&side=head");
    expect(
      screen.getByText("This link names src/nope.rs, which is not a file of this pull request."),
    ).toHaveAttribute("role", "status");
    expect(fileHeading()).toHaveTextContent("src/config.rs");
  });

  it("s switches to the Flows view at the narrowed step", async () => {
    const router = await open("#lines?file=src%2Frpc%2Fclient.rs&side=head&line=99&step=f1.5");
    await user().keyboard("s");
    await waitFor(() => expect(router.state.location.pathname).toBe(`/analyses/${ID}`));
    expect(router.state.location.hash).toBe("#f1.5");
  });
});

describe("Files Inspector panels (proposal §3)", () => {
  it("a hunk with one step shows that step's sections and flags, without Details or the per-step hint", async () => {
    await open("#lines?file=src%2Fconfig.rs&side=head&line=23");
    expect(inspectorHeading()).toHaveTextContent("config.rs · hunk, line 23");
    const flags = within(inspector()).getByRole("region", { name: /^Flags \(1\)/ });
    expect(flags.querySelector("[data-flag=red]")).not.toBeNull();
    expect(within(inspector()).getByRole("region", { name: /^Behaviour/ })).toBeDefined();
    expect(within(inspector()).queryByRole("button", { name: /^Details:/ })).toBeNull();
    expect(within(inspector()).queryByRole("region", { name: /^Called by/ })).toBeNull();
    expect(inspector()).not.toHaveTextContent("pick Details on a step");
  });

  it("a hunk with several steps lists every step's flags, each naming its step, and the per-step hint", async () => {
    await open("#lines?file=src%2Frpc%2Fclient.rs&side=head&line=96&end=109");
    const flags = within(inspector()).getByRole("region", { name: /^Flags \(3\)/ });
    expect(flags).toHaveTextContent("step 5 of flow 1");
    expect(flags).toHaveTextContent("step 6 of flow 1");
    expect(flags).toHaveTextContent("step 7 of flow 1");
    expect(within(inspector()).queryByRole("region", { name: /^Behaviour/ })).toBeNull();
    expect(inspector()).toHaveTextContent("pick Details on a step");
  });

  it("a step narrowed from a hunk keeps its flow, tick and the lines it explains here, and marks them", async () => {
    await open("#lines?file=src%2Frpc%2Fclient.rs&side=head&line=99&step=f1.5");
    expect(inspectorHeading()).toHaveTextContent("Step 5 of flow 1 · Retry allowed?");
    expect(inspector()).toHaveTextContent("Explains in this hunk: line 99");
    const flow = within(inspector()).getByRole("group", { name: /^Flow 1/ });
    expect(within(flow).getAllByRole("listitem")).toHaveLength(1);
    expect(flow).toHaveTextContent("☐ not ticked in Flows");
    expect(within(flow).getByRole("button", { name: /^Open in Flows: Step 5/ })).toBeDefined();
    expect(within(inspector()).getByRole("region", { name: /^Classic diff of these lines/ })).toBeDefined();
    expect(within(inspector()).queryByRole("region", { name: /^Called by/ })).toBeNull();
    await waitFor(() =>
      expect(
        [...document.querySelectorAll("[data-line][data-selected-line]")].map((r) =>
          r.getAttribute("data-line"),
        ),
      ).toEqual(["99"]),
    );
  });

  it("the file summary is compact: per flow its steps here with ticks, a counter, and the hunks", async () => {
    await open("#lines?file=src%2Frpc%2Fclient.rs&side=head");
    const flows = within(inspector()).getByRole("region", { name: /^Flows in this file \(2\)/ });
    expect(flows).toHaveTextContent("Steps here:");
    expect(flows).toHaveTextContent("0 of 8 ticked in Flows");
    expect(within(flows).queryByRole("button", { name: /^Details:/ })).toBeNull();
    expect(flows).not.toHaveTextContent("Where these lines are explained");
    expect(within(inspector()).getByRole("region", { name: /^Hunks in this file/ })).toBeDefined();
  });

  it("a hunk in no flow titles its section without a count and gives no per-step hint", async () => {
    await open("#lines?file=src%2Frpc%2Fmetrics.rs&side=head&line=40&end=46");
    expect(within(inspector()).getByRole("region", { name: /^Flows$/ })).toBeDefined();
    expect(inspector()).not.toHaveTextContent("Flows (0)");
    expect(inspector()).not.toHaveTextContent("pick Details on a step");
    expect(inspector()).not.toHaveTextContent("the code view shows these lines as unchanged");
  });

  it("a file recognised by its path carries the classified-by-path badge in its header and the Inspector", async () => {
    await open("#lines?file=tests%2Frpc_retry.rs&side=head");
    expect(fileHeading().closest("header")).toHaveTextContent(/classified by path/i);
    expect(inspector()).toHaveTextContent(/classified by path/i);
  });
});
