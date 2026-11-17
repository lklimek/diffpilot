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
  mine: true,
  asked_by: "alice",
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

const step5 = { id: "f1.5", label: "Step 5" };
const step6 = { id: "f1.6", label: "Step 6" };

describe("AskForm", () => {
  it("keeps the field read-only while sending and clears only what was sent", async () => {
    const user = userEvent.setup();
    const pending = deferred();
    render(<AskForm step={step5} onAsk={() => pending.promise} />);
    const field = screen.getByRole("textbox");
    await user.type(field, "A{Enter}");
    expect(field).toHaveAttribute("readonly");
    await user.type(field, "B");
    expect(field).toHaveValue("A");
    pending.resolve();
    await vi.waitFor(() => expect(field).toHaveValue(""));
    expect(field).not.toHaveAttribute("readonly");
  });

  it("ignores the outcome of a question sent from a step that is no longer current", async () => {
    const user = userEvent.setup();
    const pending = deferred();
    const { rerender } = render(<AskForm step={step5} onAsk={() => pending.promise} />);
    await user.type(screen.getByRole("textbox"), "A{Enter}");
    rerender(<AskForm step={step6} onAsk={() => pending.promise} />);
    const field = screen.getByRole("textbox", { name: /Step 6/ });
    await user.type(field, "B");
    pending.reject(new Error("failed"));
    await Promise.resolve();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(field).toHaveValue("B");
  });

  it("drops the draft and the error on a step change, also through null", async () => {
    const user = userEvent.setup();
    const onAsk = vi.fn().mockRejectedValue(new Error("failed"));
    const { rerender } = render(<AskForm step={step5} onAsk={onAsk} />);
    await user.type(screen.getByRole("textbox"), "A{Enter}");
    await screen.findByRole("alert");
    rerender(<AskForm step={step6} onAsk={onAsk} />);
    expect(screen.getByRole("textbox")).toHaveValue("");
    expect(screen.queryByRole("alert")).toBeNull();
    await user.type(screen.getByRole("textbox"), "for 6");
    rerender(<AskForm step={null} onAsk={onAsk} />);
    rerender(<AskForm step={step6} onAsk={onAsk} />);
    expect(screen.getByRole("textbox")).toHaveValue("");
    await user.type(screen.getByRole("textbox"), "Q{Enter}");
    expect(onAsk).toHaveBeenLastCalledWith("f1.6", "Q");
  });

  it("sends the trimmed question on Enter and clears the field when it was accepted", async () => {
    const user = userEvent.setup();
    const onAsk = vi.fn().mockResolvedValue(undefined);
    render(<AskForm step={{ id: "f1.5", label: "Step 5 · Retry allowed?" }} onAsk={onAsk} />);
    const field = screen.getByRole("textbox", { name: /Ask the LLM about Step 5 · Retry allowed\?/ });
    await user.type(field, "  Is it idempotent?  {Enter}");
    expect(onAsk).toHaveBeenCalledWith("f1.5", "Is it idempotent?");
    expect(field).toHaveValue("");
  });

  it("adds a line on Shift+Enter instead of sending", async () => {
    const user = userEvent.setup();
    const onAsk = vi.fn();
    render(<AskForm step={step5} onAsk={onAsk} />);
    const field = screen.getByRole("textbox");
    await user.type(field, "one{Shift>}{Enter}{/Shift}two");
    expect(onAsk).not.toHaveBeenCalled();
    expect(field).toHaveValue("one\ntwo");
  });

  it("asks for text instead of sending an empty question", async () => {
    const onAsk = vi.fn();
    render(<AskForm step={step5} onAsk={onAsk} />);
    await userEvent.click(screen.getByRole("button", { name: /^Ask/ }));
    expect(onAsk).not.toHaveBeenCalled();
    expect(screen.getByRole("alert")).toHaveTextContent("Write a question first.");
    expect(screen.getByRole("textbox")).toHaveAttribute("aria-invalid", "true");
  });

  it("sends once while the question is being sent, and keeps the text when sending failed", async () => {
    const user = userEvent.setup();
    const pending = deferred();
    const onAsk = vi.fn().mockReturnValue(pending.promise);
    render(<AskForm step={step5} onAsk={onAsk} />);
    const field = screen.getByRole("textbox");
    await user.type(field, "Why?");
    await user.click(screen.getByRole("button", { name: /^Ask/ }));
    const sending = screen.getByRole("button", { name: /Sending/ });
    expect(sending).toHaveAttribute("aria-disabled", "true");
    expect(sending).toHaveAccessibleDescription(/Not available while the current request runs\./);
    await user.click(sending);
    await user.type(field, "{Enter}");
    expect(onAsk).toHaveBeenCalledOnce();
    pending.reject(new Error("The analysis is not finished."));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "The question was not sent: The analysis is not finished.",
    );
    expect(field).toHaveValue("Why?");
  });

  it("grows the one-line field while typing", async () => {
    render(<AskForm step={step5} onAsk={() => {}} />);
    const field = screen.getByRole("textbox");
    expect(field).toHaveAttribute("rows", "1");
    Object.defineProperty(field, "scrollHeight", { configurable: true, value: 64 });
    await userEvent.type(field, "x");
    expect(field.style.height).toBe("66px");
  });

  it("exposes the field for the `a` shortcut", () => {
    const ref = createRef<HTMLTextAreaElement>();
    render(<AskForm step={step5} onAsk={() => {}} inputRef={ref} />);
    ref.current!.focus();
    expect(screen.getByRole("textbox")).toHaveFocus();
  });

  it("explains that Ask needs a step when none is open, without a field", () => {
    render(<AskForm step={null} onAsk={() => {}} />);
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(screen.getByText(/Ask works on a step/)).toBeInTheDocument();
  });
});

