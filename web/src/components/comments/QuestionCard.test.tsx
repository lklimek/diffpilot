import { act, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ComponentProps } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { Planned } from "../../__tests__/planned";
import { expectTrustWording } from "../../__tests__/wording";
import type { CommentItem } from "../../api/generated/types";
import { context, listItems, resetReviewLoop } from "../../mocks/review-loop";
import type { EffectsPreviewState } from "./decisions";
import { COMMENT_HINT } from "./DecisionForm";
import { QuestionCard, type ContextState } from "./QuestionCard";

const NOW = new Date("2026-11-18T09:27:00Z");
const byNo = (no: number) => listItems().find((i) => i.no === no)!;

type CardProps = Omit<ComponentProps<typeof QuestionCard>, "preview" | "onDraft"> &
  Partial<Pick<ComponentProps<typeof QuestionCard>, "preview" | "onDraft">>;

/** The card with its drafts planned at once by the mocks' server model, unless `preview` is given. */
function Card(props: CardProps) {
  return <Planned>{(plan) => <QuestionCard {...plan} {...props} />}</Planned>;
}

function show(item: CommentItem, props: Partial<Parameters<typeof QuestionCard>[0]> = {}) {
  const ctx = context(item.no, null, null);
  const state: ContextState = ctx ? { state: "ready", value: ctx } : { state: "none" };
  const handlers = { onDecide: vi.fn(), onSkip: vi.fn(), onAsk: vi.fn(), onMoreLines: vi.fn() };
  const rendered = render(
    <Card
      item={item}
      position={{ index: 2, total: 9 }}
      prNumber={482}
      purpose="Retries a timed-out RPC call."
      context={state}
      flowHref={(f) => `/analyses/a#${f.step}`}
      filesHref="/analyses/a/files"
      prPageHref="/pr/acme/node-client/482"
      prHead={item.assessment?.head ?? item.first_seen_head}
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
      <Card
        item={next}
        position={{ index: 2, total: 9 }}
        prNumber={482}
        purpose="Retries a timed-out RPC call."
        context={state}
        flowHref={(f) => `/analyses/a#${f.step}`}
        filesHref="/analyses/a/files"
        prPageHref="/pr/acme/node-client/482"
        prHead={next.assessment?.head ?? next.first_seen_head}
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
  it("renders the queued assessment as a paragraph", async () => {
    show({ ...byNo(14), assessment_state: { state: "queued" } });
    await userEvent.click(screen.getByRole("button", { name: "LLM assessment" }));
    const note = screen.getByText("Waiting for assessment.");
    expect(note.closest("p")).toHaveClass("m-0");
  });

  it("shows context, evidence, the LLM assessment and the recommended option first, focused heading", () => {
    show(byNo(14));
    const heading = screen.getByRole("heading", { level: 2, name: /C14 · Retry may resend/ });
    expect(heading).toHaveFocus();
    expect(screen.getByText(/C14 · 2 of 9 · thepastaclaw · bot · inline thread/)).toBeVisible();
    const options = within(screen.getByRole("radiogroup", { name: "Decision on C14" })).getAllByRole("radio");
    expect(options[0]).toHaveAccessibleName("Fix here (Recommended)");
    expect(options.at(-1)).toHaveAccessibleName(/^Accept despite the gate/);
    expect(options[0]).toBeChecked();
    expect(screen.getByRole("button", { name: "LLM assessment" })).toHaveAttribute("aria-expanded", "false");
    expect(screen.getByText("The LLM says")).not.toBeVisible();
    expect(screen.getByText(COMMENT_HINT)).toBeVisible();
    expectTrustWording(document.body.textContent ?? "");
  });

  it("keeps keyboard focus in Queue Status when the current question changes", () => {
    const { rerender } = show(byNo(14));
    render(
      <div data-queue-status>
        <button type="button">Queue status</button>
      </div>,
    );
    const status = screen.getByRole("button", { name: "Queue status" });
    status.focus();
    rerender(byNo(15));
    expect(screen.getByRole("heading", { name: /C15 ·/ })).toBeVisible();
    expect(status).toHaveFocus();
  });

  it("names a round only for a comment its round's review raised; any other where GitHub says it was written", () => {
    const header = () => screen.getByText(/^C22 · 2 of 9/);
    const c22 = byNo(22);
    const { rerender } = show({
      ...c22,
      first_round: 1,
      written: { round: 1, commit: "a797ee1c0ffee000000000000000000000000000", at: "2026-10-26T12:00:00Z" },
    });
    expect(header()).toHaveTextContent(/raised in round 1 on a797ee1, 26 Oct 2026/);
    expect(header()).not.toHaveTextContent(c22.first_seen_head.slice(0, 7));
    // A person's thread after round 2: its review's commit and its own time, no round.
    rerender({
      ...c22,
      first_round: 2,
      written: { commit: "52d2da140191a9d47becf6a446310be803c007fc", at: "2026-11-12T09:00:00Z" },
    });
    expect(header()).toHaveTextContent(/written on 52d2da1, 12 Nov 2026/);
    expect(header()).not.toHaveTextContent(/round/);
    rerender({ ...c22, first_round: 2, written: { at: "2026-11-12T09:05:00Z" } });
    expect(header()).toHaveTextContent(/written 12 Nov 2026/);
    rerender({ ...c22, first_round: 2, written: undefined });
    expect(header()).toHaveTextContent(new RegExp(`first seen at ${c22.first_seen_head.slice(0, 7)}$`));
  });

  it("says validity is judged at the PR head, so a comment fixed since is not read as a mistaken reviewer", () => {
    show(byNo(14));
    fireEvent.click(screen.getByRole("button", { name: "LLM assessment" }));
    expect(screen.getByText("partly valid at the PR head")).toBeVisible();
    expect(
      screen.getByText("Judged on the code at the PR head, not on the code the comment was written on."),
    ).toBeVisible();
    expectTrustWording(document.body.textContent ?? "");
  });

  it("an assessment of an earlier head names that commit, not the PR head", () => {
    const c14 = byNo(14);
    show(c14, { prHead: "9f3c2e1d00000000000000000000000000000000" });
    fireEvent.click(screen.getByRole("button", { name: "LLM assessment" }));
    const at = c14.assessment!.head.slice(0, 7);
    expect(screen.getByText(`partly valid at ${at}, an earlier head`)).toBeVisible();
    expect(
      screen.getByText(
        `Judged on the code at ${at}, an earlier head, not on the code the comment was written on.`,
      ),
    ).toBeVisible();
    expect(screen.queryByText(/at the PR head/)).toBeNull();
  });

  it("shows the backtick spans of the LLM's texts as code, not as backticks", () => {
    const c14 = byNo(14);
    const a = c14.assessment!;
    show({
      ...c14,
      assessment: {
        ...a,
        necessity: { ...a.necessity, reason: "`need_tok` belongs to this PR." },
        recommendation: { ...a.recommendation, reason: "Cap `rec_tok` here." },
        evidence: [{ ...a.evidence[0]!, note: "`ev_tok` is never reset." }],
        elsewhere: [{ kind: "issue", reference: "#12", note: "see `else_tok`" }],
        fix_sketch: "Add `sketch_tok`.",
      },
    });
    const codes = [...document.body.querySelectorAll("code")].map((c) => c.textContent);
    const tokens = ["need_tok", "rec_tok", "ev_tok", "else_tok", "sketch_tok"];
    for (const token of tokens) expect(codes).toContain(token);
    for (const token of tokens) expect(document.body.textContent).not.toContain(`\`${token}\``);
  });

  it("renders every URL from the GitHub body as plain text (T24, G-PHISH)", () => {
    show(byNo(14));
    const quote = document.querySelector("[data-comment-text]")!;
    expect(within(quote as HTMLElement).queryByRole("link")).toBeNull();
    expect(quote.textContent).toContain("https://github.com/acme/node-client/blob/main/docs/nonce.md");
    expect(quote.textContent).toContain("https://evil.example/phish");
  });

  it("shows only the finding from a bot body with markup and an agent prompt", () => {
    show({
      ...byNo(14),
      body: "**<picture><img src=x></picture>Minor**: use `nonce`. [Docs](https://github.com/acme/repo)\n<details><summary>Prompt for AI agents</summary>Hidden instructions.</details>",
    });
    const quote = document.querySelector("[data-comment-text]")!;
    expect(quote).toHaveTextContent("Minor: use nonce. Docs (https://github.com/acme/repo)");
    expect(quote.querySelector("code")).toHaveTextContent("nonce");
    expect(quote).not.toHaveTextContent(/Prompt|Hidden|<picture>/);
    expect(quote.querySelector("a,img,details")).toBeNull();
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

describe("Decide sends only what the server planned for the current input (RC16)", () => {
  const PENDING = "Planning the effects of your current input; you can decide once they are shown.";
  it.each<[string, EffectsPreviewState, string]>([
    ["planning", { state: "loading" }, PENDING],
    ["planned for an earlier input", { state: "ready", items: [], rows: "[]", updating: false }, PENDING],
    ["re-planning", { state: "ready", items: [], rows: "[]", updating: true }, PENDING],
    [
      "refused",
      { state: "error", message: "C11: the target does not fit this decision." },
      "The server refused this input (its message is shown above); change it to decide.",
    ],
  ])("%s: neither the button nor Enter decides, and the reason is announced", async (_, preview, reason) => {
    const { onDecide } = show(byNo(11), { preview, onDraft: vi.fn() });
    const decide = screen.getByRole("button", { name: "Decide" });
    expect(decide).toHaveAttribute("aria-disabled", "true");
    expect(decide).toHaveAccessibleDescription(expect.stringContaining(reason));
    expect(screen.getByText(reason)).toHaveAttribute("role", "status");
    await userEvent.click(decide);
    fireEvent.keyDown(document.body, { key: "Enter" });
    expect(onDecide).not.toHaveBeenCalled();
  });

  it("a refusal offers no Plan again: the input has to change", () => {
    show(byNo(11), { preview: { state: "error", message: "C11: no." }, onDraft: vi.fn() });
    expect(screen.queryByRole("button", { name: "Plan again" })).toBeNull();
  });

  it("a plan that failed (no answer, 5xx) offers Plan again on the blocked note; Decide stays blocked", async () => {
    const planAgain = vi.fn();
    const { onDecide } = show(byNo(11), {
      preview: { state: "failed", message: "The service is unavailable.", planAgain },
      onDraft: vi.fn(),
    });
    const reason =
      'The effects could not be planned (The service is unavailable); diffpilot tries again shortly, or choose "Plan again".';
    const decide = screen.getByRole("button", { name: "Decide" });
    expect(decide).toHaveAttribute("aria-disabled", "true");
    expect(decide).toHaveAccessibleDescription(expect.stringContaining(reason));
    await userEvent.click(screen.getByRole("button", { name: "Plan again" }));
    expect(planAgain).toHaveBeenCalledTimes(1);
    await userEvent.click(decide);
    fireEvent.keyDown(document.body, { key: "Enter" });
    expect(onDecide).not.toHaveBeenCalled();
  });

  it("planned for exactly the current input: Decide sends that input", async () => {
    const { onDecide } = show(byNo(11));
    const decide = screen.getByRole("button", { name: "Decide" });
    expect(decide).not.toHaveAttribute("aria-disabled");
    await userEvent.click(decide);
    expect(onDecide).toHaveBeenCalledWith(expect.objectContaining({ kind: "reject" }));
  });
});

it("shows one collapsed primary proposal with its original external target", () => {
  const item = byNo(14);
  show({
    ...item,
    assessment: {
      ...item.assessment!,
      proposed_decision: {
        kind: "handled_in_pr",
        target_pr: null,
        target_url: "https://github.com/other/repo/pull/99",
        title: null,
        fixed_in: null,
      },
    },
  });
  expect(screen.getAllByRole("button", { name: "LLM assessment" })).toHaveLength(1);
  fireEvent.click(screen.getByRole("button", { name: "LLM assessment" }));
  expect(screen.getByText(/https:\/\/github.com\/other\/repo\/pull\/99/)).toBeVisible();
});

it("keeps model-read body points separate from grounded GitHub quotes and nullable evidence", () => {
  const item = byNo(14);
  show({
    ...item,
    kind: "review_body",
    point: {
      source_kind: "review_body",
      source_url: item.url ?? "https://github.com/acme/node-client/pull/482#review-1",
      quote: "Bound the retry loop to avoid repeated requests.",
    },
    duplicate_of: 12,
    assessment: {
      ...item.assessment!,
      pre_existing: null,
      evidence_text: "The retry counter has no upper bound.",
    },
  });
  expect(screen.getByText(/Point read by diffpilot's LLM/)).toBeVisible();
  expect(screen.getByText("Quote found in the review")).toBeVisible();
  expect(screen.getByText("Bound the retry loop to avoid repeated requests.")).toBeVisible();
  fireEvent.click(screen.getByRole("button", { name: "LLM assessment" }));
  expect(screen.getByText("The retry counter has no upper bound.")).toBeVisible();
  expect(screen.getByText("not supplied")).toBeVisible();
});

it("does not claim a review-quote fact for a PR-comment point", () => {
  const item = byNo(14);
  show({
    ...item,
    kind: "pr_comment",
    point: {
      source_kind: "pr_comment",
      source_url: "https://github.com/acme/node-client/pull/482#issuecomment-1",
      quote: "A separate comment quote.",
    },
  });
  expect(screen.getByText(/Point read by diffpilot's LLM/)).toBeVisible();
  expect(screen.queryByText("Quote found in the review")).toBeNull();
});
