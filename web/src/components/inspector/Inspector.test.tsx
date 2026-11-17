import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { BlockRef, PendingReview, Step, Tour } from "../../api/generated/types";
import { expectTrustWording, toursByState } from "../../__tests__/wording";
import { seedFor } from "../../mocks/fixtures";
import { Inspector } from "./Inspector";
import { SECTION_STORAGE_KEY } from "./sectionState";

const tour = seedFor("default", "tour") as Tour;
const step = (t: Tour, id: string): Step => t.flows.flatMap((f) => f.steps).find((s) => s.id === id)!;
const atStep = (id: string): BlockRef => ({ block: "step", step_id: id });

const STEP_SECTIONS = [
  /^Classic diff/,
  /^Behaviour: was → is/,
  /^Why/,
  /^Consequences/,
  /^Call hierarchy/,
  /^Questions to check/,
  /^Assessment/,
  /^GitHub threads at this location/,
];

const sectionButton = (name: RegExp) => screen.getByRole("button", { name });
const region = (name: RegExp) => screen.getByRole("region", { name });

beforeEach(() => window.localStorage.clear());
afterEach(() => vi.restoreAllMocks());

describe("Inspector at a step", () => {
  it("shows the eight sections in the order of the brief", () => {
    render(<Inspector tour={tour} block={atStep("f1.5")} />);
    const names = screen
      .getAllByRole("region")
      .map((r) => r.getAttribute("aria-label") ?? r.querySelector("h3")?.textContent ?? "");
    expect(names).toHaveLength(8);
    STEP_SECTIONS.forEach((pattern, i) => expect(names[i]).toMatch(pattern));
  });

  it("names the step, links its lines on GitHub and says once that it is LLM analysis", () => {
    render(<Inspector tour={tour} block={atStep("f1.5")} />);
    expect(screen.getByRole("heading", { level: 2 })).toHaveTextContent("Step 5 · Retry allowed?");
    const link = screen.getByRole("link", { name: /src\/rpc\/client\.rs:99/ });
    expect(link).toHaveAttribute("href", step(tour, "f1.5").facts.github_url);
    expect(link).toHaveAttribute("target", "_blank");
    expect(link).toHaveAttribute("rel", "noopener noreferrer");
    expect(screen.getAllByText("LLM analysis unless marked otherwise.")).toHaveLength(1);
  });

  it("keeps Questions and Assessment collapsed and the others open by default", () => {
    render(<Inspector tour={tour} block={atStep("f1.5")} />);
    for (const name of STEP_SECTIONS) {
      const collapsed = /Questions|Assessment/.test(name.source);
      expect(sectionButton(name)).toHaveAttribute("aria-expanded", String(!collapsed));
    }
    expect(screen.getByText(/wallet user could see one payment broadcast twice/)).not.toBeVisible();
  });

  it("puts the classic diff slot in section 1 and says when it is missing", () => {
    const { rerender } = render(<Inspector tour={tour} block={atStep("f1.5")} diff={<p>diff slot</p>} />);
    expect(within(region(/^Classic diff of this step/)).getByText("diff slot")).toBeVisible();
    rerender(<Inspector tour={tour} block={atStep("f1.5")} />);
    expect(region(/^Classic diff/)).toHaveTextContent("The classic diff is not loaded.");
  });

  it("shows behaviour as was → is with its basis and evidence as plain text", () => {
    render(<Inspector tour={tour} block={atStep("f1.5")} />);
    const behaviour = region(/^Behaviour/);
    expect(behaviour).toHaveTextContent("No retry decision; the timeout went straight to the caller.");
    expect(behaviour).toHaveTextContent("Retries while attempt < max_attempts (3).");
    expect(behaviour).toHaveTextContent("fact");
    expect(behaviour).toHaveTextContent("src/rpc/retry.rs:23");
    expect(within(behaviour).queryByRole("link")).toBeNull();
  });

  it("shows why and the consequences, marking inferences", () => {
    render(<Inspector tour={tour} block={atStep("f1.5")} />);
    expect(region(/^Why/)).toHaveTextContent("The loop needs a rule for when to stop");
    const consequences = region(/^Consequences/);
    expect(consequences).toHaveTextContent("may submit the same transaction twice");
    expect(consequences).toHaveTextContent("inference");
  });

  it("reads n/a for the call hierarchy until a language plugin exists", () => {
    render(<Inspector tour={tour} block={atStep("f1.5")} />);
    const calls = region(/^Call hierarchy/);
    expect(calls).toHaveTextContent("n/a — needs a language plugin");
    expect(within(calls).getByText("n/a — needs a language plugin")).toHaveAccessibleDescription(
      /language plugin/,
    );
  });

  it("lists callers and callees with GitHub links when the plugin returns them", () => {
    const t = structuredClone(tour);
    step(t, "f1.5").facts.call_hierarchy = {
      callers: [
        {
          symbol: "pay",
          file: "src/wallet/pay.rs",
          line: 57,
          github_url: "https://github.com/a/b/blob/x#L57",
        },
      ],
      callees: [{ symbol: "evil", file: "src/x.rs", line: 1, github_url: "javascript:alert(1)" }],
    };
    render(<Inspector tour={t} block={atStep("f1.5")} />);
    const calls = region(/^Call hierarchy/);
    expect(within(calls).getByRole("link", { name: /pay/ })).toHaveAttribute(
      "href",
      "https://github.com/a/b/blob/x#L57",
    );
    expect(within(calls).getAllByRole("link")).toHaveLength(1);
    expect(calls).toHaveTextContent("evil");
  });

  it("moves the LLM questions here, phrased as questions, with a count in the header", async () => {
    render(<Inspector tour={tour} block={atStep("f1.5")} />);
    const button = sectionButton(/^Questions to check/);
    expect(button).toHaveTextContent("Questions to check (1)");
    await userEvent.click(button);
    expect(region(/^Questions to check/)).toHaveTextContent(
      "can broadcast_tx be sent twice when the reply arrives late?",
    );
  });

  it("says that no questions is not evidence of no defects (noq)", () => {
    const noq = seedFor("noq", "tour") as Tour;
    render(<Inspector tour={noq} block={atStep("f1.5")} />);
    expect(region(/^Questions to check/)).toHaveTextContent(
      "The LLM raised no questions. That is not evidence of no defects.",
    );
    expect(region(/^Assessment/)).toHaveTextContent("The LLM gave no assessment for this step.");
  });

  it("opens the assessment only on request, after the evidence sections", async () => {
    render(<Inspector tour={tour} block={atStep("f1.5")} />);
    const assessment = region(/^Assessment/);
    await userEvent.click(sectionButton(/^Assessment/));
    expect(within(assessment).getByText(/wallet user could see one payment broadcast twice/)).toBeVisible();
    expect(within(assessment).getByText("▲ HIGH")).toHaveAccessibleDescription(/LLM rating/);
    expect(assessment).toHaveTextContent("node-side deduplication");
    expect(assessment).toHaveTextContent("blocks this PR");
  });

  it("lists the GitHub threads of the step with their LLM status as LLM analysis", () => {
    render(<Inspector tour={tour} block={atStep("f1.5")} />);
    const threads = region(/^GitHub threads/);
    expect(threads).toHaveTextContent("@bob");
    expect(threads).toHaveTextContent("Should we retry non-idempotent calls at all?");
    expect(threads).toHaveTextContent("2 comments");
    const status = within(threads).getByText("still valid");
    expect(status).toHaveAccessibleDescription(/LLM/);
    expect(threads).toHaveTextContent("should_retry still retries every method");
    expect(within(threads).getByRole("link", { name: /Open the thread on GitHub/ })).toHaveAttribute(
      "target",
      "_blank",
    );
  });

  it("says when no GitHub thread is at the step", () => {
    render(<Inspector tour={tour} block={atStep("f1.2")} />);
    expect(region(/^GitHub threads/)).toHaveTextContent("No GitHub review threads on these lines.");
  });

  it("toggles a section from the keyboard", async () => {
    const user = userEvent.setup();
    render(<Inspector tour={tour} block={atStep("f1.5")} />);
    const why = sectionButton(/^Why/);
    why.focus();
    await user.keyboard("{Enter}");
    expect(why).toHaveAttribute("aria-expanded", "false");
    expect(screen.getByText(/The loop needs a rule/)).not.toBeVisible();
    await user.keyboard(" ");
    expect(why).toHaveAttribute("aria-expanded", "true");
  });

  it("renders the pinned answers and the dock slots", () => {
    render(<Inspector tour={tour} block={atStep("f1.5")} answers={<p>pinned</p>} dock={<p>dock slot</p>} />);
    expect(within(region(/^Ask: pinned answers/)).getByText("pinned")).toBeVisible();
    expect(screen.getByRole("group", { name: "Ask, feedback and comment" })).toHaveTextContent("dock slot");
  });
});

