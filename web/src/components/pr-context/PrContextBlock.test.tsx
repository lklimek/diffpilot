import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { Tour } from "../../api/generated/types";
import { expectTrustWording, toursByState } from "../../__tests__/wording";
import { seedFor } from "../../mocks/fixtures";
import { PrContextBlock } from "./PrContextBlock";

const tour = seedFor("default", "tour") as Tour;
const renderBlock = (t: Tour) => render(<PrContextBlock pr={t.analysis.pr} context={t.pr_context} />);

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
    expect(facts).toHaveTextContent("5 flows, 17 steps");
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

  it("lists agent instruction files in the diff as a notice", () => {
    const t = structuredClone(tour);
    t.pr_context.facts.agent_instruction_files = ["AGENTS.md"];
    renderBlock(t);
    expect(screen.getByRole("note")).toHaveTextContent("AGENTS.md");
  });

  it.each(toursByState)("keeps the wording rules in state %s", (_, stateTour) => {
    const { container } = renderBlock(stateTour);
    expectTrustWording(container.textContent ?? "");
  });
});
