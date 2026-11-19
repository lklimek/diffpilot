import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import type { AskQuestion, StepId } from "../../api/generated/types";
import { expectTrustWording } from "../../__tests__/wording";
import { AskThread, type AskScope } from "./AskThread";

const question = (over: Partial<AskQuestion>): AskQuestion => ({
  id: "q1",
  mine: true,
  asked_by: "alice",
  step_id: "f1.5",
  text: "Is the reply deduplicated?",
  state: "answered",
  answer: { answer: "Yes.", evidence: [] },
  failure: null,
  created_at: "2026-11-16T10:20:00Z",
  ...over,
});

const thread = [
  question({
    id: "c",
    text: "Third, on step 5?",
    created_at: "2026-11-16T12:00:00Z",
    state: "answering",
    answer: null,
  }),
  question({ id: "a", text: "First, on step 5?", created_at: "2026-11-16T10:00:00Z" }),
  question({ id: "b", text: "Second, on step 2?", step_id: "f1.2", created_at: "2026-11-16T11:00:00Z" }),
];

const placeOf = (open: (step: StepId) => void) => (step: StepId) =>
  step === "f9.9" ? null : { label: `Flow 1 · step ${step.slice(3)}: Title`, open: () => open(step) };

function Harness({
  questions = thread,
  onOpen = () => {},
  initial = "conversation",
}: {
  questions?: AskQuestion[];
  onOpen?: (step: StepId) => void;
  initial?: AskScope;
}) {
  const [scope, setScope] = useState<AskScope>(initial);
  return (
    <AskThread
      questions={questions}
      stepId="f1.5"
      scope={scope}
      onScope={setScope}
      placeOf={placeOf(onOpen)}
      onAskAgain={() => {}}
    />
  );
}

const items = () =>
  within(screen.getByRole("list", { name: "Ask conversation" }))
    .getAllByRole("listitem")
    .map((i) => i.querySelector("[data-question]")?.textContent);