describe("Inspector at a selected annotation", () => {
  it("narrows to the annotation and offers Back to step", async () => {
    const onBack = vi.fn();
    render(<Inspector tour={tour} block={atStep("f1.5")} annotationId="f1.5.a2" onBackToStep={onBack} />);
    expect(screen.getByRole("heading", { level: 2 })).toHaveTextContent("Consequence · line 99");
    expect(region(/^Classic diff of the selected range/)).toBeInTheDocument();
    expect(region(/^Behaviour/)).toHaveTextContent("A timeout was returned to the caller.");
    expect(region(/^Behaviour/)).toHaveTextContent("Line 99 (inference; src/mempool/mod.rs:129)");
    expect(region(/^Behaviour/)).not.toHaveTextContent("No retry decision");
    expect(region(/^Consequences/)).toHaveTextContent("may submit the same transaction twice");
    expect(region(/^Questions to check/)).toHaveTextContent("The LLM raised no questions");
    await userEvent.click(screen.getByRole("button", { name: /Back to step/ }));
    expect(onBack).toHaveBeenCalledOnce();
  });

  it("keeps only the threads on the annotated lines", () => {
    render(<Inspector tour={tour} block={atStep("f1.5")} annotationId="f1.5.a3" />);
    expect(region(/^GitHub threads/)).toHaveTextContent("@bob");
  });

  it("moves focus to the step heading after Back to step", async () => {
    const user = userEvent.setup();
    const { rerender } = render(
      <Inspector tour={tour} block={atStep("f1.5")} annotationId="f1.5.a1" onBackToStep={() => {}} />,
    );
    await user.click(screen.getByRole("button", { name: /Back to step/ }));
    rerender(<Inspector tour={tour} block={atStep("f1.5")} annotationId={null} onBackToStep={() => {}} />);
    const heading = screen.getByRole("heading", { level: 2 });
    expect(heading).toHaveTextContent("Step 5 · Retry allowed?");
    expect(heading).toHaveFocus();
  });

  it("falls back to the step when the annotation is not in it (never empty)", () => {
    render(<Inspector tour={tour} block={atStep("f1.5")} annotationId="f9.9.a9" />);
    expect(screen.getByRole("heading", { level: 2 })).toHaveTextContent("Step 5 · Retry allowed?");
    expect(screen.queryByRole("button", { name: /Back to step/ })).toBeNull();
  });
});

