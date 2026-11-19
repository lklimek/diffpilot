import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { AskQuestion } from "../../api/generated/types";
import { expectTrustWording } from "../../__tests__/wording";
import { FIXTURE_STATES, seedFor } from "../../mocks/fixtures";
import { AskAnswers } from "./AskAnswers";

const question = (over: Partial<AskQuestion>): AskQuestion => ({
  id: "q1",
  mine: true,
  asked_by: "alice",
  anchor: { kind: "step", step_id: "f1.5", lines: null },
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
    expect(onAskAgain).toHaveBeenCalledWith(
      { kind: "step", step_id: "f1.5", lines: null },
      "Is the reply deduplicated?",
    );
  });

  it("asks a failed question about lines again on the same lines, with or without a step", async () => {
    const onAskAgain = vi.fn();
    const lines = {
      kind: "lines" as const,
      path: "src/a.rs",
      side: "base" as const,
      range: { start_line: 4, end_line: 6 },
    };
    const stepLines = { kind: "step" as const, step_id: "f1.5", lines: { start_line: 99, end_line: 99 } };
    render(
      <AskAnswers
        questions={[
          question({ id: "a", state: "failed", failure: "x", anchor: lines, step_id: undefined }),
          question({ id: "b", state: "failed", failure: "x", anchor: stepLines }),
        ]}
        onAskAgain={onAskAgain}
      />,
    );
    const again = screen.getAllByRole("button", { name: "Ask again" });
    expect(again).toHaveLength(2);
    await userEvent.click(again[0]!);
    expect(onAskAgain).toHaveBeenLastCalledWith(lines, "Is the reply deduplicated?");
    await userEvent.click(again[1]!);
    expect(onAskAgain).toHaveBeenLastCalledWith(stepLines, "Is the reply deduplicated?");
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
