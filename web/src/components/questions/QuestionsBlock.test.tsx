import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { AskQuestion } from "../../api/generated/types";
import { expectTrustWording } from "../../__tests__/wording";
import { QuestionsBlock } from "./QuestionsBlock";

const q = (over: Partial<AskQuestion>): AskQuestion => ({
  id: "q1",
  anchor: { kind: "step", step_id: "f1.5", lines: null },
  step_id: "f1.5",
  text: "Is the broadcast deduplicated?",
  state: "answered",
  answer: { answer: "Not visible in this repository.", evidence: [] },
  failure: null,
  created_at: "2026-11-16T10:20:00Z",
  mine: true,
  ...over,
});

describe("QuestionsBlock (the user's questions of the review)", () => {
  it("lists each question with its state or answer, grouped under a link to its step", async () => {
    const open = vi.fn();
    render(
      <QuestionsBlock
        questions={[
          q({}),
          q({ id: "q2", step_id: "f2.1", text: "Pending one?", state: "pending", answer: null }),
          q({
            id: "q3",
            step_id: "f2.1",
            text: "Failed one?",
            state: "failed",
            answer: null,
            failure: "Timeout.",
          }),
        ]}
        placeOf={(step) =>
          step === "f1.5"
            ? { label: "Flow 1 · step 5: Retry allowed?", open }
            : { label: "Flow 2 · step 1", open }
        }
      />,
    );
    expect(screen.getByRole("heading", { level: 2 })).toHaveTextContent("Your questions and answers (3)");
    expect(document.body).toHaveTextContent("Not visible in this repository.");
    expect(document.body).toHaveTextContent("Waiting for the LLM to start on it.");
    expect(document.body).toHaveTextContent("No answer: Timeout.");
    const link = screen.getByRole("button", { name: "Flow 1 · step 5: Retry allowed?" });
    expect(link).toHaveAccessibleDescription(/Opens the step/);
    await userEvent.click(link);
    expect(open).toHaveBeenCalledOnce();
    expect(
      within(screen.getAllByRole("listitem")[0]!).getByText("Is the broadcast deduplicated?"),
    ).toBeVisible();
    expectTrustWording(document.body.textContent ?? "");
    // The UI cannot back a claim that nothing ran; it states what the agent was told.
    expect(document.body).toHaveTextContent("Answers are LLM analysis; the agent was told not to run code.");
    expect(document.body).not.toHaveTextContent(/nothing (was )?executed/i);
  });

  it("only the user's own questions; a step no longer in the tour is said plainly", () => {
    render(
      <QuestionsBlock
        questions={[q({ mine: false, text: "Someone else's" }), q({ id: "q9", step_id: "gone" })]}
        placeOf={() => null}
      />,
    );
    expect(document.body).not.toHaveTextContent("Someone else's");
    expect(document.body).toHaveTextContent("The step of this question is not part of this tour.");
  });

  it("none asked yet: says how to ask", () => {
    render(<QuestionsBlock questions={[]} placeOf={() => null} />);
    expect(document.body).toHaveTextContent("You have not asked any questions in this review yet.");
  });
});
