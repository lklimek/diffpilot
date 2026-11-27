import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { expectTagsWithTips } from "../../__tests__/provenance";
import { expectTrustWording } from "../../__tests__/wording";
import type { CommentLog, Decision, MarkLogEntry } from "../../api/generated/types";
import {
  carryOvers,
  listItems,
  log,
  notificationSettings,
  overview,
  prPage,
  resetChannels,
  resetReviewLoop,
} from "../../mocks/review-loop";
import { CollapsibleSection } from "../collapsible/CollapsibleSection";
import { NotificationChannelCard } from "../settings/NotificationChannelCard";
import { CarryOvers } from "./CarryOvers";
import { DecisionLog } from "./DecisionLog";
import { ProcessCommentsCard } from "./ProcessCommentsCard";
import { RoundsTable } from "./RoundsTable";
import { StateBadge } from "./StateBadge";

const NOW = new Date("2026-11-18T09:27:00Z");
const PR = { repo: { owner: "acme", name: "node-client" }, number: 482 };
const text = () => document.body.textContent ?? "";

beforeEach(() => {
  resetReviewLoop();
  resetChannels();
  window.localStorage.clear();
});

describe("CollapsibleSection (plan §3.9)", () => {
  it("is a disclosure button: open by default, Enter and Space toggle, the state is remembered", async () => {
    const { unmount } = render(
      <CollapsibleSection title="Facts" storageKey="alice:view:facts">
        <p>inside</p>
      </CollapsibleSection>,
    );
    const button = screen.getByRole("button", { name: "Facts" });
    expect(button).toHaveAttribute("aria-expanded", "true");
    expect(document.getElementById(button.getAttribute("aria-controls")!)).toBeVisible();
    button.focus();
    await userEvent.keyboard("{Enter}");
    expect(button).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByText("inside")).not.toBeVisible();
    unmount();
    render(
      <CollapsibleSection title="Facts" storageKey="alice:view:facts">
        <p>inside</p>
      </CollapsibleSection>,
    );
    const again = screen.getByRole("button", { name: "Facts" });
    expect(again).toHaveAttribute("aria-expanded", "false");
    again.focus();
    await userEvent.keyboard(" ");
    expect(again).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("region", { name: "Facts" })).toBeVisible();
  });
});

describe("StateBadge (B1–B5)", () => {
  it.each([
    ["blocked_failing", "✕", "Blocked / failing"],
    ["questions_waiting", "?", "Questions waiting"],
    ["executing", "▶", "Executing"],
    ["waiting_for_bot_round", "⌛", "Waiting for bot round"],
    ["ready_to_merge", "✓", "Ready to merge"],
  ] as const)("%s: icon and text, facts in the tip", (badge, icon, label) => {
    render(<StateBadge view={{ badge, facts: ["9 undecided questions"], missing: [] }} />);
    const el = screen.getByText(label);
    expect(el.textContent).toBe(`${icon}${label}`);
    expect(el).toHaveAccessibleDescription("9 undecided questions");
  });

  it("unknown inputs: no badge, the tip names what is missing", () => {
    render(<StateBadge view={{ badge: null, facts: [], missing: ["CI of the head not read"] }} />);
    expect(screen.getByText("No state")).toHaveAccessibleDescription("Not known: CI of the head not read.");
  });
});

describe("ProcessCommentsCard (V0)", () => {
  it("names a GitHub read that never succeeded in the card only", () => {
    const page = prPage();
    render(
      <ProcessCommentsCard
        summary={{
          ...page.comments,
          read: { ...page.comments.read, read_at: null, failure: "unavailable", next_read_at: null },
        }}
        now={NOW}
        questionsHref="/q"
        tableHref="/t"
        onNavigate={vi.fn()}
        storageKey="alice:pr-page:comments"
      />,
    );
    expect(screen.getByRole("status")).toHaveTextContent(
      "GitHub could not be read (GitHub did not answer as expected). Nothing was read from GitHub yet.",
    );
  });
});

