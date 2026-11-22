import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { expectTrustWording } from "../../__tests__/wording";
import { listItems, resetReviewLoop } from "../../mocks/review-loop";
import { CommentsTable } from "./CommentsTable";
import { DecideRestCard, EmptyQueueCard, GroupCard, PrQuestionCard } from "./QueueCards";

const byNo = (no: number) => listItems().find((i) => i.no === no)!;
const NOW = new Date("2026-11-18T09:27:00Z");

beforeEach(() => {
  resetReviewLoop();
  window.localStorage.clear();
});

describe("GroupCard (US5)", () => {
  it("one decision for every row, each its own record; Discuss individually per row", async () => {
    const onDecide = vi.fn();
    const onDiscuss = vi.fn();
    render(
      <GroupCard
        items={[byNo(18), byNo(19)]}
        position={{ index: 7, total: 9 }}
        onDecide={onDecide}
        onDiscussIndividually={onDiscuss}
        onSkip={vi.fn()}
        busy={false}
        error={null}
        storageKey="t"
      />,
    );
    expect(screen.getByRole("heading", { name: /Same recommendation: Reject/ })).toHaveFocus();
    expect(screen.getByText(/Gate items are never grouped/)).toBeVisible();
    await userEvent.click(screen.getAllByRole("button", { name: "Discuss individually" })[1]!);
    expect(onDiscuss).toHaveBeenCalledWith(19);
    await userEvent.click(screen.getByRole("button", { name: "Reject all 2" }));
    const rows = onDecide.mock.calls[0]![0];
    expect(rows.map((r: { item_no: number }) => r.item_no)).toEqual([18, 19]);
    expect(rows[0]).toMatchObject({ kind: "reject", resolve_ticked: true });
  });
});

describe("DecideRestCard (US7)", () => {
  it("lists the rows with recommendations, names the gate items left out, stores nothing before Apply", async () => {
    const onConfirm = vi.fn();
    render(
      <DecideRestCard
        items={[byNo(11), byNo(22), byNo(20)]}
        leftOut={[byNo(14), byNo(15)]}
        onConfirm={onConfirm}
        onCancel={vi.fn()}
        busy={false}
        error={null}
      />,
    );
    expect(screen.getByRole("alert")).toHaveTextContent("Left out: 2 gate items.");
    expect(screen.getByRole("alert")).toHaveTextContent("C14 (G-FUNDS) and C15 (G-SECRET)");
    await userEvent.click(screen.getByRole("checkbox", { name: "Include C22" }));
    expect(onConfirm).not.toHaveBeenCalled();
    expect(screen.getByText(/2 decisions: Reject 1 · Create follow-up PR 1/)).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Apply 2 decisions" }));
    expect(onConfirm.mock.calls[0]![0].map((r: { item_no: number }) => r.item_no)).toEqual([11, 20]);
    expectTrustWording(document.body.textContent ?? "");
  });
});

describe("PrQuestionCard (US11, US12)", () => {
  it("round-3 merge question: two options, no recommendation, merge stays on GitHub", async () => {
    const onDecide = vi.fn();
    render(
      <PrQuestionCard
        item={byNo(24)}
        prUrl="https://github.com/acme/node-client/pull/482"
        onDecide={onDecide}
        onSkip={vi.fn()}
        busy={false}
        error={null}
      />,
    );
    const options = within(screen.getByRole("radiogroup")).getAllByRole("radio");
    expect(options).toHaveLength(2);
    expect(options[1]).toHaveAccessibleName(/Another round/);
    expect(screen.getByText(/No recommendation: this is a PR-level choice/)).toBeVisible();
    expect(screen.getByRole("link", { name: /Merge on GitHub/ })).toHaveAttribute("target", "_blank");
    await userEvent.click(screen.getByRole("button", { name: "Decide" }));
    expect(onDecide).toHaveBeenCalledWith(expect.objectContaining({ kind: "merge_with_follow_ups" }));
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
  const show = (onSubmit = vi.fn()) => {
    render(
      <CommentsTable
        prNumber={482}
        purpose="Retries."
        items={listItems()}
        askAnswers={(i) => i.ask_answers}
        itemHref={(i) => `/pr/acme/node-client/482/comments?item=C${i.no}`}
        onNavigate={vi.fn()}
        onOpenAsk={vi.fn()}
        onSubmit={onSubmit}
        busy={false}
        error={null}
        now={NOW}
      />,
    );
    return onSubmit;
  };

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

  it("the problem links to Questions with that item; decided rows show where they were decided", () => {
    show();
    expect(
      screen.getByRole("link", { name: "Retry may resend a signed request after a late reply" }),
    ).toHaveAttribute("href", "/pr/acme/node-client/482/comments?item=C14");
    expect(screen.getAllByText(/carried/).length).toBeGreaterThan(0);
    expect(screen.getByRole("table", { name: "Decision table of #482" })).toBeVisible();
  });
});
