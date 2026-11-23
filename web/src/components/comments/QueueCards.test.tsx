import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { Planned, type Plan } from "../../__tests__/planned";
import { expectTrustWording } from "../../__tests__/wording";
import type { CommentItem, DecisionBatchRow, PlannedEffect } from "../../api/generated/types";
import { decideRestList, listItems, resetReviewLoop } from "../../mocks/review-loop";
import type { EffectsPreviewState } from "./decisions";
import { CommentsTable } from "./CommentsTable";
import { DecideRestCard, EmptyQueueCard, GroupCard, PrQuestionCard } from "./QueueCards";

const byNo = (no: number) => listItems().find((i) => i.no === no)!;
const NOW = new Date("2026-11-18T09:27:00Z");
const PENDING = "Planning the effects of your current input; you can decide once they are shown.";
const REFUSED = "The server refused this input (its message is shown above); change it to decide.";
/** Plans that do not fit the card's current draft, with the reason Decide gives. */
const UNFIT: [string, EffectsPreviewState, string][] = [
  ["planning", { state: "loading" }, PENDING],
  ["planned for other rows", { state: "ready", items: [], rows: "[]", updating: false }, PENDING],
  ["refused", { state: "error", message: "C18: this decision needs a reason." }, REFUSED],
];

beforeEach(() => {
  resetReviewLoop();
  window.localStorage.clear();
});

describe("GroupCard (US5)", () => {
  const showGroup = (preview?: EffectsPreviewState) => {
    const handlers = { onDecide: vi.fn(), onDiscuss: vi.fn(), onSkip: vi.fn() };
    render(
      <Planned>
        {(plan) => (
          <GroupCard
            items={[byNo(18), byNo(19)]}
            position={{ index: 7, total: 9 }}
            onDecide={handlers.onDecide}
            onDiscussIndividually={handlers.onDiscuss}
            onSkip={handlers.onSkip}
            busy={false}
            error={null}
            storageKey="t"
            {...plan}
            preview={preview ?? plan.preview}
          />
        )}
      </Planned>,
    );
    return handlers;
  };

  it("one decision for every row, each its own record; Discuss individually per row", async () => {
    const { onDecide, onDiscuss } = showGroup();
    expect(screen.getByRole("heading", { name: /Same recommendation: Reject/ })).toHaveFocus();
    expect(screen.getByText(/Gate items are never grouped/)).toBeVisible();
    await userEvent.click(screen.getAllByRole("button", { name: "Discuss individually" })[1]!);
    expect(onDiscuss).toHaveBeenCalledWith(19);
    await userEvent.click(screen.getByRole("button", { name: "Reject all 2" }));
    const rows = onDecide.mock.calls[0]![0];
    expect(rows.map((r: { item_no: number }) => r.item_no)).toEqual([18, 19]);
    expect(rows[0]).toMatchObject({ kind: "reject", resolve_ticked: true });
  });

  it("keys: s skips and Enter decides the group, as its tooltips say (plan §3.8)", () => {
    const { onDecide, onSkip } = showGroup();
    fireEvent.keyDown(document.body, { key: "s" });
    expect(onSkip).toHaveBeenCalledTimes(1);
    fireEvent.keyDown(document.body, { key: "Enter" });
    expect(onDecide).toHaveBeenCalledTimes(1);
  });

  it.each(UNFIT)(
    "%s: neither the button nor Enter decides the group; the reason is announced",
    async (_, preview, reason) => {
      const { onDecide } = showGroup(preview);
      const decide = screen.getByRole("button", { name: "Reject all 2" });
      expect(decide).toHaveAttribute("aria-disabled", "true");
      expect(decide).toHaveAccessibleDescription(expect.stringContaining(reason));
      await userEvent.click(decide);
      fireEvent.keyDown(document.body, { key: "Enter" });
      expect(onDecide).not.toHaveBeenCalled();
    },
  );

  it("each row opens its comment on GitHub in a new tab (plan §3.9)", () => {
    showGroup();
    expect(screen.getByRole("link", { name: "Open C18 on GitHub (new tab)" })).toHaveAttribute(
      "href",
      byNo(18).url,
    );
  });
});

