import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import { expectTrustWording } from "../../../__tests__/wording";
import type { AskAnchor, AskQuestion, Tour } from "../../../api/generated/types";
import { seedFor } from "../../../mocks/fixtures";
import { AskChat, type AskChatProps } from "./AskChat";
import { threadsOf } from "./model";

const tour = seedFor("default", "tour") as Tour;
const q = (id: string, minute: number, anchor: AskAnchor, extra: Partial<AskQuestion> = {}): AskQuestion => ({
  id,
  mine: true,
  anchor,
  text: `Question ${id}?`,
  state: "answered",
  answer: { answer: `Answer to ${id}.`, evidence: [] },
  failure: null,
  created_at: `2026-11-19T10:${String(minute).padStart(2, "0")}:00Z`,
  ...extra,
});
const STEP: AskAnchor = { kind: "step", step_id: "f1.5", lines: null };
const QUESTIONS = [q("a", 1, STEP), q("b", 2, STEP, { parent_id: "a" }), q("c", 3, { kind: "pr" })];

function Harness(over: Partial<AskChatProps> & { questions?: AskQuestion[] }) {
  const [draft, setDraft] = useState("");
  const props: AskChatProps = {
    tour,
    threads: threadsOf(over.questions ?? QUESTIONS),
    subject: STEP,
    following: true,
    placeWhat: "this step",
    draft,
    onDraft: setDraft,
    onSend: vi.fn(),
    onReply: vi.fn(),
    onAskAgain: vi.fn(),
    onUnanchor: vi.fn(),
    onUsePlace: vi.fn(),
    onGoto: vi.fn(),
    onHide: vi.fn(),
    size: { width: null, height: null, full: false },
    geometry: {
      right: 8,
      bottom: 8,
      width: 420,
      height: 500,
      minWidth: 320,
      maxWidth: 900,
      minHeight: 240,
      maxHeight: 700,
    },
    onSize: vi.fn(),
    about: "your private chat about PR #482",
    unread: new Set(),
    focusThread: null,
    ...over,
  };
  return <AskChat {...props} />;
}

const chat = () => screen.getByRole("dialog", { name: "Ask" });
const field = () => within(chat()).getByRole("textbox", { name: "Your question" });

