import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { Tour } from "../../api/generated/types";
import { expectTrustWording } from "../../__tests__/wording";
import { FIXTURE_STATES, seedFor } from "../../mocks/fixtures";
import { PrContextBlock, type PrContextView } from "./PrContextBlock";

const tour = seedFor("default", "tour") as Tour;
const renderBlock = (t: Tour, context: PrContextView = t.pr_context) =>
  render(<PrContextBlock pr={t.analysis.pr} context={context} threadCount={t.threads.length} />);

describe("PrContextBlock", () => {
  it("uses singular counts and puts Problem before the benefit", () => {
    const context = structuredClone(tour.pr_context);
    context.facts.flow_count = 1;
    context.facts.step_count = 1;
    renderBlock(tour, context);
    const text = screen.getByRole("region", { name: "LLM analysis" }).textContent!;
    expect(text).toContain("1 flow with 1 step.");
    expect(text.indexOf("Problem:")).toBeLessThan(text.indexOf("What the change gives:"));
  });

  it("separates the LLM analysis from the deterministic facts", () => {
    renderBlock(tour);
    const llm = screen.getByRole("region", { name: "LLM analysis" });
    const facts = screen.getByRole("region", { name: "Facts" });
    expect(within(llm).getByText("LLM")).toHaveAccessibleDescription(/written by the LLM/);
    expect(llm).not.toHaveTextContent(/unless marked otherwise/);
    expect(within(facts).queryByText(/LLM analysis/)).not.toBeInTheDocument();
    // LLM content
    expect(llm).toHaveTextContent(tour.pr_context.llm.problem);
    expect(llm).toHaveTextContent(tour.pr_context.llm.what_it_gives);
    expect(llm).toHaveTextContent(tour.pr_context.llm.why_needed);
    expect(within(llm).getByText("acme/node-client#479")).toBeInTheDocument();
    expect(within(llm).getAllByRole("listitem").length).toBeGreaterThanOrEqual(6);
    expect(llm).toHaveTextContent("src/config.rs:23");
    // Facts
    expect(facts).toHaveTextContent(
      "Check runs: 3 passed, 1 failed, 1 skipped, 1 neutral. Statuses: 1 pending.",
    );
    expect(facts).toHaveTextContent("sync_headers::slow_peer_2000_headers");
    expect(facts).toHaveTextContent("12 test lines changed or removed, 30 added");
    expect(facts).toHaveTextContent("3 dependencies changed: tokio, tracing, url");
    expect(facts).toHaveTextContent("Project profile: repository file");
    expect(facts).toHaveTextContent("Project profile in MemCan: not checked");
  });

  it("puts the counts of the LLM's split into flows and steps under the LLM analysis", () => {
    renderBlock(tour);
    expect(screen.getByRole("region", { name: "Facts" })).not.toHaveTextContent(/flows|steps/);
    expect(screen.getByRole("region", { name: "LLM analysis" })).toHaveTextContent(
      "The LLM split this PR into 5 flows with 17 steps.",
    );
  });

  it("says CI was not checked when the server could not read the checks", () => {
    const context: PrContextView = structuredClone(tour.pr_context);
    context.facts.ci = { state: "not_checked", total: 0, failing: [] };
    renderBlock(tour, context);
    const facts = screen.getByRole("region", { name: "Facts" });
    expect(facts).toHaveTextContent("Check runs: not checked. Statuses: not checked.");
    expect(facts).not.toHaveTextContent(/no checks reported|passing/);
  });

  it.each([
    ["passing", "Check runs: not checked. Statuses: not checked."],
    ["pending", "Check runs: not checked. Statuses: not checked."],
    ["none", "Check runs: not checked. Statuses: not checked."],
  ] as const)("states CI %s as a fact from GitHub", (state, text) => {
    const t = structuredClone(tour);
    t.pr_context.facts.ci = { state, total: state === "none" ? 0 : 6, failing: [] };
    renderBlock(t);
    expect(screen.getByRole("region", { name: "Facts" })).toHaveTextContent(text);
  });

  it("names the kind of related work with a label map", () => {
    renderBlock(tour);
    const llm = screen.getByRole("region", { name: "LLM analysis" });
    expect(within(llm).getByText("Companion repository")).toBeInTheDocument();
    expect(within(llm).queryByText(/stacked pr/)).not.toBeInTheDocument();
  });

  it("states a missing project profile (noprofile)", () => {
    renderBlock(seedFor("noprofile", "tour") as Tour);
    expect(screen.getByRole("region", { name: "Facts" })).toHaveTextContent(
      "No project profile — ratings are component-level",
    );
  });

  it("never implies there is no profile when MemCan was not checked", () => {
    const t = structuredClone(tour);
    t.pr_context.facts.project_profile = { sources: [], memcan: "not_checked" };
    renderBlock(t);
    const facts = screen.getByRole("region", { name: "Facts" });
    expect(facts).toHaveTextContent("No project profile file in the repository");
    expect(facts).toHaveTextContent("Project profile in MemCan: not checked");
    expect(facts).not.toHaveTextContent(/No project profile —|component-level/);
  });

  it("states dependency files that were not checked, never as zero dependencies", () => {
    const t = structuredClone(tour);
    t.pr_context.facts.counters.dependencies_changed = [];
    t.pr_context.facts.counters.dependency_files_not_checked = ["Cargo.lock", "web/package-lock.json"];
    renderBlock(t);
    const facts = screen.getByRole("region", { name: "Facts" });
    expect(facts).toHaveTextContent("Dependency files not checked: Cargo.lock, web/package-lock.json");
    expect(facts).not.toHaveTextContent(/0 dependencies/);
  });

  it("states both the changed dependencies and the files that were not checked", () => {
    const t = structuredClone(tour);
    t.pr_context.facts.counters.dependency_files_not_checked = ["Cargo.lock"];
    renderBlock(t);
    const facts = screen.getByRole("region", { name: "Facts" });
    expect(facts).toHaveTextContent("3 dependencies changed: tokio, tracing, url");
    expect(facts).toHaveTextContent("Dependency files not checked: Cargo.lock");
  });

  it.each([
    ["checked", "3 review threads on GitHub"],
    ["partial", "3 review threads listed. Review threads: the list from GitHub is incomplete"],
    ["not_checked", "Review threads: not checked"],
  ] as const)("states the review threads as a fact (%s)", (status, text) => {
    const t = structuredClone(tour);
    t.pr_context.facts.threads_status = status;
    render(<PrContextBlock pr={t.analysis.pr} context={t.pr_context} threadCount={3} />);
    const facts = screen.getByRole("region", { name: "Facts" });
    expect(facts).toHaveTextContent(text);
    if (status === "not_checked") expect(facts).not.toHaveTextContent(/\d+ review threads?|no open threads/);
  });

  it("says related work was not checked when it could not be looked up", () => {
    const t = structuredClone(tour);
    t.pr_context.facts.related_work_checked = false;
    renderBlock(t);
    expect(screen.getByText("Related work: not checked")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "acme/node-client#479" })).not.toBeInTheDocument();
  });

  it.each([
    "https://evil.example/login",
    "https://github.com.evil.example/acme/node-client/pull/479",
    "https://user:pass@github.com/acme/node-client/pull/479",
    "https://github.com/acme/node-client/pull/479",
    "javascript:alert(1)",
    "acme/node-client#479",
  ])("renders the LLM-supplied related-work reference %s as text, never as a link (T12)", (reference) => {
    const t = structuredClone(tour);
    t.pr_context.llm.related_work = [{ reference, kind: "competing_solution", overlap: "Overlaps." }];
    t.pr_context.facts.related_work_links = [];
    renderBlock(t);
    const llm = screen.getByRole("region", { name: "LLM analysis" });
    expect(within(llm).getByText(reference)).toBeInTheDocument();
    expect(within(llm).queryAllByRole("link")).toEqual([]);
    expect(llm.querySelectorAll("[href]")).toHaveLength(0);
  });

  it("warns before the LLM analysis when the PR changes agent instruction files (T14)", () => {
    const t = structuredClone(tour);
    t.pr_context.facts.agent_instruction_files = ["AGENTS.md", ".claude/settings.json"];
    renderBlock(t);
    const note = screen.getByRole("note");
    expect(note).toHaveTextContent("AGENTS.md, .claude/settings.json");
    expect(note).toHaveTextContent("From the diff; no LLM.");
    const llm = screen.getByRole("region", { name: "LLM analysis" });
    expect(note.compareDocumentPosition(llm) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("shows no instruction-file notice when the diff has none", () => {
    renderBlock(tour);
    expect(screen.queryByRole("note")).not.toBeInTheDocument();
  });

  it.each(FIXTURE_STATES)("shows every fact computed or not checked in state %s", (state) => {
    const stateTour = seedFor(state, "tour") as Tour;
    const { container } = renderBlock(stateTour);
    const facts = screen.getByRole("region", { name: "Facts" });
    expect(facts).toHaveTextContent(/Check runs: /);
    expect(facts).toHaveTextContent(/\d+ test lines changed or removed/);
    expect(facts).toHaveTextContent(/\d+ dependencies changed/);
    expect(facts).toHaveTextContent(/Project profile: |No project profile — ratings are component-level/);
    const relatedNotChecked = screen.queryByText("Related work: not checked");
    if (stateTour.pr_context.facts.related_work_checked) expect(relatedNotChecked).toBeNull();
    else expect(relatedNotChecked).toBeInTheDocument();
    expectTrustWording(container.textContent ?? "");
  });
});

it("shows usage in the facts footer and omits it without a report", () => {
  const { rerender } = render(
    <PrContextBlock
      pr={tour.analysis.pr}
      context={tour.pr_context}
      threadCount={0}
      usage={{
        input_tokens: 1000,
        cached_input_tokens: 500,
        output_tokens: 12,
        reasoning_output_tokens: null,
        jobs: 1,
      }}
    />,
  );
  expect(
    within(screen.getByRole("region", { name: "Facts" })).getByText(/LLM usage so far: 1 k input/),
  ).toBeVisible();
  rerender(<PrContextBlock pr={tour.analysis.pr} context={tour.pr_context} threadCount={0} usage={null} />);
  expect(screen.queryByText(/LLM usage so far/)).toBeNull();
});

it("links the CI summary to the checks page it is given", () => {
  const href = "https://github.com/acme/node-client/pull/482/checks";
  render(
    <PrContextBlock pr={tour.analysis.pr} context={tour.pr_context} threadCount={0} checksHref={href} />,
  );
  const link = screen.getByRole("link", { name: /^Check runs: .* \(opens on GitHub in a new tab\)$/ });
  expect(link.querySelector('svg[data-icon="github"]')).not.toBeNull();
  expect(link).toHaveAttribute("href", href);
  expect(link).toHaveAttribute("target", "_blank");
  expect(link).toHaveAccessibleDescription(/^Open the checks on GitHub \(new tab\)/);
});
