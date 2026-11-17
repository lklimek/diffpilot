import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { Tour } from "../../api/generated/types";
import { expectTrustWording } from "../../__tests__/wording";
import { FIXTURE_STATES, seedFor } from "../../mocks/fixtures";
import { PrContextBlock, type PrContextView } from "./PrContextBlock";

const tour = seedFor("default", "tour") as Tour;
const renderBlock = (t: Tour, context: PrContextView = t.pr_context) =>
  render(<PrContextBlock pr={t.analysis.pr} context={context} />);

describe("PrContextBlock", () => {
  it("separates the LLM analysis from the deterministic facts", () => {
    renderBlock(tour);
    const llm = screen.getByRole("region", { name: "LLM analysis" });
    const facts = screen.getByRole("region", { name: "Facts" });
    expect(within(llm).getAllByText("LLM analysis unless marked otherwise.")).toHaveLength(1);
    expect(within(facts).queryByText(/LLM analysis/)).not.toBeInTheDocument();
    // LLM content
    expect(llm).toHaveTextContent(tour.pr_context.llm.problem);
    expect(llm).toHaveTextContent(tour.pr_context.llm.what_it_gives);
    expect(llm).toHaveTextContent(tour.pr_context.llm.why_needed);
    expect(within(llm).getByText("acme/node-client#479")).toBeInTheDocument();
    expect(within(llm).getAllByRole("listitem").length).toBeGreaterThanOrEqual(6);
    expect(llm).toHaveTextContent("src/config.rs:23");
    // Facts
    expect(facts).toHaveTextContent("CI: 1 of 6 checks failing");
    expect(facts).toHaveTextContent("sync_headers::slow_peer_2000_headers");
    expect(facts).toHaveTextContent("12 lines removed from tests");
    expect(facts).toHaveTextContent("3 dependencies changed: tokio, tracing, url");
    expect(facts).toHaveTextContent("Project profile: repository file");
  });

  it("puts the counts of the LLM's split into flows and steps under the LLM analysis (UX-007)", () => {
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
    expect(facts).toHaveTextContent("CI: not checked");
    expect(facts).not.toHaveTextContent(/no checks reported|passing/);
  });

  it.each([
    ["passing", "CI: all 6 checks passing"],
    ["pending", "CI: 6 checks, still running"],
    ["none", "CI: no checks reported"],
  ] as const)("states CI %s as a fact from GitHub", (state, text) => {
    const t = structuredClone(tour);
    t.pr_context.facts.ci = { state, total: state === "none" ? 0 : 6, failing: [] };
    renderBlock(t);
    expect(screen.getByRole("region", { name: "Facts" })).toHaveTextContent(text);
  });

  it("names the kind of related work with a label map (UX-009)", () => {
    renderBlock(tour);
    const llm = screen.getByRole("region", { name: "LLM analysis" });
    expect(within(llm).getByText("Stacked PR")).toBeInTheDocument();
    expect(within(llm).queryByText(/stacked pr/)).not.toBeInTheDocument();
  });

  it("states a missing project profile (noprofile)", () => {
    renderBlock(seedFor("noprofile", "tour") as Tour);
    expect(screen.getByRole("region", { name: "Facts" })).toHaveTextContent(
      "No project profile — ratings are component-level",
    );
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
    expect(facts).toHaveTextContent(/CI: /);
    expect(facts).toHaveTextContent(/\d+ lines removed from tests/);
    expect(facts).toHaveTextContent(/\d+ dependencies changed/);
    expect(facts).toHaveTextContent(/Project profile: |No project profile — ratings are component-level/);
    const relatedNotChecked = screen.queryByText("Related work: not checked");
    if (stateTour.pr_context.facts.related_work_checked) expect(relatedNotChecked).toBeNull();
    else expect(relatedNotChecked).toBeInTheDocument();
    expectTrustWording(container.textContent ?? "");
  });
});
