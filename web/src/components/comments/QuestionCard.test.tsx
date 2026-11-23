import { act, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState, type ComponentProps } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { expectTrustWording } from "../../__tests__/wording";
import type { CommentItem, DecisionBatchRow } from "../../api/generated/types";
import { context, isRefused, listItems, previewEffects, resetReviewLoop } from "../../mocks/review-loop";
import type { EffectsPreviewState } from "./decisions";
import { COMMENT_HINT } from "./DecisionForm";
import { QuestionCard, type ContextState } from "./QuestionCard";

const NOW = new Date("2026-11-18T09:27:00Z");
const byNo = (no: number) => listItems().find((i) => i.no === no)!;

/** The draft planned at once by the mocks' server model (the screen asks RC16). */
function planOf(rows: DecisionBatchRow[] | null): EffectsPreviewState {
  if (rows === null) return { state: "idle" };
  const answer = previewEffects(rows);
  return isRefused(answer)
    ? { state: "error", message: answer[2] }
    : { state: "ready", items: answer, updating: false };
}

function Planned(props: ComponentProps<typeof QuestionCard>) {
  const [rows, setRows] = useState<DecisionBatchRow[] | null>(null);
  return <QuestionCard {...props} preview={planOf(rows)} onDraft={setRows} />;
}

function show(item: CommentItem, props: Partial<Parameters<typeof QuestionCard>[0]> = {}) {
  const ctx = context(item.no, null, null);
  const state: ContextState = ctx ? { state: "ready", value: ctx } : { state: "none" };
  const handlers = { onDecide: vi.fn(), onSkip: vi.fn(), onAsk: vi.fn(), onMoreLines: vi.fn() };
  const rendered = render(
    <Planned
      item={item}
      position={{ index: 2, total: 9 }}
      prNumber={482}
      purpose="Retries a timed-out RPC call."
      context={state}
      flowHref={(f) => `/analyses/a#${f.step}`}
      filesHref="/analyses/a/files"
      prPageHref="/pr/acme/node-client/482"
      onNavigate={vi.fn()}
      askAnswers={item.ask_answers}
      busy={false}
      error={null}
      now={NOW}
      storageKey="alice:test"
      {...handlers}
      {...props}
    />,
  );
  const rerender = (next: CommentItem) =>
    rendered.rerender(
      <Planned
        item={next}
        position={{ index: 2, total: 9 }}
        prNumber={482}
        purpose="Retries a timed-out RPC call."
        context={state}
        flowHref={(f) => `/analyses/a#${f.step}`}
        filesHref="/analyses/a/files"
        prPageHref="/pr/acme/node-client/482"
        onNavigate={vi.fn()}
        askAnswers={next.ask_answers}
        busy={false}
        error={null}
        now={NOW}
        storageKey="alice:test"
        {...handlers}
        {...props}
      />,
    );
  return { ...handlers, rerender };
}

beforeEach(() => {
  resetReviewLoop();
  window.localStorage.clear();
});