describe("AskChat (proposal AC3–AC10)", () => {
  it("is a non-modal dialog with the threads oldest first, each first question with its subject chip", () => {
    const onGoto = vi.fn();
    render(<Harness onGoto={onGoto} />);
    expect(chat()).toHaveAttribute("aria-modal", "false");
    expect(chat()).toHaveAccessibleDescription(/your private chat about PR #482/);
    const threads = within(chat())
      .getAllByRole("listitem")
      .filter((li) => li.dataset.thread);
    expect(threads.map((t) => t.dataset.thread)).toEqual(["a", "c"]);
    const chip = within(threads[0]!).getByRole("button", {
      name: /^Flows src\/rpc\/client\.rs:99 flow 1 · step 5/,
    });
    fireEvent.click(chip);
    expect(onGoto).toHaveBeenCalledWith(STEP);
    expect(within(threads[1]!).getByRole("button", { name: "Whole PR" })).toBeVisible();
    // The follow-up sits under its first question, without a chip.
    const followUps = within(threads[0]!).getByRole("list", { name: "Follow-ups" });
    expect(followUps).toHaveTextContent("Question b?");
    expect(within(followUps).queryByRole("button", { name: /^Flows/ })).toBeNull();
    expectTrustWording(chat().textContent ?? "");
  });

  it("says what to do when there is no question yet", () => {
    render(<Harness questions={[]} />);
    expect(chat()).toHaveTextContent("No questions yet.");
  });

  it("Enter sends the question about the subject; Shift+Enter adds a line; an empty field says so", async () => {
    const onSend = vi.fn(() => Promise.resolve());
    render(<Harness onSend={onSend} />);
    const user = userEvent.setup();
    await user.click(within(chat()).getByRole("button", { name: "Ask" }));
    expect(within(chat()).getByRole("alert")).toHaveTextContent("Write a question first.");
    await user.type(field(), "Why{Shift>}{Enter}{/Shift}twice?");
    expect(onSend).not.toHaveBeenCalled();
    await user.keyboard("{Enter}");
    expect(onSend).toHaveBeenCalledWith("Why\ntwice?");
    expect(field()).toHaveValue("");
  });

  it("keeps the text and says why when sending fails", async () => {
    const onSend = vi.fn(() => Promise.reject(new Error("The server is busy.")));
    render(<Harness onSend={onSend} />);
    const user = userEvent.setup();
    await user.type(field(), "Why?{Enter}");
    expect(await within(chat()).findByRole("alert")).toHaveTextContent(
      "Not sent: The server is busy. Try again.",
    );
    expect(field()).toHaveValue("Why?");
  });

  it.each([
    [
      409,
      "analysis_not_finished",
      "The analysis is not finished yet.",
      "Not sent: The analysis is not finished yet.",
    ],
    [
      404,
      "not_found",
      "This flow is not analysed in this review.",
      "Not sent: This flow is not analysed in this review.",
    ],
    [503, "unavailable", "The server is busy.", "Not sent: The server is busy. Try again in a moment."],
  ])(
    "says a refusal (%s %s) plainly, and Try again only when it can help",
    async (status, code, message, said) => {
      const onSend = vi.fn(() => Promise.reject(Object.assign(new Error(message), { status, code })));
      render(<Harness onSend={onSend} />);
      await userEvent.setup().type(field(), "Why?{Enter}");
      expect(await within(chat()).findByRole("alert")).toHaveTextContent(
        new RegExp(`^${said.replace(/[.?]/g, "\\$&")}$`),
      );
    },
  );

  it("names the subject next to the field; × asks about the whole PR, and the place can come back", async () => {
    const onUnanchor = vi.fn();
    const { unmount } = render(<Harness onUnanchor={onUnanchor} />);
    expect(field()).toHaveAttribute("placeholder", "Ask about this step…");
    await userEvent
      .setup()
      .click(within(chat()).getByRole("button", { name: "Remove the subject: ask about the whole PR" }));
    expect(onUnanchor).toHaveBeenCalled();
    unmount();
    const onUsePlace = vi.fn();
    render(<Harness subject={{ kind: "pr" }} following={false} onUsePlace={onUsePlace} />);
    expect(field()).toHaveAttribute("placeholder", "Ask about the whole PR…");
    await userEvent.setup().click(within(chat()).getByRole("button", { name: "about this step instead" }));
    expect(onUsePlace).toHaveBeenCalled();
  });

  it("a follow-up goes into its thread; Esc closes its field and keeps the text, then Esc hides the chat", async () => {
    const onReply = vi.fn(() => Promise.resolve());
    const onHide = vi.fn();
    render(<Harness onReply={onReply} onHide={onHide} />);
    const user = userEvent.setup();
    const thread = within(chat())
      .getAllByRole("listitem")
      .find((li) => li.dataset.thread === "c")!;
    await user.click(within(thread).getByRole("button", { name: "Ask a follow-up" }));
    const reply = within(thread).getByRole("textbox", { name: /^Follow-up in this thread/ });
    expect(reply).toHaveFocus();
    await user.type(reply, "And later?");
    await user.keyboard("{Escape}");
    expect(onHide).not.toHaveBeenCalled();
    expect(within(thread).queryByRole("textbox")).toBeNull();
    await user.click(within(thread).getByRole("button", { name: "Ask a follow-up" }));
    expect(within(thread).getByRole("textbox", { name: /^Follow-up in this thread/ })).toHaveValue(
      "And later?",
    );
    await user.keyboard("{Enter}");
    expect(onReply).toHaveBeenCalledWith(expect.objectContaining({ id: "c" }), "And later?");
    await user.click(field());
    await user.keyboard("{Escape}");
    expect(onHide).toHaveBeenCalled();
  });

  it("offers Use as review comment only on answers about lines, and Ask again on a failed answer", async () => {
    const onUseAsComment = vi.fn();
    const onAskAgain = vi.fn(() => Promise.resolve());
    render(
      <Harness
        onUseAsComment={onUseAsComment}
        onAskAgain={onAskAgain}
        questions={[
          q("a", 1, STEP),
          q("c", 2, { kind: "pr" }),
          q("d", 3, { kind: "pr" }, { state: "failed", answer: null, failure: "the LLM did not finish." }),
        ]}
      />,
    );
    const user = userEvent.setup();
    const [a, c, d] = within(chat())
      .getAllByRole("listitem")
      .filter((li) => li.dataset.thread);
    await user.click(within(a!).getByRole("button", { name: "Use as review comment" }));
    expect(onUseAsComment).toHaveBeenCalledWith(expect.objectContaining({ id: "a" }));
    expect(within(c!).queryByRole("button", { name: "Use as review comment" })).toBeNull();
    expect(d).toHaveTextContent("No answer: the LLM did not finish.");
    await user.click(within(d!).getByRole("button", { name: "Ask again" }));
    expect(onAskAgain).toHaveBeenCalledWith(expect.objectContaining({ id: "d" }));
  });

  it("Full height and the size separators change the size; Enter resets", async () => {
    const onSize = vi.fn();
    render(<Harness onSize={onSize} />);
    const user = userEvent.setup();
    await user.click(within(chat()).getByRole("button", { name: "Full height" }));
    expect(onSize).toHaveBeenLastCalledWith({ width: null, height: null, full: true });
    const height = within(chat()).getByRole("separator", { name: /^Chat height/ });
    expect(height).toHaveAttribute("aria-valuenow", "500");
    height.focus();
    await user.keyboard("{ArrowUp}");
    expect(onSize).toHaveBeenLastCalledWith({ width: null, height: 520, full: false });
    const width = within(chat()).getByRole("separator", { name: /^Chat width/ });
    width.focus();
    await user.keyboard("{ArrowLeft}");
    expect(onSize).toHaveBeenLastCalledWith({ width: 440, height: null, full: false });
    await user.keyboard("{Enter}");
    expect(onSize).toHaveBeenLastCalledWith({ width: null, height: null, full: false });
  });

  it("marks threads with an answer not seen yet", () => {
    render(<Harness unread={new Set(["b"])} />);
    const thread = within(chat())
      .getAllByRole("listitem")
      .find((li) => li.dataset.thread === "a")!;
    expect(thread).toHaveTextContent(/^new/);
  });
});