describe("Inspector summaries", () => {
  it("summarises a flow introduction and opens its steps", async () => {
    const onSelectStep = vi.fn();
    render(
      <Inspector tour={tour} block={{ block: "flow_intro", flow_id: "f1" }} onSelectStep={onSelectStep} />,
    );
    expect(screen.getByRole("heading", { level: 2 })).toHaveTextContent(
      "Flow 1 · Send an RPC request with retry",
    );
    expect(region(/^Files touched/)).toHaveTextContent("src/rpc/retry.rs");
    expect(region(/^Questions to check/)).toHaveTextContent(/\d+ questions across the steps of this flow/);
    expect(region(/^Depends on/)).toHaveTextContent(/Flow 3 ·/);
    await userEvent.click(screen.getByRole("button", { name: /Step 5 · Retry allowed\?/ }));
    expect(onSelectStep).toHaveBeenCalledWith("f1.5");
  });

  it("summarises the PR context, facts apart from LLM analysis", () => {
    render(<Inspector tour={tour} block={{ block: "pr_context" }} />);
    expect(screen.getByRole("heading", { level: 2 })).toHaveTextContent("PR context");
    const facts = region(/^Facts/);
    expect(facts).toHaveTextContent("5 flows, 17 steps");
    expect(facts).toHaveTextContent("Project profile: repository file");
    expect(region(/^LLM analysis at a glance/)).toHaveTextContent(/fit questions? to check/);
  });

  it("says when there is no project profile (noprofile)", () => {
    const noprofile = seedFor("noprofile", "tour") as Tour;
    render(<Inspector tour={noprofile} block={{ block: "pr_context" }} />);
    expect(region(/^Facts/)).toHaveTextContent("No project profile — ratings are component-level");
  });

  it("says related work was not checked without the GitHub App", () => {
    const t = structuredClone(tour);
    t.pr_context.facts.related_work_checked = false;
    render(<Inspector tour={t} block={{ block: "pr_context" }} />);
    expect(region(/^LLM analysis at a glance/)).toHaveTextContent("Related work: not checked");
  });

  it.each([
    [{ row: "unexplained" }, "Unexplained — read these yourself", "src/rpc/metrics.rs: lines 41–46"],
    [{ row: "moves" }, "Moves with identical text", "text identical"],
    [{ row: "not_analysed", category: "tests" }, "Not analysed: Tests", "4 files"],
  ] as const)("summarises the quiet row %o", (row, title, text) => {
    render(<Inspector tour={tour} block={{ block: "quiet", row }} />);
    expect(screen.getByRole("heading", { level: 2 })).toHaveTextContent(title);
    expect(document.body).toHaveTextContent(text);
    expect(document.body).not.toHaveTextContent("LLM analysis unless marked otherwise.");
  });

  it("summarises the pending review from GitHub", () => {
    const review = seedFor("missed", "pending-review") as PendingReview;
    const { rerender } = render(
      <Inspector tour={tour} block={{ block: "pending_review" }} pendingReview={review} />,
    );
    expect(document.body).toHaveTextContent("1 comment in your GitHub pending review");
    rerender(
      <Inspector
        tour={tour}
        block={{ block: "pending_review" }}
        pendingReview={{ commenting_available: false, comments: [], incomplete: false }}
      />,
    );
    expect(document.body).toHaveTextContent("GitHub App is not installed");
  });

  it("is never empty, also for a block outside the analysis", () => {
    render(<Inspector tour={tour} block={{ block: "flow_intro", flow_id: "f99" }} />);
    expect(screen.getByRole("heading", { level: 2 })).toBeInTheDocument();
    expect(document.body).toHaveTextContent("This block is not part of the analysis.");
  });
});

