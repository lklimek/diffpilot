import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createRef } from "react";
import { describe, expect, it, vi } from "vitest";
import type { AskQuestion } from "../../api/generated/types";
import { expectTrustWording } from "../../__tests__/wording";
import { FIXTURE_STATES, seedFor } from "../../mocks/fixtures";
import { AskAnswers } from "./AskAnswers";
import { AskForm } from "./AskForm";

const question = (over: Partial<AskQuestion>): AskQuestion => ({
  id: "q1",
  step_id: "f1.5",
  text: "Is the reply deduplicated?",
  state: "pending",
  answer: null,
  failure: null,
  created_at: "2026-11-16T10:20:00Z",
  ...over,
});

const deferred = () => {
  let resolve!: () => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<void>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
};

describe("AskForm", () => {
  it("sends the trimmed question on Enter and clears the field when it was accepted", async () => {
    const user = userEvent.setup();
    const onAsk = vi.fn().mockResolvedValue(undefined);
    render(<AskForm subject="Step 5 · Retry allowed?" onAsk={onAsk} />);
    const field = screen.getByRole("textbox", { name: /Ask the LLM about Step 5 · Retry allowed\?/ });
    await user.type(field, "  Is it idempotent?  {Enter}");
    expect(onAsk).toHaveBeenCalledWith("Is it idempotent?");
    expect(field).toHaveValue("");
  });

  it("adds a line on Shift+Enter instead of sending", async () => {
    const user = userEvent.setup();
    const onAsk = vi.fn();
    render(<AskForm subject="Step 5" onAsk={onAsk} />);
    const field = screen.getByRole("textbox");
    await user.type(field, "one{Shift>}{Enter}{/Shift}two");
    expect(onAsk).not.toHaveBeenCalled();
    expect(field).toHaveValue("one\ntwo");
  });

  it("asks for text instead of sending an empty question", async () => {
    const onAsk = vi.fn();
    render(<AskForm subject="Step 5" onAsk={onAsk} />);
    await userEvent.click(screen.getByRole("button", { name: /^Ask/ }));
    expect(onAsk).not.toHaveBeenCalled();
    expect(screen.getByRole("alert")).toHaveTextContent("Write a question first.");
    expect(screen.getByRole("textbox")).toHaveAttribute("aria-invalid", "true");
  });

  it("sends once while the question is being sent, and keeps the text when sending failed", async () => {
    const user = userEvent.setup();
    const pending = deferred();
    const onAsk = vi.fn().mockReturnValue(pending.promise);
    render(<AskForm subject="Step 5" onAsk={onAsk} />);
    const field = screen.getByRole("textbox");
    await user.type(field, "Why?");
    await user.click(screen.getByRole("button", { name: /^Ask/ }));
    expect(screen.getByRole("button", { name: /Sending/ })).toBeDisabled();
    await user.type(field, "{Enter}");
    expect(onAsk).toHaveBeenCalledOnce();
    pending.reject(new Error("The analysis is not finished."));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "The question was not sent: The analysis is not finished.",
    );
    expect(field).toHaveValue("Why?");
  });

  it("grows the one-line field while typing", async () => {
    render(<AskForm subject="Step 5" onAsk={() => {}} />);
    const field = screen.getByRole("textbox");
    expect(field).toHaveAttribute("rows", "1");
    Object.defineProperty(field, "scrollHeight", { configurable: true, value: 64 });
    await userEvent.type(field, "x");
    expect(field.style.height).toBe("66px");
  });

  it("exposes the field for the `a` shortcut", () => {
    const ref = createRef<HTMLTextAreaElement>();
    render(<AskForm subject="Step 5" onAsk={() => {}} inputRef={ref} />);
    ref.current!.focus();
    expect(screen.getByRole("textbox")).toHaveFocus();
  });

  it("explains that Ask needs a step when none is open, without a field", () => {
    render(<AskForm subject={null} onAsk={() => {}} />);
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(screen.getByText(/Ask works on a step/)).toBeInTheDocument();
  });
});

describe("AskAnswers", () => {
  it("says that nothing was asked yet", () => {
    render(<AskAnswers questions={[]} />);
    expect(screen.getByText(/No questions asked about this step yet/)).toBeInTheDocument();
  });

  it("shows the answered question of the resume state whole, labelled as an LLM answer", () => {
    const questions = seedFor("resume", "questions") as AskQuestion[];
    render(<AskAnswers questions={questions} />);
    const item = screen.getByRole("listitem");
    expect(item).toHaveTextContent("Does the node deduplicate broadcast_tx by transaction id?");
    expect(item).toHaveTextContent(questions[0]!.answer!.answer);
    expect(within(item).getByText("LLM answer")).toHaveAccessibleDescription(/LLM/);
    expect(item).toHaveTextContent("Lines read: src/mempool/mod.rs:129, src/rpc/retry.rs:24");
    expect(within(item).queryByRole("link")).toBeNull();
  });

  it.each([
    ["pending", /Waiting for the LLM/],
    ["answering", /The LLM is working on the answer; it appears here whole when it is ready/],
  ] as const)("shows the %s state as a status", (state, text) => {
    render(<AskAnswers questions={[question({ state })]} />);
    expect(screen.getByRole("status")).toHaveTextContent(text);
  });

  it("shows a failed question with its plain reason and lets the user ask again", async () => {
    const onAskAgain = vi.fn();
    render(
      <AskAnswers
        questions={[question({ state: "failed", failure: "The LLM usage limit was reached." })]}
        onAskAgain={onAskAgain}
      />,
    );
    expect(screen.getByRole("listitem")).toHaveTextContent("No answer: The LLM usage limit was reached.");
    await userEvent.click(screen.getByRole("button", { name: "Ask again" }));
    expect(onAskAgain).toHaveBeenCalledWith("Is the reply deduplicated?");
  });

  it("lists the newest question first", () => {
    render(
      <AskAnswers
        questions={[
          question({ id: "a", text: "older", created_at: "2026-11-16T10:00:00Z" }),
          question({ id: "b", text: "newer", created_at: "2026-11-16T11:00:00Z" }),
        ]}
      />,
    );
    const items = screen.getAllByRole("listitem");
    expect(items[0]).toHaveTextContent("newer");
    expect(items[1]).toHaveTextContent("older");
  });

  it("renders the question and the answer as text, never as HTML or links (T12)", () => {
    const evil = '<img src=x onerror="alert(1)"> [x](javascript:alert(1)) https://evil.example';
    const { container } = render(
      <AskAnswers
        questions={[question({ text: evil, state: "answered", answer: { answer: evil, evidence: [] } })]}
      />,
    );
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector("a")).toBeNull();
    expect(container).toHaveTextContent('<img src=x onerror="alert(1)">');
  });

  it.each(FIXTURE_STATES)("keeps the wording rules in state %s", (state) => {
    const questions = seedFor(state, "questions") as AskQuestion[];
    const { container } = render(
      <>
        <AskAnswers questions={[...questions, question({ state: "failed", failure: "x" })]} />
        <AskForm subject="Step 5" onAsk={() => {}} />
      </>,
    );
    expectTrustWording(container.textContent ?? "");
  });
});