describe("RoundsTable (V4)", () => {
  it("one row per round with counts and a GitHub link-out", () => {
    render(<RoundsTable rounds={overview().rounds} prNumber={482} />);
    const table = screen.getByRole("table", { name: "Bot rounds of #482" });
    expect(within(table).getAllByRole("row")).toHaveLength(5);
    expect(screen.getByRole("link", { name: "Open round 4 on GitHub (new tab)" })).toHaveAttribute(
      "target",
      "_blank",
    );
  });
});

describe("RoundsTable column headers", () => {
  it("the GitHub tag of Bot verdict takes keyboard focus and shows its tip (brief §4 rule 3)", () => {
    render(<RoundsTable rounds={overview().rounds} prNumber={482} />);
    expectTagsWithTips(screen.getByRole("columnheader", { name: /^Bot verdict/ }), 1);
  });
});

describe("CarryOvers (V5)", () => {
  it("this PR: tiles with counts and the dated list with why", () => {
    render(
      <CarryOvers
        page={carryOvers("acme/node-client/482")}
        scope="pr"
        prHref="/a"
        allHref="/b"
        onNavigate={vi.fn()}
        now={NOW}
      />,
    );
    expect(
      within(screen.getByRole("list", { name: "Carry-over counts" })).getAllByRole("listitem"),
    ).toHaveLength(5);
    expect(screen.getByRole("link", { name: "This pull request" })).toHaveAttribute("aria-current", "page");
    expect(screen.getByText(/the bot edited its comment \(text changed\)/)).toBeVisible();
    expectTrustWording(text());
  });

  it("all PRs: one row per PR", () => {
    render(
      <CarryOvers
        page={carryOvers(null)}
        scope="all"
        prHref={null}
        allHref="/b"
        onNavigate={vi.fn()}
        now={NOW}
      />,
    );
    expect(
      within(screen.getByRole("table", { name: "Carry-overs per pull request" })).getAllByRole("row"),
    ).toHaveLength(5);
    expect(screen.getByRole("link", { name: "Open acme/wire-codec #77 on GitHub (new tab)" })).toBeVisible();
  });

  it("a page with older events says how many are shown and loads older ones (RC11 next_cursor)", async () => {
    const onShowOlder = vi.fn();
    render(
      <CarryOvers
        page={{ ...carryOvers("acme/node-client/482"), next_cursor: "2026-11-16T15:30:00Z~c0" }}
        scope="pr"
        prHref="/a"
        allHref="/b"
        onNavigate={vi.fn()}
        onShowOlder={onShowOlder}
        now={NOW}
      />,
    );
    expect(screen.getByText("The 6 newest events are shown.")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Show older events" }));
    expect(onShowOlder).toHaveBeenCalledTimes(1);
  });
});

describe("DecisionLog (V3)", () => {
  /** The log with the newest decision changed by `fields`. */
  const withDecision = (fields: Partial<Decision>): CommentLog => {
    const l = log();
    const last = l.decisions.at(-1)!;
    return { ...l, decisions: [...l.decisions.slice(0, -1), { ...last, ...fields }] };
  };
  const reported = (reference: string): CommentLog => {
    const l = log();
    const last = l.decisions.at(-1)!;
    const effect = {
      ...last.effects[0]!,
      reports: [
        {
          outcome: "done" as const,
          reference,
          message: null,
          token_name: "agent-7",
          at: "2026-11-18T09:00:00Z",
        },
      ],
    };
    return { ...l, decisions: [{ ...last, effects: [effect] }] };
  };

  it("agents with push targets, decisions with via, comment and agent reports", () => {
    render(<DecisionLog log={log()} pr={PR} now={NOW} storageKey="t" origin="http://agentic:3000" />);
    expect(screen.getByText("build-host-2")).toBeVisible();
    expect(screen.getByText(/failing since 09:20: HTTP 502/)).toBeVisible();
    expect(screen.getByText(/Accept despite the gate \(G-FUNDS\)/)).toBeVisible();
    expect(screen.getByText(/Your comment \(sent to the agent, not posted\)/)).toBeVisible();
    expect(screen.getAllByText(/Agent laptop-cli reports: done/).length).toBeGreaterThan(0);
    expect(screen.getAllByText(/Found on GitHub/).length).toBeGreaterThan(0);
    expectTrustWording(text());
  });

  it("hides GitHub decisions by default and filters the log by source", async () => {
    const stored = log();
    const imported: Decision = {
      ...stored.decisions[0]!,
      id: "imported",
      via: "imported_github",
      decided_by: "external-maintainer",
      kind: "resolve",
      override: null,
    };
    const withImport = { ...stored, decisions: [...stored.decisions, imported] };
    render(<DecisionLog log={withImport} pr={PR} now={NOW} storageKey="t" origin="http://agentic:3000" />);
    expect(screen.queryByText(/by external-maintainer/)).toBeNull();
    const source = screen.getByRole("combobox", { name: "Decision source" });
    expect(source).toHaveDisplayValue("DiffPilot");
    expect(screen.getByRole("option", { name: "GitHub (1)" })).toBeInTheDocument();
    await userEvent.selectOptions(source, "github");
    expect(screen.getByText(/by external-maintainer/)).toBeVisible();
    expect(screen.queryByText(/Accept despite the gate/)).toBeNull();
    await userEvent.selectOptions(source, "all");
    expect(screen.getByText(/by external-maintainer/)).toBeVisible();
    expect(screen.getByText(/Accept despite the gate/)).toBeVisible();
  });

  it("lists a standing GitHub import once when it occurs in both responses", async () => {
    const stored = log();
    const item = listItems()[0]!;
    const imported: Decision = {
      ...stored.decisions[0]!,
      id: "imported",
      item_no: item.no,
      via: "imported_github",
      decided_by: "external-maintainer",
      kind: "resolve",
      override: null,
    };
    render(
      <DecisionLog
        log={{ ...stored, decisions: [...stored.decisions, imported] }}
        githubDecisions={[
          {
            ...item,
            imported_decision: {
              kind: "resolve",
              by: imported.decided_by,
              at: imported.decided_at,
              evidence_url: "https://github.com/acme/node-client/pull/482#issuecomment-17",
            },
          },
        ]}
        pr={PR}
        now={NOW}
        storageKey="t"
        origin="http://agentic:3000"
      />,
    );
    expect(screen.getByRole("option", { name: "GitHub (1)" })).toBeInTheDocument();
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "Decision source" }), "github");
    expect(screen.getByText(/by external-maintainer/)).toBeVisible();
    expect(screen.queryByText(/Resolved on GitHub by/)).toBeNull();
  });

  it("labels and links the effect of creating a follow-up issue", () => {
    const stored = reported("https://github.com/acme/node-client/issues/42");
    const decision = stored.decisions[0]!;
    const effect = decision.effects[0]!;
    render(
      <DecisionLog
        log={{
          ...stored,
          decisions: [
            {
              ...decision,
              kind: "follow_up_issue",
              effects: [
                { ...effect, kind: "upstream_issue", params: { ...effect.params, title: "Follow up" } },
              ],
            },
          ],
        }}
        pr={PR}
        now={NOW}
        storageKey="t"
        origin="http://agentic:3000"
      />,
    );
    expect(screen.getByText(/follow-up issue: /)).toBeVisible();
    expect(screen.getByRole("link", { name: /Open the reported follow-up issue/ })).toHaveAttribute(
      "href",
      "https://github.com/acme/node-client/issues/42",
    );
  });

  it("shows mark history and offers Undo only for the active entry", async () => {
    const active: MarkLogEntry = {
      id: "mark-active",
      item_no: 30,
      by: "alice",
      at: "2026-11-18T09:12:00Z",
      follows: 14,
      reason: null,
      undone_at: null,
      undone_by: null,
    };
    const ended: MarkLogEntry = {
      ...active,
      id: "mark-ended",
      item_no: 31,
      reason: "Fixture only.",
      undone_at: "2026-11-18T09:15:00Z",
      undone_by: "bob",
    };
    const props = {
      log: { ...log(), marks: [ended, active] },
      pr: PR,
      now: NOW,
      storageKey: "t",
      origin: "http://agentic:3000",
      onUndoMark: vi.fn(),
    };
    const { rerender } = render(<DecisionLog {...props} undoBusy />);
    const activeEntry = screen.getByText(/C30 · Not a review point/).closest("li")!;
    expect(activeEntry).toHaveTextContent(/18 Nov 2026, .* by alice · follows C14/);
    const undo = within(activeEntry).getByRole("button", { name: "Undo mark on C30" });
    expect(undo).toHaveAttribute("aria-disabled", "true");
    await userEvent.click(undo);
    expect(props.onUndoMark).not.toHaveBeenCalled();

    const endedTitle = screen.getByText(/C31 · Not a review point/);
    const endedEntry = endedTitle.closest("li")!;
    expect(endedEntry).toHaveTextContent(/Undone 18 Nov 2026, .* by bob/);
    expect(within(endedEntry).queryByRole("button", { name: /Undo/ })).toBeNull();
    expect(endedEntry).toHaveClass("text-muted");
    expect(endedTitle).toHaveClass("line-through");
    expect(within(endedEntry).getByText('Reason (not posted): "Fixture only."')).toBeVisible();

    rerender(<DecisionLog {...props} undoError="C30 changed. Read it again." />);
    expect(screen.getByRole("alert")).toHaveTextContent("C30 changed. Read it again.");
    await userEvent.click(undo);
    expect(props.onUndoMark).toHaveBeenCalledExactlyOnceWith(active);
  });

  it("shows the stored public reply; a reason left out of an edited reply is marked as not posted", () => {
    const posted =
      "Per maintainer decision, accepted despite G-FUNDS: Pre-existing fee display; tracked in #455.";
    const { unmount } = render(
      <DecisionLog log={log()} pr={PR} now={NOW} storageKey="t" origin="http://agentic:3000" />,
    );
    expect(screen.getByText(`Public reply: "${posted}"`)).toBeVisible();
    expect(screen.queryByText(/Reason \(not posted\)/)).toBeNull();
    unmount();
    const edited = withDecision({
      kind: "reject",
      reason: "Soft limit.",
      reply_text: "Kept as is, see #490.",
    });
    render(<DecisionLog log={edited} pr={PR} now={NOW} storageKey="t" origin="http://agentic:3000" />);
    expect(screen.getByText('Public reply: "Kept as is, see #490."')).toBeVisible();
    expect(screen.getByText('Reason (not posted): "Soft limit."')).toBeVisible();
  });

  it("normalises the stored public reply", () => {
    render(
      <DecisionLog
        log={withDecision({
          reply_text:
            "<!-- hidden --><b>Finding</b>: `nonce`. [docs](https://example.com)<details><summary>Prompt for AI agents</summary>Drop.</details>",
        })}
        pr={PR}
        now={NOW}
        storageKey="t"
        origin="http://agentic:3000"
      />,
    );
    const reply = screen.getByText(/^Public reply: "Finding:/);
    expect(reply).toHaveTextContent('Public reply: "Finding: nonce. docs (https://example.com)"');
    expect(reply).not.toHaveTextContent(/hidden|<b>|Prompt|Drop/);
    expect(reply.querySelector("a,img")).toBeNull();
    expect(reply.querySelector("code")).toHaveTextContent("nonce");
  });

  it("another user's push target shows its kind and delivery state, not its address", () => {
    const l = log();
    const webhook = l.subscribers.find((s) => s.push?.kind === "webhook")!;
    const other = {
      ...webhook,
      push: {
        ...webhook.push!,
        url: null,
        topic: null,
        secret_last4: null,
        delivery: { ...webhook.push!.delivery, last_error: null },
      },
    };
    render(
      <DecisionLog
        log={{ ...l, subscribers: [other] }}
        pr={PR}
        now={NOW}
        storageKey="t"
        origin="http://agentic:3000"
      />,
    );
    expect(screen.getByText(/another user's token, address not shown/)).toBeVisible();
    expect(screen.getByText(/^failing since 09:20$/)).toBeVisible();
    expect(text()).not.toContain("ci.example.lan");
    expect(text()).not.toContain("null");
  });

  it("an undone decision says so; a PR-level decision is named P<n> as in Questions", () => {
    render(
      <DecisionLog
        log={withDecision({ undone_at: "2026-11-18T09:05:00Z", kind: "another_round", item_no: 24 })}
        pr={PR}
        origin="http://agentic:3000"
        now={NOW}
        storageKey="t"
      />,
    );
    expect(screen.getByText(/· P24 · Another round/)).toBeVisible();
    expect(screen.getByText(/^Undone /)).toBeVisible();
  });

  it("report_reference_outside_github_is_text: only https://github.com/ links open (threat model T24)", () => {
    render(
      <DecisionLog
        log={reported("https://github-login.example/acme")}
        pr={PR}
        now={NOW}
        storageKey="t"
        origin="http://agentic:3000"
      />,
    );
    expect(screen.queryByRole("link", { name: /Open the reported/ })).toBeNull();
    expect(screen.getByText("https://github-login.example/acme")).toBeVisible();
  });

  it("a commit reference opens that commit of the PR's repository on GitHub", () => {
    render(
      <DecisionLog
        log={reported("4c1d9e0a7b")}
        pr={PR}
        now={NOW}
        storageKey="t"
        origin="http://agentic:3000"
      />,
    );
    expect(screen.getByRole("link", { name: "Open commit 4c1d9e0 on GitHub (new tab)" })).toHaveAttribute(
      "href",
      "https://github.com/acme/node-client/commit/4c1d9e0a7b",
    );
  });
});

describe("NotificationChannelCard (V7)", () => {
  const [ntfy, webhook] = notificationSettings().channels;

  it("shows the token masked, the last delivery and Send test", async () => {
    const onTest = vi.fn();
    render(
      <NotificationChannelCard
        kind="ntfy"
        channel={ntfy!}
        onSave={vi.fn()}
        onRemove={vi.fn()}
        onTest={onTest}
        testResult={null}
        busy={false}
        error={null}
        now={NOW}
      />,
    );
    expect(screen.getByLabelText("Access token (optional)")).toHaveValue("•••• 9f2a");
    expect(screen.getByText(/Last delivery: questions ready, delivered 09:12/)).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Send test" }));
    expect(onTest).toHaveBeenCalled();
  });

  it("a failing webhook says since when and why; saving keeps an untouched secret", async () => {
    const onSave = vi.fn();
    render(
      <NotificationChannelCard
        kind="webhook"
        channel={webhook!}
        onSave={onSave}
        onRemove={vi.fn()}
        onTest={vi.fn()}
        testResult={{ outcome: "failed", http_status: null, error: "connection refused" }}
        busy={false}
        error={null}
        now={NOW}
      />,
    );
    expect(screen.getByText("Failing since 09:22")).toBeVisible();
    expect(screen.getByText(/Test failed: connection refused; one attempt, no retry/)).toBeVisible();
    fireEvent.click(screen.getByRole("checkbox", { name: "All decisions of a round executed" }));
    await userEvent.click(screen.getByRole("button", { name: "Save channel" }));
    expect(onSave).toHaveBeenCalledWith(
      expect.objectContaining({
        secret: { action: "keep" },
        events: ["questions_ready", "round_executed"],
        enabled: true,
      }),
    );
  });

  it("replaces or removes the stored secret only by an explicit choice", async () => {
    const onSave = vi.fn();
    render(
      <NotificationChannelCard
        kind="ntfy"
        channel={ntfy!}
        onSave={onSave}
        onRemove={vi.fn()}
        onTest={vi.fn()}
        testResult={null}
        busy={false}
        error={null}
        now={NOW}
      />,
    );
    await userEvent.click(screen.getByRole("button", { name: "Remove secret" }));
    expect(screen.getByLabelText("Access token (optional)")).toHaveValue("removed when you save");
    await userEvent.click(screen.getByRole("button", { name: "Save channel" }));
    expect(onSave).toHaveBeenLastCalledWith(expect.objectContaining({ secret: { action: "remove" } }));
    await userEvent.click(screen.getByRole("button", { name: "Replace" }));
    await userEvent.type(screen.getByLabelText("Access token (optional)"), "tk_new_token_1234");
    await userEvent.click(screen.getByRole("button", { name: "Save channel" }));
    expect(onSave).toHaveBeenLastCalledWith(
      expect.objectContaining({ secret: { action: "replace", value: "tk_new_token_1234" } }),
    );
    await userEvent.click(screen.getByRole("button", { name: "Replace" }));
    await userEvent.click(screen.getByRole("button", { name: "Save channel" }));
    expect(onSave).toHaveBeenLastCalledWith(expect.objectContaining({ secret: { action: "keep" } }));
  });
});