describe("Inspector section memory", () => {
  it("remembers a collapsed section across blocks and remounts", async () => {
    const { rerender, unmount } = render(<Inspector tour={tour} block={atStep("f1.5")} />);
    await userEvent.click(sectionButton(/^Why/));
    rerender(<Inspector tour={tour} block={atStep("f1.6")} />);
    expect(sectionButton(/^Why/)).toHaveAttribute("aria-expanded", "false");
    unmount();
    expect(JSON.parse(window.localStorage.getItem(SECTION_STORAGE_KEY)!)).toMatchObject({ why: false });
    render(<Inspector tour={tour} block={atStep("f1.2")} />);
    expect(sectionButton(/^Why/)).toHaveAttribute("aria-expanded", "false");
  });

  it("still collapses and remembers sections in memory when storage fails", async () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new DOMException("denied", "SecurityError");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new DOMException("denied", "SecurityError");
    });
    const { rerender } = render(<Inspector tour={tour} block={atStep("f1.5")} />);
    await userEvent.click(sectionButton(/^Why/));
    rerender(<Inspector tour={tour} block={atStep("f1.6")} />);
    expect(sectionButton(/^Why/)).toHaveAttribute("aria-expanded", "false");
  });
});

describe("Inspector trust rules", () => {
  it("renders LLM and GitHub text as text only, never as HTML or links (T12)", () => {
    const t = structuredClone(tour);
    const s = step(t, "f1.5");
    const evil = '<img src=x onerror="alert(1)"> javascript:alert(1) https://evil.example';
    s.llm.why = evil;
    s.llm.annotations.forEach((a) => {
      a.was = evil;
      a.is = evil;
      a.text = a.text === null ? null : evil;
    });
    s.llm.assessment!.headline = evil;
    s.facts.github_url = "javascript:alert(1)";
    t.threads[0]!.excerpt = evil;
    t.threads[0]!.llm_reason = evil;
    t.threads[0]!.url = "javascript:alert(2)";
    const { container } = render(<Inspector tour={t} block={atStep("f1.5")} />);
    expect(container.querySelector("img")).toBeNull();
    for (const a of container.querySelectorAll("a")) {
      expect(a.getAttribute("href")).toMatch(/^https:\/\/github\.com\//);
    }
    expect(container).toHaveTextContent('<img src=x onerror="alert(1)">');
  });

  it("separates LLM sections from fact sections visually", () => {
    render(<Inspector tour={tour} block={atStep("f1.5")} />);
    expect(region(/^Why/)).toHaveAttribute("data-tone", "llm");
    expect(region(/^Classic diff/)).toHaveAttribute("data-tone", "facts");
    expect(region(/^Call hierarchy/)).toHaveAttribute("data-tone", "facts");
  });

  it.each(toursByState)("keeps the wording rules and is never empty on every block in state %s", (_, t) => {
    const blocks: [BlockRef, string | null][] = [
      [{ block: "pr_context" }, null],
      [{ block: "pending_review" }, null],
      [{ block: "quiet", row: { row: "unexplained" } }, null],
      [{ block: "quiet", row: { row: "moves" } }, null],
      ...(["tests", "docs_comments", "generated_mechanical"] as const).map((category): [BlockRef, null] => [
        { block: "quiet", row: { row: "not_analysed", category } },
        null,
      ]),
      ...t.flows.map((f): [BlockRef, null] => [{ block: "flow_intro", flow_id: f.id }, null]),
      ...t.flows
        .flatMap((f) => f.steps)
        .flatMap((s): [BlockRef, string | null][] => [
          [atStep(s.id), null],
          ...s.llm.annotations.map((a): [BlockRef, string] => [atStep(s.id), a.id]),
        ]),
    ];
    for (const [block, annotationId] of blocks) {
      const { container, unmount } = render(
        <Inspector tour={t} block={block} annotationId={annotationId} onBackToStep={() => {}} />,
      );
      expect(screen.getByRole("heading", { level: 2 }).textContent).not.toBe("");
      expectTrustWording(container.textContent ?? "");
      unmount();
    }
  });
});