describe("AskThread: the analysis's Ask conversation as one thread", () => {
  it("lists every question of the analysis oldest first, each with the step it was asked on as a link", async () => {
    const onOpen = vi.fn();
    render(<Harness onOpen={onOpen} />);
    expect(items()).toEqual([
      "Asked by you: First, on step 5?",
      "Asked by you: Second, on step 2?",
      "Asked by you: Third, on step 5?",
    ]);
    const list = screen.getByRole("list", { name: "Ask conversation" });
    const link = within(list).getByRole("button", { name: "Flow 1 · step 2: Title" });
    expect(link).toHaveAccessibleDescription(/Opens the step this question was asked on/);
    await userEvent.click(link);
    expect(onOpen).toHaveBeenCalledWith("f1.2");
    // A pending answer keeps the existing progress line.
    expect(within(list).getByRole("status")).toHaveTextContent(/the conversation is prepared first/);
  });

  it("filters to this step and back; the whole conversation is the default", async () => {
    render(<Harness />);
    const whole = screen.getByRole("button", { name: "Whole conversation" });
    const here = screen.getByRole("button", { name: "This step" });
    expect(whole).toHaveAttribute("aria-pressed", "true");
    expect(here).toHaveAttribute("aria-pressed", "false");
    expect(whole).toHaveAccessibleDescription(/every question you asked in this analysis/);
    // The thread is the user's conversation about this analysis; nothing is claimed about the LLM's memory.
    expect(whole).not.toHaveAccessibleDescription(/remember/i);
    expect(here).toHaveAccessibleDescription(/only the questions asked on this step/);
    await userEvent.click(here);
    expect(here).toHaveAttribute("aria-pressed", "true");
    expect(items()).toEqual(["Asked by you: First, on step 5?", "Asked by you: Third, on step 5?"]);
    await userEvent.click(whole);
    expect(items()).toHaveLength(3);
  });

  it("says so when this step has no question yet but the conversation has", () => {
    render(<Harness questions={[thread[2]!]} initial="step" />);
    expect(screen.queryByRole("list", { name: "Ask conversation" })).toBeNull();
    expect(screen.getByText(/No question on this step yet/)).toBeInTheDocument();
  });

  it("names a question whose step is not part of the tour instead of a link", () => {
    render(<Harness questions={[question({ step_id: "f9.9" })]} />);
    expect(screen.getByText("On a step that is not part of this tour")).toBeInTheDocument();
  });

  it("scrolls to the newest entry, but not while the user reads further up", () => {
    const { rerender } = render(<Harness />);
    const box = screen.getByRole("region", { name: "Scrollable conversation" });
    // One name each: the scroller and the list inside it are announced differently.
    expect(screen.queryByRole("region", { name: "Ask conversation" })).toBeNull();
    expect(box.className).toMatch(/(^|\s)overflow-y-auto(\s|$)/);
    expect(box).toHaveAttribute("tabindex", "0");
    // jsdom has no layout: give the box a height and content size.
    Object.defineProperty(box, "clientHeight", { configurable: true, value: 100 });
    Object.defineProperty(box, "scrollHeight", { configurable: true, value: 1000 });
    box.scrollTop = 900;
    box.dispatchEvent(new Event("scroll"));
    const answered = thread.map((q) =>
      q.id === "c" ? { ...q, state: "answered" as const, answer: { answer: "Done.", evidence: [] } } : q,
    );
    Object.defineProperty(box, "scrollHeight", { configurable: true, value: 1200 });
    rerender(<Harness questions={answered} />);
    expect(box.scrollTop).toBe(1200);
    // Scrolled up: an update leaves the position alone.
    box.scrollTop = 200;
    box.dispatchEvent(new Event("scroll"));
    Object.defineProperty(box, "scrollHeight", { configurable: true, value: 1400 });
    rerender(<Harness questions={answered.map((q) => (q.id === "a" ? { ...q, text: "Edited?" } : q))} />);
    expect(box.scrollTop).toBe(200);
    // A new question of the user's always shows.
    rerender(
      <Harness
        questions={[...answered, question({ id: "d", text: "Fourth?", created_at: "2026-11-16T13:00:00Z" })]}
      />,
    );
    expect(box.scrollTop).toBe(1400);
  });

  it("renders list lines of an answer as typed, one per line, with a hanging indent", () => {
    const answer =
      "Potential double send:\n1. The first reply is late.\n2. The client retries.\n\n- Not reproduced.";
    render(<Harness questions={[question({ answer: { answer, evidence: [] } })]} />);
    const lines = screen.getAllByText(/^(1\.|2\.|-) /);
    expect(lines.map((l) => l.textContent)).toEqual([
      "1. The first reply is late.",
      "2. The client retries.",
      "- Not reproduced.",
    ]);
    for (const line of lines) expect(line.style.textIndent).toMatch(/^-\d+ch$/);
    expect(screen.getByText("Potential double send:").style.textIndent).toBe("");
  });

  it("offers Use as review comment on answers asked on the open step only", async () => {
    const onUseAsComment = vi.fn();
    render(
      <AskThread
        questions={thread}
        stepId="f1.5"
        scope="conversation"
        onScope={() => {}}
        placeOf={placeOf(() => {})}
        onUseAsComment={onUseAsComment}
      />,
    );
    // Answered: "a" (step 5) and "b" (step 2); "c" is still being answered.
    const buttons = screen.getAllByRole("button", { name: "Use as review comment" });
    expect(buttons).toHaveLength(1);
    expect(buttons[0]).toHaveAccessibleDescription(/draft.*nothing is posted/i);
    await userEvent.click(buttons[0]!);
    expect(onUseAsComment).toHaveBeenCalledWith(expect.objectContaining({ id: "a" }));
  });

  it("keeps the wording rules", () => {
    const { container } = render(<Harness />);
    expectTrustWording(container.textContent ?? "");
  });
});
