import { act, render, screen, within } from "@testing-library/react";
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

  it("shows the answers below the question field, newest first, pending and failed in the same place", () => {
    const questions = [
      question({ id: "old", text: "Old question?", state: "failed", failure: "the job failed." }),
      question({ id: "new", text: "New question?", created_at: "2026-11-16T10:30:00Z" }),
    ];
    render(<AskForm step={step5} onAsk={() => {}} answers={<AskAnswers questions={questions} />} />);
    const field = screen.getByRole("textbox");
    const items = within(screen.getByRole("list", { name: "Questions asked about this step" })).getAllByRole(
      "listitem",
    );
    expect(items.map((i) => i.textContent)).toEqual([
      expect.stringContaining("Asked by you: New question?"),
      expect.stringContaining("Asked by you: Old question?"),
    ]);
    for (const item of items)
      expect(field.compareDocumentPosition(item) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(items[0]).toHaveTextContent("Waiting for the LLM");
    expect(items[1]).toHaveTextContent("No answer: the job failed.");
    expect(screen.getByRole("heading", { name: /Ask/ })).toHaveTextContent(/answers appear below/);
  });

  it("keeps the answers in a block of bounded height with its own scroll, so the rest of the dock stays in view", async () => {
    const user = userEvent.setup();
    const onAsk = vi.fn().mockResolvedValue(undefined);
    render(
      <AskForm step={step5} onAsk={onAsk} answers={<AskAnswers questions={[question({ id: "q" })]} />} />,
    );
    const block = screen.getByRole("region", { name: "Answers" });
    expect(block).toContainElement(screen.getByRole("list", { name: "Questions asked about this step" }));
    expect(block.className).toMatch(/(^|\s)max-h-\[/);
    expect(block.className).toMatch(/(^|\s)overflow-y-auto(\s|$)/);
    expect(block).toHaveAttribute("tabindex", "0");
    // A new question goes to the top of the newest-first list: the block shows its start.
    block.scrollTop = 120;
    await user.type(screen.getByRole("textbox"), "Why?{Enter}");
    expect(block.scrollTop).toBe(0);
  });
});

describe("AskForm: a collapsible Inspector section", () => {
  const control = (open: boolean) => ({
    isOpen: vi.fn(() => open),
    toggle: vi.fn(),
    reveal: vi.fn(),
  });

  it("opens and closes with the chevron like the other sections; the header keeps the step's question count", async () => {
    const sections = control(false);
    const { rerender } = render(<AskForm step={step5} onAsk={() => {}} sections={sections} count={2} />);
    const header = screen.getByRole("button", { name: /^Ask about/ });
    expect(header).toHaveAttribute("aria-expanded", "false");
    expect(header).toHaveTextContent("2 questions on this step");
    expect(screen.queryByRole("textbox")).toBeNull();
    await userEvent.click(header);
    expect(sections.toggle).toHaveBeenCalledWith("ask");
    rerender(<AskForm step={step5} onAsk={() => {}} sections={control(true)} count={2} />);
    expect(screen.getByRole("button", { name: /^Ask about/ })).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("textbox", { name: "Ask the LLM about Step 5" })).toBeVisible();
  });

  it("opens itself when sending fails while collapsed, and announces the error", async () => {
    let fail: (e: Error) => void = () => {};
    const onAsk = () => new Promise((_, reject) => (fail = reject));
    const closed = control(false);
    const { rerender } = render(<AskForm step={step5} onAsk={onAsk} sections={control(true)} count={0} />);
    await userEvent.type(screen.getByRole("textbox"), "Why?{Enter}");
    // The reader collapses Ask while the question is on its way.
    rerender(<AskForm step={step5} onAsk={onAsk} sections={closed} count={0} />);
    await act(async () => fail(new Error("server gone")));
    expect(closed.reveal).toHaveBeenCalledWith("ask");
    rerender(<AskForm step={step5} onAsk={onAsk} sections={control(true)} count={0} />);
    expect(screen.getByRole("alert")).toHaveTextContent("The question was not sent: server gone");
  });

  it("opens itself for a notice such as a lost answer stream, and announces it", async () => {
    const closed = control(false);
    const opened = control(true);
    const { rerender } = render(<AskForm step={step5} onAsk={() => {}} sections={closed} count={0} />);
    rerender(<AskForm step={step5} onAsk={() => {}} sections={closed} count={0} notice="Connection lost." />);
    expect(closed.reveal).toHaveBeenCalledWith("ask");
    rerender(<AskForm step={step5} onAsk={() => {}} sections={opened} count={0} notice="Connection lost." />);
    expect(screen.getByRole("alert")).toHaveTextContent("Connection lost.");
  });

  it("says where answers appear: below the field, newest first", () => {
    render(<AskForm step={step5} onAsk={() => {}} sections={control(true)} count={0} />);
    const ask = screen.getByRole("region", { name: /^Ask/ });
    expect(ask).toHaveTextContent("answers appear below the field, newest first");
    expect(ask).not.toHaveTextContent(/pinned/);
    expect(screen.getByRole("button", { name: "Ask" })).not.toHaveAccessibleDescription(/pinned/);
  });
});

