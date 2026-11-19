import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { AskQuestion } from "../../api/generated/types";
import { expectTrustWording } from "../../__tests__/wording";
import { FIXTURE_STATES, seedFor } from "../../mocks/fixtures";
import { Answer } from "./Answer";

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

describe("Answer", () => {
  it("shows the current file's name in bold in an answer's evidence", async () => {
    const user = userEvent.setup();
    render(
      <Answer
        currentFile="src/rpc/client.rs"
        question={question({
          state: "answered",
          answer: {
            answer: "Yes.",
            evidence: [
              { file: "src/rpc/client.rs", side: "head", line: 99 },
              { file: "src/rpc/retry.rs", side: "head", line: 23 },
            ],
          },
        })}
      />,
    );
    await user.click(screen.getByText("Evidence · 2 places"));
    expect(screen.getByText("client.rs")).toHaveClass("font-bold");
    expect(screen.getByText("retry.rs")).not.toHaveClass("font-bold");
  });

  it("wraps long answer words and code", () => {
    const answered = structuredClone((seedFor("resume", "questions") as AskQuestion[])[0]!);
    const long = "x".repeat(500);
    answered.answer!.answer = `${long}\n\n\`\`\`text\n${long}\n\`\`\``;
    render(<Answer question={answered} />);
    const text = screen.getByText(long, { selector: "div" });
    expect(text).toHaveClass("[overflow-wrap:anywhere]");
    expect(text).toHaveClass("[&_pre]:whitespace-pre-wrap");
  });

  it("shows the answer of the resume state whole, labelled as an LLM answer", () => {
    const answered = (seedFor("resume", "questions") as AskQuestion[])[0]!;
    const { container } = render(<Answer question={answered} />);
    expect(container).toHaveTextContent(answered.answer!.answer);
    expect(screen.getByText("LLM answer")).toHaveAccessibleDescription(/LLM/);
    expect(screen.getByText("LLM answer")).toHaveAccessibleDescription(/told not to run/);
    expect(screen.getByText("LLM answer")).not.toHaveAccessibleDescription(/nothing was executed/i);
    expect(container).toHaveTextContent("Evidence · 2 places");
    expect(within(container).queryByRole("link")).toBeNull();
  });

  it.each([
    ["pending", false, "Waiting to start"],
    ["pending", true, "Queued behind your earlier question"],
    ["answering", false, "Being written"],
  ] as const)(
    "shows the %s state (queued %s) in one line with the time since asking",
    (state, queued, text) => {
      const { container } = render(
        <Answer question={question({ state })} queued={queued} now={Date.parse("2026-11-16T10:21:42Z")} />,
      );
      expect(screen.getByRole("status")).toHaveTextContent(new RegExp(`^${text}$`));
      expect(container).toHaveTextContent(`${text} · 1:42`);
      expect(container.querySelector(".animate-spin")).not.toBeNull();
    },
  );

  it("shows a long wait in hours and minutes", () => {
    const { container } = render(
      <Answer question={question({ state: "answering" })} now={Date.parse("2026-11-16T12:35:00Z")} />,
    );
    expect(container).toHaveTextContent("Being written · 2 h 15 min");
  });

  it("shows a question waiting for the usage limit with the reset time", () => {
    const resumes = new Date();
    resumes.setHours(23, 59, 0, 0);
    render(<Answer question={question({ state: "pending", resumes_at: resumes.toISOString() })} />);
    expect(screen.getByRole("status")).toHaveTextContent(
      "Waiting for the LLM usage limit to reset; resumes at 23:59.",
    );
  });

  it("shows a failed question with its plain reason and lets the user ask again", async () => {
    const onAskAgain = vi.fn();
    const { container } = render(
      <Answer
        question={question({ state: "failed", failure: "The LLM usage limit was reached." })}
        onAskAgain={onAskAgain}
      />,
    );
    expect(container).toHaveTextContent("No answer: The LLM usage limit was reached.");
    await userEvent.click(screen.getByRole("button", { name: "Ask again" }));
    expect(onAskAgain).toHaveBeenCalledOnce();
  });

  it("asks again once while pending and shows a rejection", async () => {
    const user = userEvent.setup();
    const pending = deferred();
    const onAskAgain = vi.fn().mockReturnValue(pending.promise);
    render(<Answer question={question({ state: "failed", failure: "x" })} onAskAgain={onAskAgain} />);
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

  it("renders the answer as text, never as HTML or links (T12)", () => {
    const evil = '<img src=x onerror="alert(1)"> [x](javascript:alert(1)) https://evil.example';
    const { container } = render(
      <Answer question={question({ state: "answered", answer: { answer: evil, evidence: [] } })} />,
    );
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector("a")).toBeNull();
    expect(container).toHaveTextContent('<img src=x onerror="alert(1)">');
  });

  it.each(FIXTURE_STATES)("keeps the wording rules in state %s", (state) => {
    const questions = seedFor(state, "questions") as AskQuestion[];
    const { container } = render(
      <>
        {[...questions, question({ state: "failed", failure: "x" }), question({ state: "answering" })].map(
          (q, i) => (
            <Answer key={i} question={q} now={Date.now()} />
          ),
        )}
      </>,
    );
    expectTrustWording(container.textContent ?? "");
  });
});

it("renders only inline and fenced code in answers, leaving HTML and Markdown literal", () => {
  const answer =
    "Use `value` and **literal** [link](https://evil.test).\n```html\n<img src=x onerror=alert(1)>\n```\n<script>alert(1)</script>";
  const { container } = render(
    <Answer question={question({ state: "answered", answer: { answer, evidence: [] } })} />,
  );
  expect(container.querySelector("code")?.textContent).toBe("value");
  expect(container.querySelector("pre code")?.textContent).toBe("<img src=x onerror=alert(1)>\n");
  expect(container.querySelector("img,script,a,strong")).toBeNull();
  expect(container).toHaveTextContent("**literal** [link](https://evil.test)");
});