describe("DecideRestCard (US7)", () => {
  const showRest = (preview?: EffectsPreviewState) => {
    const onConfirm = vi.fn();
    render(
      <Planned>
        {(plan) => (
          <DecideRestCard
            rest={decideRestList()}
            items={new Map(listItems().map((i) => [i.no, i]))}
            onConfirm={onConfirm}
            onCancel={vi.fn()}
            busy={false}
            error={null}
            {...plan}
            preview={preview ?? plan.preview}
          />
        )}
      </Planned>,
    );
    return onConfirm;
  };

  it.each(UNFIT)("%s: Apply stores nothing; the reason is announced", async (_, preview, reason) => {
    const onConfirm = showRest(preview);
    const apply = screen.getByRole("button", { name: /^Apply \d+ decisions$/ });
    expect(apply).toHaveAttribute("aria-disabled", "true");
    expect(apply).toHaveAccessibleDescription(expect.stringContaining(reason));
    await userEvent.click(apply);
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it("lists the server's rows, names every item left out with its reason, stores nothing before Apply", async () => {
    const onConfirm = showRest();
    const left = screen.getByRole("alert");
    expect(left).toHaveTextContent(/^Left out: 3 items\./);
    expect(left).toHaveTextContent(
      "C14 (gate item G-FUNDS), C15 (gate item G-SECRET), C21 (asked again: its reason needs a look) stay single questions; gate items are never decided in bulk.",
    );
    expect(screen.queryByRole("checkbox", { name: "Include C21" })).toBeNull();
    expect(screen.getByRole("link", { name: "Open C22 on GitHub (new tab)" })).toBeVisible();
    await userEvent.click(screen.getByRole("checkbox", { name: "Include C22" }));
    expect(onConfirm).not.toHaveBeenCalled();
    const apply = screen.getByRole("button", { name: /^Apply \d+ decisions$/ });
    await userEvent.click(apply);
    const nos = onConfirm.mock.calls[0]![0].map((r: { item_no: number }) => r.item_no);
    expect(nos).not.toContain(22);
    expect(nos).toEqual(
      decideRestList()
        .rows.map((r) => r.row.item_no)
        .filter((n) => n !== 22),
    );
    expectTrustWording(document.body.textContent ?? "");
  });
});

describe("PrQuestionCard (US11, US12)", () => {
  const card = (item: CommentItem, handlers: { onDecide: () => void; onSkip: () => void }, plan: Plan) => (
    <PrQuestionCard
      item={item}
      prUrl="https://github.com/acme/node-client/pull/482"
      busy={false}
      error={null}
      {...handlers}
      {...plan}
    />
  );
  /** The card planned by the mocks' server model, or with `preview` for whatever it drafts. */
  const showQuestion = (item: CommentItem, preview?: EffectsPreviewState) => {
    const handlers = { onDecide: vi.fn(), onSkip: vi.fn() };
    render(
      preview ? (
        card(item, handlers, { preview, onDraft: vi.fn() })
      ) : (
        <Planned>{(plan) => card(item, handlers, plan)}</Planned>
      ),
    );
    return handlers;
  };
  /** The card with a plan given per draft: `planFor` gets the rows the card drafted. */
  const showDrafted = (item: CommentItem) => {
    const handlers = { onDecide: vi.fn(), onSkip: vi.fn() };
    const onDraft = vi.fn<(rows: DecisionBatchRow[] | null) => void>();
    const view = render(card(item, handlers, { preview: { state: "loading" }, onDraft }));
    const drafted = JSON.stringify(onDraft.mock.calls.at(-1)![0]);
    const plan = (effects: PlannedEffect[]) =>
      view.rerender(
        card(item, handlers, {
          preview: { state: "ready", items: [{ item_no: item.no, effects }], rows: drafted, updating: false },
          onDraft,
        }),
      );
    return { ...handlers, plan };
  };
  const effect = (kind: PlannedEffect["kind"], params: Partial<PlannedEffect["params"]>): PlannedEffect => ({
    seq: 1,
    after: null,
    kind,
    executor: "agent",
    params: { text: null, thread_id: null, review_id: null, title: null, url: null, item_nos: [], ...params },
  });

  it("round-3 merge question: two options, no recommendation, merge stays on GitHub", async () => {
    const { onDecide } = showQuestion(byNo(24));
    const options = within(screen.getByRole("radiogroup")).getAllByRole("radio");
    expect(options).toHaveLength(2);
    expect(options[1]).toHaveAccessibleName(/Another round/);
    expect(screen.getByText(/No recommendation: this is a PR-level choice/)).toBeVisible();
    expect(screen.getByRole("link", { name: /Merge on GitHub/ })).toHaveAttribute("target", "_blank");
    await userEvent.click(screen.getByRole("button", { name: "Decide" }));
    expect(onDecide).toHaveBeenCalledWith(expect.objectContaining({ kind: "merge_with_follow_ups" }));
  });

  it("keys: 2 chooses Another round, Enter decides, s skips", () => {
    const { onDecide, onSkip } = showQuestion(byNo(24));
    fireEvent.keyDown(document.body, { key: "2" });
    expect(screen.getByRole("radio", { name: /Another round/ })).toBeChecked();
    fireEvent.keyDown(document.body, { key: "Enter" });
    expect(onDecide).toHaveBeenCalledWith(expect.objectContaining({ kind: "another_round" }));
    fireEvent.keyDown(document.body, { key: "s" });
    expect(onSkip).toHaveBeenCalledTimes(1);
  });

  it.each(UNFIT)(
    "%s: neither the button nor Enter decides; the reason is announced",
    async (_, preview, reason) => {
      const { onDecide } = showQuestion(byNo(24), preview);
      const decide = screen.getByRole("button", { name: "Decide" });
      expect(decide).toHaveAttribute("aria-disabled", "true");
      expect(decide).toHaveAccessibleDescription(expect.stringContaining(reason));
      await userEvent.click(decide);
      fireEvent.keyDown(document.body, { key: "Enter" });
      expect(onDecide).not.toHaveBeenCalled();
    },
  );

  it("a plan whose scope changed while it was open is decided only after the owner confirms it", async () => {
    const { onDecide, plan } = showDrafted(byNo(24));
    plan([effect("follow_up_pr", { item_nos: [11, 22] })]);
    expect(screen.getByText(/listing C11, C22 /)).toBeVisible();
    plan([effect("follow_up_pr", { item_nos: [11, 22, 30] })]);
    const notice = screen.getByText(/What this decision acts on changed since you looked/);
    expect(notice).toHaveAttribute("role", "status");
    const decide = screen.getByRole("button", { name: "Decide" });
    expect(decide).toHaveAttribute("aria-disabled", "true");
    await userEvent.click(decide);
    fireEvent.keyDown(document.body, { key: "Enter" });
    expect(onDecide).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "Use the new plan" }));
    expect(screen.queryByText(/What this decision acts on changed since you looked/)).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Decide" }));
    expect(onDecide).toHaveBeenCalledWith(expect.objectContaining({ kind: "merge_with_follow_ups" }));
  });

  it("dismissal: the field shows the server's default message, which is posted when left as is", async () => {
    const item = { ...byNo(24), stable_key: "pr:3:dismiss", title: "Dismiss stale bot reviews?" };
    const { onDecide, plan } = showDrafted(item);
    plan([effect("dismiss_review", { text: "Server default.", review_id: "PRR_1" })]);
    expect(screen.getByLabelText("Dismissal message (posted)")).toHaveValue("Server default.");
    expect(screen.getByText("dismisses the stale review with the message above.")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Decide" }));
    expect(onDecide).toHaveBeenCalledWith(
      expect.objectContaining({ kind: "dismiss_stale_reviews", reason: null, reply_text: null }),
    );
  });
});

