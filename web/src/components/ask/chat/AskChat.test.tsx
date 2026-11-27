import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import { expectTrustWording } from "../../../__tests__/wording";
import type { AskAnchor, AskQuestion, Tour } from "../../../api/generated/types";
import { seedFor } from "../../../mocks/fixtures";
import { AskChat, type AskChatProps } from "./AskChat";
import { subjectOf, threadsOf } from "./model";

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

const NOW = Date.parse("2026-11-19T10:10:00Z");

function Harness(over: Partial<AskChatProps> & { questions?: AskQuestion[] }) {
  const [draft, setDraft] = useState("");
  const [replyDrafts, setReplyDrafts] = useState<Record<string, string>>({});
  const props: AskChatProps = {
    describe: (anchor) => subjectOf(anchor, tour),
    threads: threadsOf(over.questions ?? QUESTIONS),
    subject: STEP,
    following: true,
    placeWhat: "this step",
    draft,
    onDraft: setDraft,
    replyDrafts,
    onReplyDraft: (id, edit) =>
      setReplyDrafts((all) => ({
        ...all,
        [id]: typeof edit === "string" ? edit : edit(all[id] ?? ""),
      })),
    now: NOW,
    signInHref: "/auth/github/login",
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
      name: "Step 1.5 · client.rs 99 · src/rpc/client.rs:99 (new code)",
    });
    expect(chip).toHaveTextContent("Step 1.5 · client.rs 99");
    expect(chip).toHaveAccessibleDescription(/src\/rpc\/client\.rs:99 \(new code\)/);
    const subject = within(within(chat()).getByRole("form", { name: "New question" })).getByLabelText(
      "Step 1.5 · client.rs 99 · src/rpc/client.rs:99 (new code)",
    );
    expect(subject).toHaveAccessibleDescription(/src\/rpc\/client\.rs:99/);
    fireEvent.click(chip);
    expect(onGoto).toHaveBeenCalledWith(STEP);
    expect(within(threads[1]!).getByRole("button", { name: "Whole PR" })).toBeVisible();
    expect(threads[0]).toHaveTextContent(/Q1.*answered/);
    expect(threads[1]).toHaveTextContent(/Q2.*answered/);
    expect(chat()).toHaveTextContent("New question");
    const show = within(threads[0]!).getByRole("button", { name: "Show Q1" });
    expect(show).toHaveAttribute("aria-expanded", "false");
    expect(within(threads[0]!).queryByRole("list", { name: "Follow-ups" })).toBeNull();
    fireEvent.click(show);
    expect(show).toHaveAttribute("aria-expanded", "true");
    // The follow-up sits under its first question, without a chip.
    const followUps = within(threads[0]!).getByRole("list", { name: "Follow-ups" });
    expect(followUps).toHaveTextContent("Question b?");
    expect(within(followUps).queryByRole("button", { name: /^Step / })).toBeNull();
    expectTrustWording(chat().textContent ?? "");
  });

  it("says what to do when there is no question yet", () => {
    render(<Harness questions={[]} />);
    expect(chat()).toHaveTextContent("No questions yet.");
  });

  it("keeps each answer's own duration on its existing timestamp", () => {
    const root = q("root", 1, STEP);
    const first = q("first", 2, STEP, { parent_id: root.id });
    const second = q("second", 3, STEP, { parent_id: root.id });
    const pending = q("pending", 4, STEP, { parent_id: root.id, state: "pending", answer: null });
    render(
      <Harness
        questions={[
          { ...root, answered_at: "2026-11-19T10:01:14Z" },
          { ...first, answered_at: "2026-11-19T10:02:08Z" },
          { ...second, answered_at: "2026-11-19T10:03:32Z" },
          pending,
        ]}
      />,
    );
    const thread = within(chat()).getByRole("listitem", { name: "Question: Question root?" });
    expect(thread).toHaveTextContent("waiting");
    const times = thread.querySelectorAll("time");
    expect(times[0]).toHaveAttribute("title", expect.stringMatching(/^Asked by you · .+ · Took 14s$/));
    expect(times[1]).toHaveAttribute("title", expect.stringMatching(/^.+ · Took 8s$/));
    expect(times[2]).toHaveAttribute("title", expect.stringMatching(/^.+ · Took 32s$/));
    expect(times[3]?.title).not.toContain("Took");
    expect(times[3]?.title).toBeTruthy();
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
    [
      503,
      "unavailable",
      "The server is busy. Try again in a moment.",
      "Not sent: The server is busy. Try again in a moment.",
    ],
    [503, "unavailable", "The server is busy.", "Not sent: The server is busy. Try again in a moment."],
  ])(
    "says a refusal (%s %s) plainly, and Try again only when it can help",
    async (status, code, message, said) => {
      const onSend = vi.fn(() => Promise.reject(Object.assign(new Error(message), { status, code })));
      render(<Harness onSend={onSend} />);
      await userEvent.setup().type(field(), "Why?{Enter}");
      const alert = await within(chat()).findByRole("alert");
      expect(alert.textContent?.replace(/\s+/g, " ").trim()).toBe(said);
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

  it("where the whole PR cannot be asked about, × is off with the hint and such a question is not sent", async () => {
    const hint =
      "Questions here are about one comment. Ask about the whole PR in Flows or Files of its analysis.";
    const onUnanchor = vi.fn();
    const { unmount } = render(<Harness onUnanchor={onUnanchor} wholePrUnavailable={hint} />);
    const user = userEvent.setup();
    const remove = within(chat()).getByRole("button", { name: "Remove the subject: ask about the whole PR" });
    expect(remove).toHaveAttribute("aria-disabled", "true");
    await user.click(remove);
    expect(onUnanchor).not.toHaveBeenCalled();
    unmount();
    const onSend = vi.fn();
    render(<Harness subject={{ kind: "pr" }} following={false} onSend={onSend} wholePrUnavailable={hint} />);
    await user.type(field(), "Is this safe?");
    await user.click(within(chat()).getByRole("button", { name: "Ask" }));
    expect(onSend).not.toHaveBeenCalled();
    expect(field()).toHaveAccessibleDescription(new RegExp(hint.slice(0, 40)));
    expect(field()).toHaveValue("Is this safe?");
  });

  it("a follow-up goes into its thread; Esc closes its field and keeps the text, then Esc hides the chat", async () => {
    const onReply = vi.fn(() => Promise.resolve());
    const onHide = vi.fn();
    render(<Harness onReply={onReply} onHide={onHide} />);
    const user = userEvent.setup();
    const thread = within(chat())
      .getAllByRole("listitem")
      .find((li) => li.dataset.thread === "c")!;
    const older = within(chat()).getByRole("listitem", { name: "Question: Question a?" });
    await user.click(within(older).getByRole("button", { name: "Show Q1" }));
    await user.click(within(older).getByRole("button", { name: "Follow up on Q1" }));
    await user.type(within(older).getByRole("textbox", { name: /^Follow-up to Q1/ }), "kept on Q1");
    await user.click(within(thread).getByRole("button", { name: "Follow up on Q2" }));
    expect(within(older).queryByRole("textbox")).toBeNull();
    expect(within(thread).getByRole("button", { name: "Send follow-up" })).toBeVisible();
    expect(field()).toBeVisible();
    const reply = within(thread).getByRole("textbox", { name: /^Follow-up to Q2/ });
    expect(reply).toHaveFocus();
    await user.type(reply, "And later?");
    await user.keyboard("{Escape}");
    expect(onHide).not.toHaveBeenCalled();
    expect(within(thread).queryByRole("textbox")).toBeNull();
    await user.click(within(thread).getByRole("button", { name: "Follow up on Q2" }));
    expect(within(thread).getByRole("textbox", { name: /^Follow-up to Q2/ })).toHaveValue("And later?");
    await user.keyboard("{Enter}");
    expect(onReply).toHaveBeenCalledWith(expect.objectContaining({ id: "c" }), "And later?");
    await user.click(within(older).getByRole("button", { name: "Follow up on Q1" }));
    expect(within(older).getByRole("textbox", { name: /^Follow-up to Q1/ })).toHaveValue("kept on Q1");
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
    await user.click(within(a!).getByRole("button", { name: "Show Q1" }));
    await user.click(within(c!).getByRole("button", { name: "Show Q2" }));
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
    expect(within(thread).getByText("new", { exact: true })).toBeVisible();
    expect(within(thread).getByRole("list", { name: "Follow-ups" })).toBeVisible();
  });

  it("a 401 offers Sign in again and keeps the text", async () => {
    const onSend = vi.fn(() =>
      Promise.reject(
        Object.assign(new Error("Sign in to continue."), { status: 401, code: "unauthenticated" }),
      ),
    );
    render(<Harness onSend={onSend} />);
    await userEvent.setup().type(field(), "Why?{Enter}");
    const alert = await within(chat()).findByRole("alert");
    expect(alert).toHaveTextContent("Not sent: Sign in to continue.");
    expect(within(alert).getByRole("link", { name: "Sign in again" })).toHaveAttribute(
      "href",
      "/auth/github/login",
    );
    expect(field()).toHaveValue("Why?");
  });

  it("names the question a waiting card is queued behind", () => {
    render(
      <Harness
        questions={[
          q("a", 8, STEP, { state: "answering", answer: null }),
          q("b", 9, { kind: "pr" }, { state: "pending", answer: null }),
        ]}
      />,
    );
    const [a, b] = within(chat())
      .getAllByRole("listitem")
      .filter((li) => li.dataset.thread);
    expect(a).toHaveTextContent("◔ answering");
    expect(b).toHaveTextContent("◌ waits for Q1");
  });

  it("shows when a waiting question starts answering while the chat is open", async () => {
    const pending = q("a", 8, STEP, { state: "pending", answer: null });
    const { rerender } = render(<Harness questions={[pending]} />);
    expect(chat()).toHaveTextContent("◌ waiting");
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
    rerender(<Harness questions={[{ ...pending, state: "answering" }]} />);
    await waitFor(() =>
      expect(within(chat()).getByRole("status")).toHaveTextContent(/◔ answering since \d{2}:\d{2}/),
    );
  });

  it("a follow-up draft is the caller's: it is there again when the chat comes back", async () => {
    const user = userEvent.setup();
    const { unmount } = render(<Harness replyDrafts={{ c: "kept" }} onReplyDraft={vi.fn()} />);
    const thread = within(chat())
      .getAllByRole("listitem")
      .find((li) => li.dataset.thread === "c")!;
    await user.click(within(thread).getByRole("button", { name: "Follow up on Q2" }));
    expect(within(thread).getByRole("textbox", { name: /^Follow-up to Q2/ })).toHaveValue("kept");
    unmount();
  });

  it("an answer arriving out of view shows New answer, which scrolls to it", async () => {
    const user = userEvent.setup();
    const pending = q("c", 3, { kind: "pr" }, { state: "answering", answer: null });
    const { rerender } = render(<Harness questions={[...QUESTIONS.slice(0, 2), pending]} />);
    const list = within(chat()).getByRole("region", { name: "Conversation, oldest first" });
    // The reader has scrolled up: the thread is out of view.
    Object.defineProperties(list, {
      scrollHeight: { configurable: true, value: 1000 },
      clientHeight: { configurable: true, value: 200 },
    });
    list.scrollTop = 0;
    fireEvent.scroll(list);
    const item = chat().querySelector<HTMLElement>('[data-thread="c"]')!;
    item.getBoundingClientRect = () => ({ top: 900, bottom: 960 }) as DOMRect;
    list.getBoundingClientRect = () => ({ top: 0, bottom: 200 }) as DOMRect;
    let visible = false;
    const scrolled = vi.fn(() => {
      visible = true;
    });
    const onSeen = vi.fn();
    // Use the actual answer's location rather than the card's.
    const query = item.querySelectorAll.bind(item);
    vi.spyOn(item, "querySelectorAll").mockImplementation((selector) => {
      const matches = query(selector);
      const outcome = matches[0] as HTMLElement | undefined;
      if (outcome) {
        outcome.scrollIntoView = scrolled;
        outcome.getBoundingClientRect = () =>
          (visible ? { top: 20, bottom: 80, height: 60 } : { top: 900, bottom: 960, height: 60 }) as DOMRect;
      }
      return matches;
    });
    rerender(
      <Harness
        unread={new Set([pending.id])}
        onSeen={onSeen}
        questions={[
          ...QUESTIONS.slice(0, 2),
          { ...pending, state: "answered", answer: { answer: "Done.", evidence: [] } },
        ]}
      />,
    );
    const jump = await within(chat()).findByRole("button", { name: "↓ New answer" });
    await user.click(jump);
    expect(scrolled).toHaveBeenCalled();
    await waitFor(() => expect(within(chat()).queryByRole("button", { name: "↓ New answer" })).toBeNull());
    expect(onSeen).toHaveBeenCalledWith([pending.id]);
  });

  it("the top-left corner resizes width and height together by dragging; Enter on a separator still resets", () => {
    const onSize = vi.fn();
    render(<Harness onSize={onSize} />);
    const corner = chat().querySelector<HTMLElement>("[data-resize-corner]")!;
    fireEvent.pointerDown(corner, { clientX: 500, clientY: 300, pointerId: 1 });
    fireEvent.pointerMove(corner, { clientX: 470, clientY: 260, pointerId: 1 });
    expect(onSize).toHaveBeenLastCalledWith({ width: 450, height: 540, full: false });
    fireEvent.pointerUp(corner, { pointerId: 1 });
  });
});
