import type { AskQuestion } from "../../../api/generated/types";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { threadsOf } from "./model";
import { QuestionsHere } from "./QuestionsHere";

const q = (id: string, extra: Partial<AskQuestion> = {}): AskQuestion => ({
  id,
  mine: true,
  anchor: { kind: "pr" },
  text: `Question ${id}?`,
  state: "answered",
  answer: { answer: "Line 1\nLine 2\nLine 3\nLine 4", evidence: [] },
  failure: null,
  created_at: "2026-11-19T10:00:00Z",
  ...extra,
});

describe("QuestionsHere: the Inspector copy of the chat (proposal AC11)", () => {
  it("lists each thread with its state or first answer lines, its follow-ups, and Open in chat", async () => {
    const onOpen = vi.fn();
    const onAsk = vi.fn();
    render(
      <QuestionsHere
        threads={threadsOf([
          q("a"),
          q("b", { parent_id: "a", state: "answering", answer: null, created_at: "2026-11-19T10:05:00Z" }),
          q("c", {
            state: "failed",
            answer: null,
            failure: "the LLM did not finish.",
            created_at: "2026-11-19T10:06:00Z",
          }),
        ])}
        what="this step"
        onOpenInChat={onOpen}
        onAsk={onAsk}
      />,
    );
    const region = screen.getByRole("region", { name: "Your questions about this step (2)" });
    const [a, c] = within(region).getAllByRole("listitem");
    expect(a).toHaveTextContent("Question a?");
    expect(a).toHaveTextContent("LLM answer Line 1 Line 2 Line 3");
    expect(a).not.toHaveTextContent("Line 4");
    expect(a).toHaveTextContent("1 follow-up · latest being answered");
    expect(c).toHaveTextContent("No answer: the LLM did not finish.");
    expect(within(region).queryByRole("textbox")).toBeNull();
    const user = userEvent.setup();
    await user.click(within(a!).getByRole("button", { name: "Open in chat" }));
    expect(onOpen).toHaveBeenCalledWith("a");
    await user.click(within(region).getByRole("button", { name: /^Ask about this step/ }));
    expect(onAsk).toHaveBeenCalled();
  });

  it("says when there is no question here yet", () => {
    render(<QuestionsHere threads={[]} what="these lines" onOpenInChat={() => {}} onAsk={() => {}} />);
    expect(screen.getByRole("region", { name: "Your questions about these lines (0)" })).toHaveTextContent(
      "No questions about these lines yet.",
    );
  });
});