describe("AskAnswers", () => {
  it("shows the current file's name in bold in an answer's evidence", async () => {
    const user = userEvent.setup();
    render(
      <AskAnswers
        currentFile="src/rpc/client.rs"
        questions={[
          question({
            state: "answered",
            answer: {
              answer: "Yes.",
              evidence: [
                { file: "src/rpc/client.rs", side: "head", line: 99 },
                { file: "src/rpc/retry.rs", side: "head", line: 23 },
              ],
            },
          }),
        ]}
      />,
    );
    await user.click(screen.getByText("Evidence · 2 places"));
    expect(screen.getByText("client.rs")).toHaveClass("font-bold");
    expect(screen.getByText("retry.rs")).not.toHaveClass("font-bold");
  });

  it("wraps long answer words and code inside the inspector", () => {
    const questions = structuredClone(seedFor("resume", "questions") as AskQuestion[]);
    const long = "x".repeat(500);
    questions[0]!.answer!.answer = `${long}\n\n\`\`\`text\n${long}\n\`\`\``;
    render(<AskAnswers questions={questions} />);
    const text = screen.getByText(long, { selector: "div" });
    expect(text).toHaveClass("[overflow-wrap:anywhere]");
    expect(text).toHaveClass("[&_pre]:whitespace-pre-wrap");
  });

  it("hides an empty answer list", () => {
    render(<AskAnswers questions={[]} />);
    expect(screen.queryByText(/No questions asked about this step yet/)).not.toBeInTheDocument();
  });

  it("shows the answered question of the resume state whole, labelled as an LLM answer", () => {
    const questions = seedFor("resume", "questions") as AskQuestion[];
    render(<AskAnswers questions={questions} />);
    const item = screen.getByRole("listitem");
    expect(item).toHaveTextContent("Does the node deduplicate broadcast_tx by transaction id?");
    expect(item).toHaveTextContent(questions[0]!.answer!.answer);
    expect(within(item).getByText("LLM answer")).toHaveAccessibleDescription(/LLM/);
    expect(within(item).getByText("LLM answer")).toHaveAccessibleDescription(/told not to run/);
    expect(within(item).getByText("LLM answer")).not.toHaveAccessibleDescription(/nothing was executed/i);
    expect(item).toHaveTextContent("Evidence · 2 places");
    expect(within(item).queryByRole("link")).toBeNull();
  });

  it.each([
    ["pending", /Waiting for the LLM/],
    [
      "answering",
      /The LLM is working on the answer; it appears here whole when it is ready\. A first question, or the first after the analysis changed, takes longer: the conversation is prepared first\./,
    ],
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