describe("QuestionCard (plan §3.2)", () => {
  it("shows context, evidence, the LLM assessment and the recommended option first, focused heading", () => {
    show(byNo(14));
    const heading = screen.getByRole("heading", { level: 2, name: /C14 · Retry may resend/ });
    expect(heading).toHaveFocus();
    expect(screen.getByText(/C14 · 2 of 9 · thepastaclaw · bot · inline thread/)).toBeVisible();
    const options = within(screen.getByRole("radiogroup", { name: "Decision on C14" })).getAllByRole("radio");
    expect(options[0]).toHaveAccessibleName("Fix here (Recommended)");
    expect(options.at(-1)).toHaveAccessibleName(/^Accept despite the gate/);
    expect(options[0]).toBeChecked();
    expect(screen.getByText("The LLM says")).toBeVisible();
    expect(screen.getByRole("button", { name: "LLM assessment" })).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText(COMMENT_HINT)).toBeVisible();
    expectTrustWording(document.body.textContent ?? "");
  });

  it("renders the comment as text: only https://github.com links become links (T24)", () => {
    show(byNo(14));
    const quote = document.querySelector("[data-comment-text]")!;
    const links = within(quote as HTMLElement).getAllByRole("link");
    expect(links.map((a) => a.getAttribute("href"))).toEqual([
      "https://github.com/acme/node-client/blob/main/docs/nonce.md",
    ]);
    expect(quote.textContent).toContain("https://evil.example/phish");
  });

  it("notes elsewhere are named icons opening their place in a new tab", () => {
    show(byNo(14));
    const notes = within(screen.getByRole("list", { name: "Notes about this code elsewhere" })).getAllByRole(
      "link",
    );
    expect(notes.length).toBeGreaterThanOrEqual(4);
    for (const note of notes) {
      expect(note).toHaveAttribute("target", "_blank");
      expect(note).toHaveAccessibleName(/\(new tab\)$/);
    }
    expect(
      screen.getByRole("link", { name: /Flow 1 · Send an RPC request with retry · step 2/ }),
    ).toHaveAttribute("target", "_blank");
  });

  it("without an analysis: 'No analysis yet' with the way to the PR page", () => {
    const item = byNo(11);
    const value = { ...context(11, null, null)!, analysis: null, analysis_head: null, in_flows: [] };
    show(item, { context: { state: "ready", value } });
    expect(screen.getByText(/No analysis yet/)).toBeVisible();
    expect(screen.getByRole("link", { name: "Start on the PR page" })).toHaveAttribute(
      "href",
      "/pr/acme/node-client/482",
    );
  });

  it("gate item: Accept despite the gate locks Resolve ticked and needs a reason, posted as the reply", async () => {
    const { onDecide } = show(byNo(14));
    const resolve = screen.getByRole("checkbox", { name: "Resolve this thread" });
    expect(resolve).toBeChecked();
    expect(resolve).toHaveAccessibleDescription(/Gate item \(G-FUNDS\): submitting with this ticked/);
    await userEvent.click(screen.getByRole("radio", { name: /Accept despite the gate/ }));
    expect(resolve).toBeChecked();
    expect(resolve).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "Decide" }));
    expect(screen.getByRole("alert")).toHaveTextContent("A reason is required: it is posted as the reply.");
    expect(onDecide).not.toHaveBeenCalled();
    await userEvent.type(screen.getByLabelText("Reason (posted as the public reply)"), "Tracked in #455.");
    expect(screen.getByRole("group", { name: "Preview of the reply on GitHub" })).toHaveTextContent(
      "Per maintainer decision, accepted despite G-FUNDS: Tracked in #455.",
    );
    await userEvent.click(screen.getByRole("button", { name: "Decide" }));
    expect(onDecide).toHaveBeenCalledWith(
      expect.objectContaining({
        kind: "reject",
        reason: "Tracked in #455.",
        override: { type: "blocker_gate", gate: "G-FUNDS" },
        resolve_ticked: true,
      }),
    );
  });

  it("unticking Resolve replies and leaves the thread open; the comment goes with the decision", async () => {
    const { onDecide } = show(byNo(22));
    await userEvent.click(screen.getByRole("checkbox", { name: "Resolve this thread" }));
    expect(screen.getByText("Thread stays open (not ticked).")).toBeVisible();
    await userEvent.type(screen.getByLabelText("Your comment (optional)"), "keep the test small");
    await userEvent.click(screen.getByRole("button", { name: "Decide" }));
    expect(onDecide).toHaveBeenCalledWith(
      expect.objectContaining({ kind: "fix_here", resolve_ticked: false, comment: "keep the test small" }),
    );
  });

  it("Create follow-up PR asks for no link", async () => {
    const { onDecide } = show(byNo(11));
    await userEvent.click(screen.getByRole("radio", { name: /Create follow-up PR/ }));
    expect(screen.getByText(/no link is needed, it does not exist yet/)).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Decide" }));
    expect(onDecide).toHaveBeenCalledWith(
      expect.objectContaining({
        kind: "follow_up_pr",
        target: { type: "follow_up_pr", title: expect.any(String) },
      }),
    );
  });

  it("keys: 1–8 choose, Enter decides, a asks, s skips; not while typing", async () => {
    const handlers = show(byNo(11));
    fireEvent.keyDown(document.body, { key: "2" });
    expect(screen.getByRole("radio", { name: /^Fix here/ })).toBeChecked();
    fireEvent.keyDown(document.body, { key: "a" });
    expect(handlers.onAsk).toHaveBeenCalledTimes(1);
    fireEvent.keyDown(screen.getByLabelText("Your comment (optional)"), { key: "s" });
    expect(handlers.onSkip).not.toHaveBeenCalled();
    fireEvent.keyDown(document.body, { key: "s" });
    expect(handlers.onSkip).toHaveBeenCalledTimes(1);
    fireEvent.keyDown(document.body, { key: "Enter" });
    expect(handlers.onDecide).toHaveBeenCalledWith(expect.objectContaining({ kind: "fix_here" }));
  });

  it("an item that changes while the reader typed keeps the input; Decide waits until it is kept", async () => {
    const item = byNo(11);
    const { onDecide, rerender } = show(item);
    await userEvent.type(screen.getByLabelText("Your comment (optional)"), "keep it small");
    act(() => rerender({ ...item, revision: item.revision + 1 }));
    expect(screen.getByLabelText("Your comment (optional)")).toHaveValue("keep it small");
    await userEvent.click(screen.getByRole("button", { name: "Decide" }));
    expect(onDecide).not.toHaveBeenCalled();
    expect(screen.getByRole("alert")).toHaveTextContent(/C11 changed since you started/);
    await userEvent.click(screen.getByRole("button", { name: "Keep my input" }));
    await userEvent.click(screen.getByRole("button", { name: "Decide" }));
    expect(onDecide).toHaveBeenCalledWith(
      expect.objectContaining({ item_revision: item.revision + 1, comment: "keep it small" }),
    );
  });

  it("an untouched card follows the changed item and its new recommendation", () => {
    const item = byNo(11);
    const { rerender } = show(item);
    const changed: CommentItem = {
      ...item,
      revision: item.revision + 1,
      assessment: {
        ...item.assessment!,
        recommendation: { kind: "fix_here", reason: "Small.", reason_class: "small", target: null },
      },
    };
    act(() => rerender(changed));
    expect(screen.getByRole("radio", { name: "Fix here (Recommended)" })).toBeChecked();
    expect(screen.queryByRole("button", { name: "Keep my input" })).toBeNull();
  });

  it("a fix against an 'out' necessity verdict carries the scope override", async () => {
    const { onDecide } = show(byNo(11));
    await userEvent.click(screen.getByRole("radio", { name: /Fix here/ }));
    expect(screen.getByText(/scope override/)).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Decide" }));
    expect(onDecide).toHaveBeenCalledWith(expect.objectContaining({ override: { type: "necessity_gate" } }));
  });

  it("Ask: the answers count and an unavailable Ask with the reason", () => {
    show(byNo(11), { onAsk: undefined, askUnavailable: "Ask needs an analysis." });
    expect(screen.getByRole("button", { name: "1 Ask answer" })).toBeVisible();
    const ask = screen.getByRole("button", { name: "Ask about this comment" });
    expect(ask).toHaveAttribute("aria-disabled", "true");
    expect(ask).toHaveAccessibleDescription("Ask needs an analysis.");
  });

  it("a re-asked repeat says why, with the evidence diffpilot read", () => {
    show(byNo(21), {
      reask: {
        id: "e",
        pr: { repo: { owner: "acme", name: "node-client" }, number: 482 },
        item_no: 21,
        source_item_no: 3,
        source_decision: "d",
        source_kind: "reject",
        kind: "asked_again",
        reason: "code_changed",
        evidence: {
          url: null,
          body_hash_before: null,
          body_hash_after: null,
          commit: "7d8e9f0aaaa",
          lines: { start_line: 90, end_line: 91 },
        },
        effects_pending: 0,
        at: "2026-11-18T08:46:00Z",
      },
    });
    expect(screen.getByText("The code at that place changed after your decision.")).toBeVisible();
    expect(screen.getByText(/Asked again — repeat of C3/)).toBeVisible();
  });
});
