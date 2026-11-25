import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { AskAnchor, AskQuestion, Tour } from "../../../api/generated/types";
import { seedFor } from "../../../mocks/fixtures";
import { chatGeometry } from "../../../screens/review/chatGeometry";
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
const STEP: AskAnchor = { kind: "step", step_id: "f1.5", lines: { start_line: 99, end_line: 99 } };

function chat(questions: AskQuestion[]) {
  const props: AskChatProps = {
    describe: (anchor) => subjectOf(anchor, tour),
    threads: threadsOf(questions),
    subject: STEP,
    following: true,
    placeWhat: "this step",
    draft: "",
    onDraft: vi.fn(),
    replyDrafts: {},
    onReplyDraft: vi.fn(),
    now: Date.parse("2026-11-19T10:10:00Z"),
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
    about: "your private chat",
    unread: new Set(),
    focusThread: null,
  };
  render(<AskChat {...props} />);
  return screen.getByRole("dialog", { name: "Ask" });
}

describe("probe: Ask window v1.4", () => {
  it("collapsed one-line summary of a remark thread names the question, not the quote (U53)", () => {
    const quoted = "> LLM remark (Red flag, line 99): The retry may double send.\n\nIs the retry idempotent?";
    const dialog = chat([q("a", 1, STEP, { text: quoted }), q("b", 2, { kind: "pr" })]);
    const card = within(dialog)
      .getAllByRole("listitem")
      .find((li) => li.dataset.thread === "a")!;
    expect(within(card).getByRole("button", { name: "Show Q1" })).toHaveAttribute("aria-expanded", "false");
    // Expected per U53 (`Q2 · Step 1.4 · "Why is the cache …"`): the reader's question is the summary.
    // The summary is one truncated line (`truncate`): what comes first is what the reader sees.
    const summary = card.querySelector<HTMLElement>("span.truncate")!;
    expect(summary.textContent).toMatch(/^“Is the retry idempotent\?/);
  });

  it("a collapsed card whose follow-up is still answering does not say only 'answered'", () => {
    const dialog = chat([
      q("a", 1, STEP),
      q("b", 2, STEP, { parent_id: "a", state: "answering", answer: null }),
      q("c", 3, { kind: "pr" }),
    ]);
    const card = within(dialog)
      .getAllByRole("listitem")
      .find((li) => li.dataset.thread === "a")!;
    // Expanded by default (not all answered); collapse it as a reader would.
    fireEvent.click(within(card).getByRole("button", { name: "Hide Q1" }));
    expect(card.textContent).toMatch(/answering/);
  });
});

describe("probe: chatGeometry presets", () => {
  it("Top left keeps the window below the review top bar (mockup clamps top at the bar)", () => {
    const view = { width: 1600, height: 900 };
    const ws = { top: 48, right: 1600, bottom: 900, width: 1600 };
    const g = chatGeometry(view, ws, null, null, {
      width: null,
      height: null,
      full: false,
      position: "top-left",
    });
    const top = view.height - g.bottom - g.height;
    expect(top).toBeGreaterThanOrEqual(ws.top);
  });

  it("Full height of a moved window stays below the review top bar", () => {
    const view = { width: 1600, height: 900 };
    const ws = { top: 48, right: 1600, bottom: 900, width: 1600 };
    const g = chatGeometry(view, ws, null, null, {
      width: null,
      height: null,
      full: true,
      position: "bottom-left",
    });
    const top = view.height - g.bottom - g.height;
    expect(top).toBeGreaterThanOrEqual(ws.top);
  });
});
