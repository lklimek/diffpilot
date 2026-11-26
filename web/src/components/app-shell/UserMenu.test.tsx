import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import type { LlmActivity, LlmJob, LlmJobKind, WaitReason } from "../../api/generated/types";
import { clockText } from "../format/time";
import { UserMenu } from "./UserMenu";

const activity: LlmActivity = { running: [], queued: [], paused_until: null, last_call: null };
const now = new Date("2026-11-25T11:00:00Z");
const pausedUntil = "2026-11-25T13:00:00Z";
const at = "2026-11-25T10:42:00Z";
const running: LlmJob = {
  id: "running",
  owner: "self",
  kind: "ranking",
  subject: { label: "My work", href: "/" },
  since: at,
  queued_at: null,
  waits_for: null,
};
const queued: LlmJob = {
  id: "queued",
  owner: "self",
  kind: "flow_analysis",
  subject: { label: "acme/node-client#482", href: "/pr/acme/node-client/482" },
  since: null,
  queued_at: at,
  waits_for: { kind: "runner" },
};
const failed = { at, status: { state: "not_working", message: "Codex sign-in expired." } } as const;
const props = {
  login: "alice",
  settingsHref: "/settings",
  usageHref: "/settings#llm-usage",
  now,
  onSignOut: vi.fn(),
};

it.each([
  { state: "idle", snapshot: activity, name: "alice: account and LLM activity", attention: null },
  {
    state: "working",
    snapshot: { ...activity, running: [running], queued: [queued] },
    name: "alice: account and LLM activity",
    attention: null,
  },
  {
    state: "failed",
    snapshot: { ...activity, last_call: failed },
    name: "alice — LLM not working",
    attention: "failed",
  },
  {
    state: "paused",
    snapshot: { ...activity, paused_until: pausedUntil, last_call: failed },
    name: `alice — LLM paused until ${clockText(pausedUntil, now)}`,
    attention: "paused",
  },
])(
  "gives the trigger the correct name, tooltip and dot when $state",
  async ({ snapshot, name, attention }) => {
    const { container } = render(<UserMenu {...props} activity={snapshot} />);
    const trigger = screen.getByRole("button", { name });
    expect(trigger).toHaveAttribute("aria-haspopup", "dialog");
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    expect(container.querySelector("[data-llm-attention]")?.getAttribute("data-llm-attention") ?? null).toBe(
      attention,
    );
    if (attention) {
      await userEvent.hover(trigger);
      expect(screen.getByRole("tooltip")).toHaveTextContent(name);
    }
  },
);

it("shows own job details and folds the two lists independently without revealing other subjects", async () => {
  const user = userEvent.setup();
  const other = {
    ...queued,
    id: "other",
    owner: "other",
    subject: { label: "private/repo#7", href: "/pr/private/repo/7" },
  } as unknown as LlmJob;
  render(
    <UserMenu
      {...props}
      activity={{
        ...activity,
        running: Array.from({ length: 7 }, (_, index) => ({ ...running, id: `running-${index}` })),
        queued: [other, ...Array.from({ length: 6 }, (_, index) => ({ ...queued, id: `queued-${index}` }))],
      }}
    />,
  );
  await user.click(screen.getByRole("button", { name: "alice: account and LLM activity" }));
  const active = screen.getByRole("list", { name: "Running now (7)" });
  const waiting = screen.getByRole("list", { name: "Queued (7)" });
  expect(within(active).getAllByRole("listitem")).toHaveLength(5);
  expect(within(active).getAllByRole("link", { name: "My work" })[0]).toHaveAttribute("href", "/");
  expect(active).toHaveTextContent(`since ${clockText(at, now)}`);
  expect(within(waiting).getAllByRole("listitem")).toHaveLength(6);
  expect(waiting).toHaveTextContent("1 job of another user");
  expect(waiting).toHaveTextContent("Waits for a free runner");

  await user.click(screen.getByRole("button", { name: "Show all (7)" }));
  expect(within(active).getAllByRole("listitem")).toHaveLength(7);
  expect(within(waiting).getAllByRole("listitem")).toHaveLength(6);
  await user.click(screen.getByRole("button", { name: "Show all (6)" }));
  expect(within(waiting).getAllByRole("listitem")).toHaveLength(7);
  expect(document.body).not.toHaveTextContent("private/repo#7");
  expect(document.querySelector('a[href="/pr/private/repo/7"]')).toBeNull();
});

it("describes the typed usage and analysis wait reasons", async () => {
  render(
    <UserMenu
      {...props}
      activity={{
        ...activity,
        queued: [
          { ...queued, waits_for: { kind: "usage_limit", until: pausedUntil } },
          { ...queued, id: "ranking", kind: "ranking", waits_for: { kind: "analysis_running" } },
        ],
      }}
    />,
  );
  await userEvent.click(screen.getByRole("button", { name: /^alice/ }));
  expect(
    screen.getByText(`Waits for the usage limit to reset at ${clockText(pausedUntil, now)}`),
  ).toBeVisible();
  expect(screen.getByText("Waits for the review analysis to finish")).toBeVisible();
});