describe("EmptyQueueCard", () => {
  it("says what was read and that an empty queue proves nothing", () => {
    render(
      <EmptyQueueCard
        read={{
          read_at: "2026-11-18T09:20:00Z",
          threads: 14,
          complete: true,
          reviews: 4,
          comments: 3,
          failure: null,
          next_read_at: null,
        }}
        assessed={14}
        head="52d2da1"
        now={NOW}
      />,
    );
    expect(screen.getByText(/14 threads \(all pages read\), 4 reviews and 3 PR comments/)).toBeVisible();
    expectTrustWording(document.body.textContent ?? "");
  });
});

describe("CommentsTable (US8, US27)", () => {
  const table = (items: CommentItem[], onSubmit: () => void) => (
    <CommentsTable
      prNumber={482}
      purpose="Retries."
      items={items}
      askAnswers={(i) => i.ask_answers}
      itemHref={(i) => `/pr/acme/node-client/482/comments?item=C${i.no}`}
      onNavigate={vi.fn()}
      onOpenAsk={vi.fn()}
      onSubmit={onSubmit}
      busy={false}
      error={null}
      now={NOW}
    />
  );
  const show = (onSubmit = vi.fn()) => {
    render(table(listItems(), onSubmit));
    return onSubmit;
  };

  it("a read that changes an item keeps typed rows; the changed row needs a look", async () => {
    const onSubmit = vi.fn();
    const { rerender } = render(table(listItems(), onSubmit));
    await userEvent.type(screen.getByRole("textbox", { name: "Your comment on C22" }), "small please");
    rerender(
      table(
        listItems().map((i) => (i.no === 22 ? { ...i, revision: i.revision + 1 } : i)),
        onSubmit,
      ),
    );
    expect(screen.getByRole("textbox", { name: "Your comment on C22" })).toHaveValue("small please");
    expect(
      screen.getByText(/Still needed: .*C22 \(changed since you started: check the row\)/),
    ).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Keep my input for C22" }));
    expect(screen.queryByText(/C22 \(changed since you started/)).toBeNull();
  });

  it("each row opens its comment on GitHub in a new tab (plan §3.9)", () => {
    show();
    expect(screen.getByRole("link", { name: "Open C22 on GitHub (new tab)" })).toHaveAttribute(
      "href",
      listItems().find((i) => i.no === 22)!.url,
    );
  });

  it("gate rows start empty and block submit with the reason; choosing makes it available", async () => {
    const onSubmit = show();
    const select = screen.getByRole("combobox", { name: /Decision for C14 .*gate item: choose explicitly/ });
    expect(select).toHaveValue("");
    const submit = screen.getByRole("button", { name: /Record \d+ decisions/ });
    expect(submit).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByText(/Still needed: C14 \(gate item: choose explicitly\), C15/)).toBeVisible();
    await userEvent.click(submit);
    expect(onSubmit).not.toHaveBeenCalled();
    await userEvent.selectOptions(select, "fix_here");
    await userEvent.selectOptions(screen.getByRole("combobox", { name: /Decision for C15/ }), "accept");
    expect(screen.getByText(/C15 \(reason or target\)/)).toBeVisible();
    await userEvent.type(screen.getByRole("textbox", { name: /Reason or target for C15/ }), "Test key only.");
    expect(screen.getByRole("checkbox", { name: "Resolve C15" })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: /Record \d+ decisions/ }));
    const rows = onSubmit.mock.calls[0]![0];
    expect(rows.find((r: { item_no: number }) => r.item_no === 15)).toMatchObject({
      kind: "reject",
      override: { type: "blocker_gate", gate: "G-SECRET" },
      resolve_ticked: true,
    });
  });

  it("an item the latest read no longer shows is listed with that fact and needs no decision (plan §2.1)", async () => {
    render(
      table(
        listItems().map((i) => (i.no === 22 ? { ...i, gone_at: "2026-11-18T09:00:00Z" } : i)),
        vi.fn(),
      ),
    );
    expect(screen.queryByRole("combobox", { name: /Decision for C22/ })).toBeNull();
    expect(screen.queryByText(/C22 \(/)).toBeNull();
    const more = screen.queryByRole("button", { name: "Show all" });
    if (more) await userEvent.click(more);
    const row = document.querySelector('[data-row="C22"]') as HTMLElement;
    expect(within(row).getByText(/not in the latest read from GitHub/)).toBeVisible();
    expect(row.textContent).not.toMatch(/Resolved on GitHub/);
  });

  it("the problem links to Questions with that item; decided rows show where they were decided", () => {
    show();
    expect(
      screen.getByRole("link", { name: "Retry may resend a signed request after a late reply" }),
    ).toHaveAttribute("href", "/pr/acme/node-client/482/comments?item=C14");
    expect(screen.getAllByText(/carried/).length).toBeGreaterThan(0);
    expect(screen.getByRole("table", { name: "Decision table of #482" })).toBeVisible();
  });
});
