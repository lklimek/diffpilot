import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { Tour, AskQuestion } from "../../api/generated/types";
import { seedFor } from "../../mocks/fixtures";
import { PrContextBlock } from "./PrContextBlock";
import { TopBar } from "../top-bar/TopBar";
import { AskAnswers } from "../ask/AskAnswers";
import { FeedbackButtons } from "../feedback/FeedbackButtons";
const tour = seedFor("default", "tour") as Tour;
const counts = { passed: 1, skipped: 4, neutral: 0, pending: 0, failed: 0 };
describe("current wire facts", () => {
  it.each(["context", "top bar"])("%s gives exact counts from both CI sources", (where) => {
    const ci = {
      state: "pending" as const,
      total: 6,
      failing: [],
      check_runs: counts,
      statuses: { ...counts, passed: 0, skipped: 0, pending: 1 },
    };
    const context = structuredClone(tour.pr_context);
    context.facts.ci = ci;
    render(
      where === "context" ? (
        <PrContextBlock pr={tour.analysis.pr} context={context} threadCount={0} />
      ) : (
        <TopBar login="alice" ci={ci} pr={tour.analysis.pr} />
      ),
    );
    expect(screen.getByText("Check runs: 1 passed, 4 skipped. Statuses: 1 pending.")).toBeVisible();
    expect(screen.queryByText(/all.*passing/i)).toBeNull();
  });
  it("keeps MemCan connection separate from profile and counts added test lines", () => {
    const context = structuredClone(tour.pr_context);
    context.facts.memcan_status = { state: "not_reachable", last_success_at: null };
    context.facts.counters.lines_removed_from_tests = 4;
    context.facts.counters.lines_added_to_tests = 57;
    render(<PrContextBlock pr={tour.analysis.pr} context={context} threadCount={0} />);
    expect(screen.getByText(/MemCan: not reachable/)).toBeVisible();
    expect(screen.getByText("4 test lines changed or removed, 57 added")).toBeVisible();
  });
  it("attributes shared questions and only offers ask-again for mine", () => {
    const question: AskQuestion = {
      id: "q1",
      step_id: "f1.1",
      text: "Why?",
      state: "failed",
      answer: null,
      failure: "Failed",
      created_at: "2026-11-17T00:00:00Z",
      asked_by: "alice",
      mine: false,
    };
    const view = render(<AskAnswers questions={[question]} onAskAgain={vi.fn()} />);
    expect(screen.getByText(/asked by alice/i)).toBeVisible();
    expect(screen.queryByRole("button", { name: "Ask again" })).toBeNull();
    view.rerender(<AskAnswers questions={[{ ...question, mine: true }]} onAskAgain={vi.fn()} />);
    expect(screen.getByText(/asked by you/i)).toBeVisible();
    expect(screen.getByRole("button", { name: "Ask again" })).toBeVisible();
  });
  it("does not call a correct rating calibration memory", () => {
    render(
      <FeedbackButtons
        subject="step"
        target={{ target: "step", step_id: "f1.1" }}
        feedback={[
          {
            id: "fb1",
            target: { target: "step", step_id: "f1.1" },
            value: "correct",
            note: null,
            memory: "written",
            calibration_example: false,
            created_at: "2026-11-17T00:00:00Z",
          },
        ]}
        onSubmit={vi.fn()}
        onRemove={vi.fn()}
      />,
    );
    expect(screen.getByText("correct: Recorded")).toBeVisible();
    expect(screen.queryByText(/Saved to calibration memory/)).toBeNull();
  });
});

describe("structured related links", () => {
  it("links only through structured targets and marks unchecked targets", () => {
    const context = structuredClone(tour.pr_context);
    context.facts.related_work_checked = true;
    context.llm.related_work = [
      { reference: "javascript:alert(1)", kind: "linked_issue", overlap: "Related" },
    ];
    context.facts.related_work_links = [
      { reference: "javascript:alert(1)", repo: "acme/node", number: 42, kind: "issue", verified: false },
    ];
    render(<PrContextBlock pr={tour.analysis.pr} context={context} threadCount={0} />);
    const link = screen.getByRole("link", { name: "acme/node#42" });
    expect(link).toHaveAttribute("href", "https://github.com/acme/node/issues/42");
    expect(link).toHaveAttribute("target", "_blank");
    expect(link).toHaveAttribute("rel", "noopener noreferrer");
    expect(screen.getByText(/not checked against GitHub/)).toBeVisible();
  });
});