describe("AskAnswers", () => {
  it("wraps long answer words and code inside the inspector", () => {
    const questions = structuredClone(seedFor("resume", "questions") as AskQuestion[]);
    const long = "x".repeat(500);
    questions[0]!.answer!.answer = `${long}\n\n\`\`\`text\n${long}\n\`\`\``;
    render(<AskAnswers questions={questions} />);
    const text = screen.getByText(long, { selector: "div" });
    expect(text).toHaveClass("[overflow-wrap:anywhere]");
    expect(text).toHaveClass("[&_pre]:whitespace-pre-wrap");
  });

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

  it("shows a question waiting for the usage limit with the reset time", () => {
    const resumes = new Date();
    resumes.setHours(23, 59, 0, 0);
    render(<AskAnswers questions={[question({ state: "pending", resumes_at: resumes.toISOString() })]} />);
    expect(screen.getByRole("status")).toHaveTextContent(
      "Waiting for the LLM usage limit to reset; the question resumes automatically at 23:59.",
    );
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
    expect(onAskAgain).toHaveBeenCalledWith("f1.5", "Is the reply deduplicated?");
  });

  it("asks again once while pending and shows a rejection", async () => {
    const user = userEvent.setup();
    const pending = deferred();
    const onAskAgain = vi.fn().mockReturnValue(pending.promise);
    render(<AskAnswers questions={[question({ state: "failed", failure: "x" })]} onAskAgain={onAskAgain} />);
    const button = screen.getByRole("button", { name: "Ask again" });
    await user.dblClick(button);
    expect(onAskAgain).toHaveBeenCalledOnce();
    const asking = screen.getByRole("button", { name: /Asking/ });
    expect(asking).toHaveAttribute("aria-disabled", "true");
    expect(asking).toHaveAccessibleDescription(/Not available while the current request runs\./);
    pending.reject(new Error("The analysis is busy."));
    expect(await screen.findByRole("alert")).toHaveTextContent("Not asked again: The analysis is busy.");
    expect(screen.getByRole("button", { name: "Ask again" })).not.toHaveAttribute("aria-disabled");
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
        <AskForm step={step5} onAsk={() => {}} />
      </>,
    );
    expectTrustWording(container.textContent ?? "");
  });
});

it("renders only inline and fenced code in answers, leaving HTML and Markdown literal", () => {
  const answer =
    "Use `value` and **literal** [link](https://evil.test).\n```html\n<img src=x onerror=alert(1)>\n```\n<script>alert(1)</script>";
  const { container } = render(
    <AskAnswers questions={[question({ state: "answered", answer: { answer, evidence: [] } })]} />,
  );
  expect(container.querySelector("code")?.textContent).toBe("value");
  expect(container.querySelector("pre code")?.textContent).toBe("<img src=x onerror=alert(1)>\n");
  expect(container.querySelector("img,script,a,strong")).toBeNull();
  expect(container).toHaveTextContent("**literal** [link](https://evil.test)");
});
