import { render, screen, waitFor, within } from "@testing-library/react";
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
  /^Called by/,
  /^Goes next/,
  /^Classic diff/,
  /^Behaviour: was → is/,
  /^Why/,
  /^Consequences/,
  /^Questions to check/,
  /^Assessment/,
  /^GitHub threads at this location/,
];

const sectionButton = (name: RegExp) => screen.getByRole("button", { name });
const region = (name: RegExp) => screen.getByRole("region", { name });

beforeEach(() => window.localStorage.clear());
afterEach(() => vi.restoreAllMocks());

describe("Inspector at a step", () => {
  it("shows a low rating and one-sentence reason without detailed assessment fields", async () => {
    render(<Inspector tour={tour} block={atStep("f1.1")} />);
    await userEvent.click(sectionButton(/^Assessment/));
    expect(region(/^Assessment/)).toHaveTextContent("LOW");
    expect(region(/^Assessment/)).toHaveTextContent(step(tour, "f1.1").llm.assessment!.reason);
    expect(region(/^Assessment/)).not.toHaveTextContent("nothing executed");
  });

  it("shows populated sections in review order", () => {
    render(<Inspector tour={tour} block={atStep("f1.5")} />);
    const names = screen
      .getAllByRole("region")
      .map((r) => r.getAttribute("aria-label") ?? r.querySelector("h3")?.textContent ?? "");
    expect(names).toHaveLength(STEP_SECTIONS.length);
    STEP_SECTIONS.forEach((pattern, i) => expect(names[i]).toMatch(pattern));
  });

  it("names the step, links its lines on GitHub and tags LLM sections instead of a caption", () => {
    render(<Inspector tour={tour} block={atStep("f1.5")} />);
    expect(screen.getByRole("heading", { level: 2 })).toHaveTextContent("Step 5 · Retry allowed?");
    const link = screen.getByRole("link", { name: /src\/rpc\/client\.rs:99/ });
    expect(link).toHaveAttribute("href", step(tour, "f1.5").facts.github_url);
    expect(link).toHaveAttribute("target", "_blank");
    expect(link).toHaveAttribute("rel", "noopener noreferrer");
    expect(screen.queryByText(/unless marked otherwise/)).toBeNull();
    expect(within(region(/^Why/)).getByText("LLM")).toHaveAccessibleDescription(/written by the LLM/);
    expect(within(region(/^Classic diff/)).queryByText("LLM")).toBeNull();
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

  it("hides empty connections of a flow boundary step", () => {
    const t = structuredClone(tour);
    step(t, "f1.5").llm.called_by = "";
    step(t, "f1.5").llm.next = " ";
    render(<Inspector tour={t} block={atStep("f1.5")} />);
    expect(screen.queryByRole("region", { name: /^Called by/ })).toBeNull();
    expect(screen.queryByRole("region", { name: /^Goes next/ })).toBeNull();
    expect(region(/^Why/)).toBeInTheDocument();
  });

  it("states an unavailable call hierarchy in one line, without a section", () => {
    render(<Inspector tour={tour} block={atStep("f1.5")} />);
    expect(screen.queryByRole("region", { name: /Call hierarchy/ })).toBeNull();
    expect(screen.getByText("Call hierarchy: not available for this language yet.")).toBeVisible();
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

  it("hides Questions and Assessment when the LLM gave none (noq)", () => {
    const noq = seedFor("noq", "tour") as Tour;
    render(<Inspector tour={noq} block={atStep("f1.5")} />);
    expect(screen.queryByRole("region", { name: /Questions to check/ })).toBeNull();
    expect(screen.queryByRole("region", { name: /Assessment/ })).toBeNull();
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
    expect(within(threads).getByText("GitHub")).toHaveAccessibleDescription(/from GitHub/);
    expect(threads).toHaveTextContent("@bob");
    expect(threads).toHaveTextContent("Should we retry non-idempotent calls at all?");
    expect(threads).toHaveTextContent("2 comments");
    const status = within(threads).getByText("still valid");
    expect(status).toHaveAccessibleDescription(/LLM/);
    expect(status.closest("div")).toContainElement(within(threads).getByText("LLM"));
    expect(threads).toHaveTextContent("should_retry still retries every method");
    expect(within(threads).getByRole("link", { name: /Open the thread on GitHub/ })).toHaveAttribute(
      "target",
      "_blank",
    );
  });

  it("says when no GitHub thread is at the step", () => {
    render(<Inspector tour={tour} block={atStep("f1.2")} />);
    expect(screen.queryByRole("region", { name: /GitHub threads/ })).toBeNull();
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

  it("renders the dock slot", () => {
    render(<Inspector tour={tour} block={atStep("f1.5")} dock={<p>dock slot</p>} />);
    expect(screen.getByRole("group", { name: "Ask, feedback and comment" })).toHaveTextContent("dock slot");
  });
});

describe("Inspector at a selected annotation", () => {
  it("does not display empty was/is when selecting a consequence", () => {
    const updated = structuredClone(tour);
    const annotation = step(updated, "f1.5").llm.annotations.find((a) => a.id === "f1.5.a2")!;
    annotation.was = "";
    annotation.is = "";
    render(<Inspector tour={updated} block={atStep("f1.5")} annotationId={annotation.id} />);
    expect(screen.queryByRole("region", { name: /Behaviour/ })).toBeNull();
  });

  it("narrows to the annotation and offers Back to step", async () => {
    const onBack = vi.fn();
    render(<Inspector tour={tour} block={atStep("f1.5")} annotationId="f1.5.a2" onBackToStep={onBack} />);
    expect(screen.getByRole("heading", { level: 2 })).toHaveTextContent("Consequence · line 99");
    expect(region(/^Classic diff of the selected range/)).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: /Behaviour/ })).toBeNull();
    expect(region(/^Consequences/)).toHaveTextContent("may submit the same transaction twice");
    expect(screen.queryByRole("region", { name: /Questions to check/ })).toBeNull();
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
    expect(region(/^Questions to check/)).toHaveTextContent("Step 5 · Retry allowed?");
    expect(region(/^Depends on/)).toHaveTextContent(/Flow 3 ·/);
    await userEvent.click(screen.getByRole("button", { name: /Step 5 · Retry allowed\?/ }));
    expect(onSelectStep).toHaveBeenCalledWith("f1.5");
  });

  it("summarises the PR context: coverage facts apart from the LLM analysis, nothing the centre already lists", () => {
    render(<Inspector tour={tour} block={{ block: "pr_context" }} />);
    expect(screen.getByRole("heading", { level: 2 })).toHaveTextContent("PR context");
    const coverage = region(/^Coverage/);
    expect(coverage).toHaveTextContent("From the diff and the pre-pass; no LLM.");
    expect(coverage).toHaveTextContent("Unexplained: 6 changed lines in 1 file — read these yourself");
    expect(coverage).toHaveTextContent(
      "Not analysed: 8 files (Tests 4, Docs & comments 3, Mechanical & generated 1)",
    );
    expect(coverage).toHaveTextContent("Moves with identical text: 1");
    // The LLM's split is not a fact; the centre's Facts are not repeated.
    expect(coverage).not.toHaveTextContent(/flows?|steps?|CI|Project profile|removed from tests|dependenc/);
    const glance = region(/^LLM analysis at a glance/);
    expect(glance).toHaveTextContent("The LLM split this PR into 5 flows with 17 steps.");
    expect(glance).toHaveTextContent("Flows by LLM risk: 1 HIGH, 3 MEDIUM, 1 LOW");
    expect(glance).not.toHaveTextContent(/fit questions? to check/);
  });

  it("counts the unexplained ranges that were not sent to the LLM", () => {
    const t = structuredClone(tour);
    t.quiet.unexplained = [
      ...t.quiet.unexplained,
      {
        file: "src/huge.rs",
        side: "head",
        ranges: [
          { start_line: 1, end_line: 900 },
          { start_line: 950, end_line: 990 },
        ],
        reason: "too_large_for_prompt",
      },
    ];
    render(<Inspector tour={t} block={{ block: "pr_context" }} />);
    expect(region(/^Coverage/)).toHaveTextContent("2 ranges not sent to the LLM: too large");
  });

  it("states dependency files that were not checked in the Mechanical & generated summary", () => {
    const t = structuredClone(tour);
    t.pr_context.facts.counters.dependencies_changed = [];
    t.pr_context.facts.counters.dependency_files_not_checked = ["Cargo.lock"];
    render(
      <Inspector
        tour={t}
        block={{ block: "quiet", row: { row: "not_analysed", category: "generated_mechanical" } }}
      />,
    );
    expect(document.body).toHaveTextContent("Dependency files not checked: Cargo.lock");
    expect(document.body).not.toHaveTextContent(/0 dependencies/);
  });

  it.each([
    ["partial", "f1.5", "Review threads: the list from GitHub is incomplete"],
    ["not_checked", "f2.2", "Review threads: not checked"],
  ] as const)("the threads section never reads as complete when the list is %s", (status, stepId, text) => {
    const t = structuredClone(tour);
    t.pr_context.facts.threads_status = status;
    if (status === "not_checked") t.threads = [];
    render(<Inspector tour={t} block={{ block: "step", step_id: stepId }} />);
    const threads = region(/^GitHub threads at this location/);
    expect(threads).toHaveTextContent(text);
    expect(threads).not.toHaveTextContent("No GitHub review threads on these lines.");
  });

  it("says when GitHub lists more pending comments than were read", () => {
    const review: PendingReview = { commenting_available: true, comments: [], incomplete: true };
    render(<Inspector tour={tour} block={{ block: "pending_review" }} pendingReview={review} />);
    expect(region(/^Pending review/)).toHaveTextContent(
      "More pending comments exist on GitHub than are listed here.",
    );
  });

  it("counts flows the LLM did not rate as not rated", () => {
    const t = structuredClone(tour);
    t.flows[0]!.llm.risk = null;
    render(<Inspector tour={t} block={{ block: "pr_context" }} />);
    expect(region(/^LLM analysis at a glance/)).toHaveTextContent(
      "Flows by LLM risk: 3 MEDIUM, 1 LOW, 1 not rated",
    );
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
    // Deterministic panels carry no LLM provenance tag.
    expect(document.querySelector('[data-provenance="llm"]')).toBeNull();
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
    expect(document.body).toHaveTextContent("Commenting is not available for this repository right now.");
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
    expect(region(/^Classic diff/)).toHaveAttribute("data-tone", "facts");
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

describe("Inspector: Show in Inspector from a code marker", () => {
  it("opens a collapsed section, scrolls to the remark and highlights it briefly", async () => {
    const { rerender } = render(<Inspector tour={tour} block={atStep("f1.5")} />);
    expect(sectionButton(/^Questions to check/)).toHaveAttribute("aria-expanded", "false");
    rerender(
      <Inspector
        tour={tour}
        block={atStep("f1.5")}
        flash={{ kind: "annotation", id: "f1.5.a3", nonce: 1 }}
      />,
    );
    expect(sectionButton(/^Questions to check/)).toHaveAttribute("aria-expanded", "true");
    const item = region(/^Questions to check/).querySelector('[data-annotation-id="f1.5.a3"]');
    expect(item).not.toBeNull();
    await waitFor(() => expect(item).toHaveAttribute("data-flash"));
  });

  it("highlights a GitHub thread of the step", async () => {
    render(
      <Inspector
        tour={tour}
        block={atStep("f1.5")}
        flash={{ kind: "thread", id: "PRRT_fixture482_t1", nonce: 1 }}
      />,
    );
    const item = region(/^GitHub threads/).querySelector('[data-thread-id="PRRT_fixture482_t1"]');
    expect(item).not.toBeNull();
    await waitFor(() => expect(item).toHaveAttribute("data-flash"));
  });
});