it("opens as a disclosure with normal Tab order and Escape returning to the trigger", async () => {
  const user = userEvent.setup();
  render(<UserMenu {...props} activity={activity} onHelp={vi.fn()} />);
  const trigger = screen.getByRole("button", { name: "alice: account and LLM activity" });
  await user.tab();
  await user.keyboard("{Enter}");
  expect(trigger).toHaveFocus();
  expect(trigger).toHaveAttribute("aria-expanded", "true");
  const disclosed = screen.getByRole("dialog", { name: "alice: account and LLM activity" });
  expect(disclosed).not.toHaveAttribute("aria-modal", "true");
  expect(trigger).toHaveAttribute("aria-controls", disclosed.id);
  expect(within(disclosed).getByText("Signed in as alice (GitHub)")).toBeVisible();
  expect(screen.queryByRole("menu")).toBeNull();
  await user.tab();
  expect(screen.getByRole("link", { name: "Usage and limits" })).toHaveFocus();
  await user.tab();
  expect(screen.getByRole("link", { name: "Settings" })).toHaveFocus();
  await user.tab();
  expect(screen.getByRole("button", { name: "Keyboard shortcuts" })).toHaveFocus();
  const helpTip = screen.getByRole("tooltip");
  expect(helpTip).toHaveTextContent("Show keyboard shortcuts");
  await user.tab();
  expect(helpTip).not.toBeVisible();
  await user.keyboard("{Escape}");
  expect(trigger).toHaveFocus();
  expect(trigger).toHaveAttribute("aria-expanded", "false");
});

it("shows an own subject as text if the server gives it an external URL", async () => {
  render(
    <UserMenu
      {...props}
      activity={{
        ...activity,
        running: [{ ...running, subject: { label: "My work", href: "//elsewhere.invalid/work" } }],
      }}
    />,
  );
  await userEvent.click(screen.getByRole("button", { name: "alice: account and LLM activity" }));
  expect(screen.getByRole("list", { name: "Running now (1)" })).toHaveTextContent("My work");
  expect(screen.queryByRole("link", { name: "My work" })).toBeNull();
});

it("keeps all five own jobs visible and summarizes other users separately in each list", async () => {
  const other = { owner: "other", kind: "ask" } as const;
  render(
    <UserMenu
      {...props}
      activity={{
        ...activity,
        running: [{ ...other, id: "other-running-1" }, running, { ...other, id: "other-running-2" }],
        queued: [
          { ...other, id: "other-queue-1" },
          { ...other, id: "other-queue-2" },
          { ...other, id: "other-queue-3" },
          ...Array.from({ length: 5 }, (_, index) => ({ ...queued, id: `own-${index}` })),
        ],
      }}
    />,
  );
  await userEvent.click(screen.getByRole("button", { name: /^alice/ }));
  const runningRows = within(screen.getByRole("list", { name: "Running now (3)" })).getAllByRole("listitem");
  expect(runningRows).toHaveLength(2);
  const otherRunning = within(runningRows.at(-1)!);
  expect(otherRunning.getByText("2 jobs of other users")).toBeVisible();
  expect(runningRows.at(-1)).not.toHaveTextContent(/since/);
  const queuedRows = within(screen.getByRole("list", { name: "Queued (8)" })).getAllByRole("listitem");
  expect(queuedRows).toHaveLength(6);
  expect(queuedRows.slice(0, 5).every((row) => row.textContent?.includes("acme/node-client#482"))).toBe(true);
  const otherQueued = within(queuedRows.at(-1)!);
  expect(otherQueued.getByText("3 jobs of other users")).toBeVisible();
  expect(queuedRows.at(-1)).not.toHaveTextContent(/queued/);
  expect(screen.queryByRole("button", { name: /Show all/ })).toBeNull();
});

it("keeps the banner silent on idle, running and recovery, and reports a failed call", () => {
  const { rerender } = render(<UserMenu {...props} activity={activity} />);
  expect(screen.getByRole("status")).toBeEmptyDOMElement();
  const notice = screen.getByRole("status");
  rerender(<UserMenu {...props} activity={{ ...activity, running: [running] }} />);
  expect(screen.getByRole("status")).toBeEmptyDOMElement();

  rerender(<UserMenu {...props} activity={{ ...activity, last_call: failed }} />);
  expect(screen.getByRole("status")).toBe(notice);
  expect(notice).toHaveTextContent("Last LLM call failed");
  rerender(<UserMenu {...props} activity={{ ...activity, last_call: failed, queued: [queued] }} />);
  expect(screen.getByRole("status")).toBe(notice);
  expect(notice).not.toHaveTextContent(/queued/);

  rerender(<UserMenu {...props} activity={activity} />);
  expect(screen.getByRole("status")).toBeEmptyDOMElement();
});

it("uses neutral labels for future job kinds and wait reasons", async () => {
  render(
    <UserMenu
      {...props}
      activity={{
        ...activity,
        queued: [
          {
            ...queued,
            kind: "toString" as LlmJobKind,
            waits_for: { kind: "provider_window" } as unknown as WaitReason,
          },
        ],
      }}
    />,
  );
  await userEvent.click(screen.getByRole("button", { name: /^alice/ }));
  const list = within(screen.getByRole("list", { name: "Queued (1)" }));
  expect(list.getByText(/^LLM job/)).toBeVisible();
  expect(list.getByText("Waiting")).toBeVisible();
});
