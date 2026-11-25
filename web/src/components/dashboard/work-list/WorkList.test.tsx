import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState, type ComponentProps } from "react";
import { describe, expect, it, vi } from "vitest";
import type { TodosState, WorkItem } from "../../../api/generated/types";
import { rowOpenLabel } from "../common/OpenExternal";
import type { Filter } from "../filter-chips/filters";
import { ITEMS, NOW, PROVISIONAL_ITEMS } from "../test-fixtures";
import { reviewAction } from "../work-item/ItemAction";
import { ALL_KINDS, WorkList, type ItemKind } from "./WorkList";

const reviewHref = (item: WorkItem) => `/review?pr=${encodeURIComponent(item.url ?? "")}`;

function List(props: {
  items?: WorkItem[];
  initial?: ItemKind[];
  onNavigate?: (href: string) => void;
  onHelp?: () => void;
  todos?: TodosState;
}) {
  const [kinds, setKinds] = useState<ItemKind[]>(props.initial ?? ALL_KINDS);
  const [filters, setFilters] = useState<Filter[]>([]);
  return (
    <>
      <input aria-label="search" />
      <WorkList
        items={props.items ?? ITEMS}
        kinds={kinds}
        onKindsChange={setKinds}
        filters={filters}
        onFiltersChange={setFilters}
        reviewHref={reviewHref}
        onNavigate={props.onNavigate}
        onHelp={props.onHelp}
        todos={props.todos}
        now={NOW}
      />
    </>
  );
}

const rowIds = () =>
  [...document.querySelectorAll("li[data-item]")].map((li) => li.getAttribute("data-item"));

describe("WorkList", () => {
  it("the row menu snoozes the selected item and P0 alarms cannot be snoozed", async () => {
    const onSnooze = vi.fn();
    render(
      <WorkList
        items={ITEMS}
        kinds={ALL_KINDS}
        onKindsChange={vi.fn()}
        reviewHref={reviewHref}
        onSnooze={onSnooze}
        snoozingKeys={[]}
        now={NOW}
      />,
    );
    const first = ITEMS.find((item) => item.score.tier !== "p0")!;
    const alarm = ITEMS.find((item) => item.score.tier === "p0")!;
    await userEvent.click(screen.getByRole("button", { name: `More actions for ${alarm.id}` }));
    expect(screen.queryByRole("menuitem", { name: "Snooze…" })).toBeNull();
    expect(screen.getByRole("menuitem", { name: "Open on GitHub, new tab" })).toHaveAttribute(
      "href",
      alarm.url,
    );
    await userEvent.keyboard("{Escape}");
    await userEvent.click(screen.getByRole("button", { name: `More actions for ${first.id}` }));
    await userEvent.click(screen.getByRole("menuitem", { name: "Snooze…" }));
    await userEvent.click(
      within(screen.getByRole("group", { name: `Snooze ${first.id}` })).getByRole("button", {
        name: "3 days",
      }),
    );
    expect(onSnooze).toHaveBeenCalledWith(first, 3);
  });

  it("shows a plug-in's statement on its pull request's row, the further points once the row is expanded", async () => {
    const note = {
      plugin: "pr-hygiene",
      source: "PR Hygiene",
      url: "https://pr-hygiene.example.test/api/v1/prs/acme/node-client/482",
      as_of: "2026-11-18T10:12:00Z",
      summary: "Author must post /self-reviewed 1a2b3c",
      details: [
        "Author must post /self-reviewed 1a2b3c",
        "Human approval or objection resolution is required",
      ],
      facts: [{ label: "Stage", value: "self-review" }],
    };
    const items = ITEMS.map((item) => (item.id === "NC-482" ? { ...item, plugin_notes: [note] } : item));
    render(<List items={items} />);
    const row = document.querySelector("li[data-item][data-item*='482']") as HTMLElement;
    expect(row).toHaveTextContent("PR Hygiene: Author must post /self-reviewed 1a2b3c");
    expect(within(row).getByText("Human approval or objection resolution is required")).not.toBeVisible();
    expect(document.querySelectorAll("[data-plugin-note]")).toHaveLength(1);
    await userEvent.click(within(row).getByRole("button", { name: "Details of NC-482" }));
    expect(
      within(within(row).getByRole("list", { name: "PR Hygiene: further points" })).getByText(
        "Human approval or objection resolution is required",
      ),
    ).toBeVisible();
  });

  it("PRs, Issues and TODOs are independent toggles, all on at first; kinds combine; P0 alarms always show", async () => {
    render(<List />);
    expect(screen.getByRole("group", { name: "Kinds shown" })).toBeInTheDocument();
    const prs = screen.getByRole("button", { name: /PRs 8/ });
    const issues = screen.getByRole("button", { name: /Issues 3/ });
    const todos = screen.getByRole("button", { name: /TODOs 2/ });
    for (const button of [prs, issues, todos]) expect(button).toHaveAttribute("aria-pressed", "true");
    expect(rowIds()).toHaveLength(12);

    await userEvent.click(issues);
    expect(issues).toHaveAttribute("aria-pressed", "false");
    expect(rowIds()).toContain("acme~platform~pr~1203");
    expect(rowIds()).toContain("memcan~3fa91c2e-0000-4000-8000-000000000000");
    expect(rowIds()).not.toContain("acme~platform~issue~1210");
    // The P0 issue stays visible as an alarm.
    expect(screen.getByRole("group", { name: "P0 alarm PLAT-1187" })).toBeInTheDocument();

    await userEvent.click(prs);
    expect(rowIds()).toHaveLength(2);
    expect(rowIds().every((id) => id?.startsWith("memcan~"))).toBe(true);
    await userEvent.click(issues);
    expect(rowIds()).toContain("acme~platform~issue~1210");
    expect(rowIds()).not.toContain("acme~platform~pr~1203");
  });

  it("uses a row size as a removable filter and keeps P0 alarms visible", async () => {
    render(<List />);
    expect(screen.queryByRole("group", { name: "Sizes shown" })).toBeNull();
    await userEvent.click(screen.getAllByRole("button", { name: "Filter by size medium" })[0]!);
    expect(rowIds()).toContain("acme~platform~pr~1203");
    expect(rowIds()).not.toContain("acme~platform~issue~1210");
    expect(screen.getByRole("group", { name: "P0 alarm PLAT-1187" })).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Remove filter size medium" }));
    expect(rowIds()).toContain("acme~platform~issue~1210");
  });

  it("each PR row shows its size pill; issues have none", () => {
    render(<List />);
    const row = (key: string) => document.querySelector(`li[data-item="${key}"]`)!;
    expect(
      within(row("acme~platform~pr~1203") as HTMLElement).getByRole("button", {
        name: "Filter by size medium",
      }),
    ).toBeInTheDocument();
    expect(row("acme~platform~issue~1210").querySelector("[data-size-pill]")).toBeNull();
  });

  it("the last kind on stays on and says why; keyboard toggles", async () => {
    render(<List initial={["pr", "issue"]} />);
    const prs = screen.getByRole("button", { name: /PRs 8/ });
    const issues = screen.getByRole("button", { name: /Issues 3/ });
    expect(prs).not.toHaveAttribute("aria-disabled");
    issues.focus();
    await userEvent.keyboard("{Enter}");
    expect(issues).toHaveAttribute("aria-pressed", "false");
    expect(prs).toHaveAttribute("aria-disabled", "true");
    expect(prs).toHaveAccessibleDescription(/At least one kind stays shown/);
    await userEvent.click(prs);
    expect(prs).toHaveAttribute("aria-pressed", "true");
    expect(rowIds()).toContain("acme~platform~pr~1203");
    issues.focus();
    await userEvent.keyboard(" ");
    expect(prs).not.toHaveAttribute("aria-disabled");
    expect(issues).toHaveAttribute("aria-pressed", "true");
  });

  it("the TODOs toggle is hidden while TODOs are off and none are listed", () => {
    const noTodos = ITEMS.filter((item) => item.kind !== "todo");
    const { unmount } = render(
      <List items={noTodos} todos={{ status: "off", read_at: null, failed_at: null }} />,
    );
    expect(screen.queryByRole("button", { name: /TODOs/ })).toBeNull();
    expect(
      within(screen.getByRole("group", { name: "Kinds shown" })).getAllByRole("button", { pressed: true }),
    ).toHaveLength(2);
    unmount();
    // Stored as TODOs only while TODOs are off: the other kinds show instead of nothing.
    const again = render(
      <List
        items={noTodos}
        initial={["todo"]}
        todos={{ status: "not_configured", read_at: null, failed_at: null }}
      />,
    );
    expect(screen.getByRole("button", { name: /PRs 8/ })).toHaveAttribute("aria-pressed", "true");
    expect(rowIds()).toContain("acme~platform~pr~1203");
    again.unmount();
    render(
      <List items={noTodos} todos={{ status: "read", read_at: "2026-11-18T09:24:00Z", failed_at: null }} />,
    );
    expect(screen.getByRole("button", { name: /TODOs 0/ })).toHaveAttribute("aria-pressed", "true");
  });

  it("groups items by tier in the server's order, with new items under Not judged yet", () => {
    render(<List items={PROVISIONAL_ITEMS} />);
    const headings = screen.getAllByRole("heading", { level: 3 }).map((h) => h.textContent);
    expect(headings).toEqual([
      expect.stringMatching(/^P1/),
      expect.stringMatching(/^P2/),
      expect.stringMatching(/^P3/),
      expect.stringMatching(/^Not judged yet/),
    ]);
    const notJudged = screen.getByRole("heading", { name: /Not judged yet/ })
      .nextElementSibling as HTMLElement;
    expect(within(notJudged).getByText("fix(ui): restore the focus ring on tabs")).toBeInTheDocument();
    // State-neutral: after a failed, cancelled or capped ranking nothing ranks it "next".
    expect(within(notJudged).getByText("◌ not judged yet")).toBeInTheDocument();
  });

  it("marks provisional scores with ~ and every estimate in the breakdown", async () => {
    render(<List items={PROVISIONAL_ITEMS} />);
    const score = screen.getByRole("button", { name: /Score provisional 90, show breakdown/ });
    expect(score).toHaveTextContent("~90");
    await userEvent.click(score);
    const severity = document.querySelector(
      '[data-item="acme~wallet-ui~pr~320"] tr[data-part="defect_severity"]',
    )!;
    expect(severity).toHaveTextContent("estimate");
    expect(severity).toHaveTextContent("estimate until judged");
  });

  it("every LLM text carries the LLM tag", async () => {
    render(<List items={PROVISIONAL_ITEMS} />);
    for (const button of screen.getAllByRole("button", { name: /^Details of/ }))
      await userEvent.click(button);
    const texts = [...document.querySelectorAll("[data-llm-text]")];
    expect(texts.length).toBeGreaterThan(20);
    for (const text of texts) expect(text.querySelector('[data-provenance="llm"]')).not.toBeNull();
    // LLM-judged parts name the LLM; computed parts say computed.
    for (const row of document.querySelectorAll("tr[data-part]")) {
      expect(row.querySelector("[data-source]")?.textContent).toMatch(
        /LLM|computed|estimate|not applicable|pending/,
      );
    }
  });

  it("j / k move the cursor, Enter expands, v opens the review, g the item, ? the help", () => {
    const onNavigate = vi.fn();
    const onHelp = vi.fn();
    const open = vi.spyOn(window, "open").mockReturnValue(null);
    render(<List onNavigate={onNavigate} onHelp={onHelp} />);
    // The P0 alarm comes first, whatever the kind switch says.
    fireEvent.keyDown(document.body, { key: "j" });
    const alarm = screen.getByRole("group", { name: "P0 alarm PLAT-1187" });
    expect(alarm).toHaveAttribute("data-current", "true");
    expect(document.activeElement).toBe(alarm);
    fireEvent.keyDown(document.body, { key: "g" });
    expect(open).toHaveBeenCalledWith(
      "https://github.com/acme/platform/issues/1187",
      "_blank",
      "noopener,noreferrer",
    );
    open.mockClear();

    fireEvent.keyDown(document.body, { key: "j" });
    expect(alarm).not.toHaveAttribute("data-current");
    const first = document.querySelector("li[data-current]")!;
    expect(first.getAttribute("data-item")).toBe("acme~platform~pr~1203");
    expect(document.activeElement).toBe(
      within(first as HTMLElement).getByRole("button", { name: "Details of PLAT-1203" }),
    );

    fireEvent.keyDown(document.body, { key: "j" });
    fireEvent.keyDown(document.body, { key: "k" });
    expect(document.querySelector("li[data-current]")?.getAttribute("data-item")).toBe(
      "acme~platform~pr~1203",
    );

    fireEvent.keyDown(document.body, { key: "Enter" });
    expect(screen.getByRole("button", { name: "Details of PLAT-1203" })).toHaveAttribute(
      "aria-expanded",
      "true",
    );

    fireEvent.keyDown(document.body, { key: "v" });
    expect(onNavigate).toHaveBeenCalledWith(
      `/review?pr=${encodeURIComponent("https://github.com/acme/platform/pull/1203")}`,
    );

    fireEvent.keyDown(document.body, { key: "g" });
    expect(open).toHaveBeenCalledWith(
      "https://github.com/acme/platform/pull/1203",
      "_blank",
      "noopener,noreferrer",
    );

    fireEvent.keyDown(document.body, { key: "?" });
    expect(onHelp).toHaveBeenCalledOnce();
    open.mockRestore();
  });

  it("ignores shortcuts while typing or with a modifier", () => {
    const onHelp = vi.fn();
    render(<List onHelp={onHelp} />);
    fireEvent.keyDown(screen.getByRole("textbox", { name: "search" }), { key: "j" });
    fireEvent.keyDown(document.body, { key: "j", ctrlKey: true });
    fireEvent.keyDown(screen.getByRole("textbox", { name: "search" }), { key: "?" });
    expect(document.querySelector("li[data-current]")).toBeNull();
    expect(onHelp).not.toHaveBeenCalled();
  });

  it("links a PR to its review: Review without an analysis, Open when the head moved", () => {
    render(<List items={PROVISIONAL_ITEMS} />);
    const plain = document.querySelector('[data-item="acme~platform~pr~1203"] a[data-review-link]');
    expect(plain).toHaveTextContent("Review");
    const moved = document.querySelector('[data-item="acme~node-client~pr~482"] a[data-review-link]');
    expect(moved).toHaveTextContent("Open");
    expect(moved).not.toHaveTextContent("review");
    expect(moved).toHaveAttribute(
      "href",
      `/review?pr=${encodeURIComponent("https://github.com/acme/node-client/pull/482")}`,
    );
  });

  it("review links have unique accessible names that start with the visible label", () => {
    render(<List items={PROVISIONAL_ITEMS} />);
    const links = [...document.querySelectorAll<HTMLAnchorElement>("a[data-review-link]")];
    expect(links.length).toBeGreaterThan(1);
    const names = links.map((a) => a.getAttribute("aria-label") ?? "");
    for (const a of links) {
      const name = a.getAttribute("aria-label") ?? "";
      expect(name.startsWith(a.textContent ?? "?")).toBe(true);
      expect(name).toMatch(/#\d+ /);
    }
    expect(new Set(names).size).toBe(names.length);
    expect(screen.getByRole("link", { name: /^Open review of #482 / })).toBeVisible();
    expect(screen.getByRole("link", { name: /^Review pull request #1203 / })).toBeVisible();
  });

  it.each([
    [{ head_moved: false, has_my_state: true }, /ticks and answers are kept/, true],
    [{ head_moved: false, has_my_state: false }, /^Opens your review\./, false],
    [{ head_moved: true, has_my_state: true }, /^The PR moved\./, false],
  ])("every Open link says it starts nothing (%j)", (review, extra, kept) => {
    const item = PROVISIONAL_ITEMS.find((i) => i.kind === "pr" && i.review);
    expect(item).toBeDefined();
    const { tip, label } = reviewAction({
      ...item!,
      review: { ...item!.review!, ...review, analysed_head: "0123456789abcdef" },
    });
    expect(label).toBe("Open");
    expect(tip).toMatch(/starts nothing\./i);
    expect(tip).toMatch(extra);
    expect(tip.includes("ticks and answers are kept")).toBe(kept);
  });

  it("a plain click on a review link navigates in the app; a modified click keeps the browser's", () => {
    const onNavigate = vi.fn();
    render(<List onNavigate={onNavigate} />);
    const link = document.querySelector('[data-item="acme~platform~pr~1203"] a[data-review-link]')!;
    fireEvent.click(link, { ctrlKey: true });
    expect(onNavigate).not.toHaveBeenCalled();
    fireEvent.click(link);
    expect(onNavigate).toHaveBeenCalledOnce();
  });

  it("marks a stale list by its note and a dashed frame, never by lowering text contrast", () => {
    render(
      <WorkList
        items={ITEMS}
        kinds={["pr"]}
        onKindsChange={() => {}}
        reviewHref={reviewHref}
        stale={{ at: "08:12", note: "It is replaced when a scan finishes." }}
        now={NOW}
      />,
    );
    const rows = document.querySelector("[data-stale]")!;
    expect(rows).toHaveAccessibleDescription(
      "Previous snapshot · 08:12 It is replaced when a scan finishes.",
    );
    expect(within(rows as HTMLElement).getByText("Previous snapshot · 08:12")).toBeInTheDocument();
    expect(rows.className).not.toMatch(/opacity/);
    expect(rows.className).toMatch(/border-dashed/);
    expect(rows.className).toMatch(/bg-surface-2/);
  });

  it("a P0 alarm shows its points and the same score breakdown as a ranked row (button and Enter)", async () => {
    render(<List />);
    const alarm = screen.getByRole("group", { name: "P0 alarm PLAT-1187" });
    const score = within(alarm).getByRole("button", { name: "Score 107, show breakdown" });
    expect(score).toHaveTextContent(/^107$/);
    expect(within(alarm).queryByRole("heading", { name: "Score breakdown" })).toBeNull();
    await userEvent.click(score);
    expect(within(alarm).getByRole("heading", { name: "Score breakdown" })).toBeVisible();
    expect(within(alarm).getByText(/P0 alarm \(score 107 kept for reference\)/)).toBeVisible();
    expect(score).toHaveAccessibleName("Score 107, hide breakdown");
    await userEvent.click(score);
    expect(within(alarm).queryByRole("heading", { name: "Score breakdown" })).toBeNull();
    // Enter on the focused alarm toggles it like a ranked row.
    fireEvent.keyDown(document.body, { key: "j" });
    expect(document.activeElement).toBe(alarm);
    fireEvent.keyDown(document.body, { key: "Enter" });
    const detail = document.getElementById(score.getAttribute("aria-controls")!)!;
    expect(within(detail).getByRole("heading", { name: "Score breakdown" })).toBeVisible();
    // Enter on the score button itself toggles once.
    score.focus();
    await userEvent.keyboard("{Enter}");
    expect(detail).not.toBeVisible();
    await userEvent.keyboard("{Enter}");
    expect(within(detail).getByRole("heading", { name: "Score breakdown" })).toBeVisible();
  });

  it("counts the P0 alarms of a hidden kind among the listed items", async () => {
    render(<List initial={["pr", "todo"]} />);
    const listed = () =>
      document.querySelectorAll("li[data-item]").length + document.querySelectorAll("[data-alarm]").length;
    const header = () => screen.getByText(/of 13 items, ranked/);
    expect(header()).toHaveTextContent(`${listed()} of 13 items, ranked`);
    expect(header()).toHaveTextContent("11 of 13 items, ranked");
    await userEvent.click(screen.getByRole("button", { name: /Issues 3/ }));
    expect(header()).toHaveTextContent(`${listed()} of 13 items, ranked`);
    expect(header()).not.toHaveTextContent(/listed with every choice/);
  });

  it("says when no item of the chosen kind needs the user", async () => {
    const { unmount } = render(
      <List items={ITEMS.filter((item) => item.kind !== "issue")} initial={["issue"]} />,
    );
    expect(screen.getByText("No issues need you.")).toBeInTheDocument();
    unmount();
    render(
      <List
        items={ITEMS.filter((item) => item.kind === "pr")}
        initial={["issue", "todo"]}
        todos={{ status: "read", read_at: "2026-11-18T09:24:00Z", failed_at: null }}
      />,
    );
    expect(screen.getByText("No issues or TODOs need you.")).toBeInTheDocument();
  });

  it("tags and scores show their tooltip on keyboard focus", () => {
    render(<List />);
    const tag = document.querySelector('[data-item="acme~platform~pr~1203"] [data-fact-tag]') as HTMLElement;
    fireEvent.focus(tag);
    expect(screen.getByRole("tooltip")).toHaveTextContent(/requested/);
    fireEvent.blur(tag);
    const score = screen.getByRole("button", { name: /^Score 148/ });
    fireEvent.focus(score);
    expect(screen.getByRole("tooltip")).toHaveTextContent("Score 148 = sum of 12 parts");
  });
});

describe("WorkList: a visible link out on every row (F38)", () => {
  const pr = ITEMS.find((item) => item.kind === "pr" && item.score.tier !== "p0")!;
  const todo = ITEMS.find((item) => item.kind === "todo")!;
  const alarm = ITEMS.find((item) => item.score.tier === "p0")!;

  it("each row and the P0 alarm have an icon link named and tipped 'Open <id> on GitHub', in a new tab", () => {
    render(<List />);
    for (const item of [pr, alarm]) {
      const link = screen.getByRole("link", { name: rowOpenLabel(item) });
      expect(link).toHaveAttribute("href", item.url);
      expect(link).toHaveAttribute("target", "_blank");
      expect(link).toHaveAttribute("rel", "noopener noreferrer");
      expect(link).toHaveAccessibleDescription(rowOpenLabel(item));
      expect(link.querySelector('[data-icon="github"]')).not.toBeNull();
      const duplicate = document.querySelector(`a[aria-hidden="true"][href="${item.url}"]`);
      expect(duplicate).toHaveAttribute("tabindex", "-1");
      expect(link.className).toMatch(/min-h-6/);
      expect(link.className).toMatch(/min-w-6/);
    }
  });

  it("a TODO with a link opens it in MemCan; no icon without a link or with a non-http URL", () => {
    const items = [
      { ...todo, url: "http://memcan.example/ui/tasks/1" },
      { ...pr, key: "no-url", id: "NOURL-1", url: null },
      { ...pr, key: "js-url", id: "JS-1", url: "javascript:alert(1)" },
    ];
    render(<List items={items} />);
    expect(screen.getByRole("link", { name: "Open TODO in MemCan (new tab)" })).toHaveAttribute(
      "href",
      "http://memcan.example/ui/tasks/1",
    );
    expect(screen.queryByRole("link", { name: /NOURL-1/ })).toBeNull();
    expect(screen.queryByRole("link", { name: /JS-1/ })).toBeNull();
    expect(document.querySelector('a[href^="javascript"]')).toBeNull();
  });

  it("clicking the icon does not expand or select the row", async () => {
    render(<List />);
    const link = screen.getByRole("link", { name: rowOpenLabel(pr) });
    link.addEventListener("click", (event) => event.preventDefault());
    const bubbled = vi.fn();
    // React handlers run at the root; a stopped click does not reach the body beyond it.
    document.body.addEventListener("click", bubbled);
    await userEvent.click(link);
    document.body.removeEventListener("click", bubbled);
    expect(bubbled).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: `Details of ${pr.id}` })).toHaveAttribute(
      "aria-expanded",
      "false",
    );
    expect(document.querySelector("[data-current]")).toBeNull();
  });
});

describe("WorkList: TODO rows kept while MemCan was unreachable (plan §G Failures)", () => {
  const row = (key: string) => document.querySelector<HTMLElement>(`li[data-item="${key}"]`)!;
  const todoKeys = ITEMS.filter((item) => item.kind === "todo").map((item) => item.key);
  const otherKey = ITEMS.find((item) => item.kind === "pr")!.key;

  it("each kept TODO row says it was not refreshed, with the time MemCan was unreachable", () => {
    expect(todoKeys.length).toBeGreaterThan(0);
    render(
      <List
        todos={{
          status: "not_refreshed",
          read_at: "2026-11-18T08:12:00Z",
          failed_at: "2026-11-18T09:24:00Z",
        }}
      />,
    );
    for (const key of todoKeys) {
      const mark = within(row(key)).getByText(/not refreshed \(MemCan unreachable at \d\d:\d\d\)/);
      expect(mark).toHaveAccessibleDescription(/MemCan could not be reached/);
    }
    expect(row(otherKey)).not.toHaveTextContent(/not refreshed/);
  });

  it("without the time it still says so; TODOs read in this scan carry no mark", () => {
    const { unmount } = render(<List todos={{ status: "not_refreshed", read_at: null, failed_at: null }} />);
    expect(row(todoKeys[0]!)).toHaveTextContent("not refreshed (MemCan unreachable)");
    unmount();
    render(<List todos={{ status: "read", read_at: "2026-11-18T09:24:00Z", failed_at: null }} />);
    expect(row(todoKeys[0]!)).not.toHaveTextContent(/not refreshed/);
  });
});

describe("WorkList: comments of a pull request (owner 2026-11-23)", () => {
  const commentsHref = (item: WorkItem) =>
    `/pr/${item.repo!.owner}/${item.repo!.name}/${item.number}/comments`;
  const counted: WorkItem = {
    ...ITEMS.find((item) => item.id === "NC-482")!,
    comments: { total: 12, needs_you: 5 },
  };
  const unread: WorkItem = {
    ...ITEMS.find((item) => item.kind === "pr" && item.id !== "NC-482")!,
    comments: undefined,
  };
  // A ranked issue (a P0 alarm is drawn by AlarmRow, without the comments line).
  const issue = ITEMS.find((item) => item.kind === "issue" && item.score.tier !== "p0")!;
  const list = (props: Partial<ComponentProps<typeof WorkList>>) => (
    <WorkList
      items={[counted, unread, issue]}
      kinds={ALL_KINDS}
      onKindsChange={vi.fn()}
      reviewHref={reviewHref}
      commentsHref={commentsHref}
      now={NOW}
      {...props}
    />
  );

  it("counted comments link to the pull request's Comments; a pull request without counts offers Watch", async () => {
    const onNavigate = vi.fn();
    const onWatch = vi.fn();
    render(list({ onNavigate, onWatch }));
    const link = screen.getByRole("link", { name: /^12 comments · 5 need you: comments of #482 / });
    expect(link).toHaveAttribute("href", "/pr/acme/node-client/482/comments");
    expect(link).toHaveAccessibleDescription(/counted from its last read/);
    await userEvent.click(link);
    expect(onNavigate).toHaveBeenCalledWith("/pr/acme/node-client/482/comments");
    await userEvent.click(screen.getByRole("button", { name: `More actions for ${unread.id}` }));
    const watch = screen.getByRole("menuitemcheckbox", { name: "Watch comments" });
    expect(watch).toHaveAttribute("aria-checked", "false");
    await userEvent.click(watch);
    expect(onWatch).toHaveBeenCalledWith(unread, true);
    const issueRow = document.querySelector<HTMLElement>(`li[data-item="${issue.key}"]`)!;
    expect(within(issueRow).queryByRole("button", { name: /^Watch/ })).toBeNull();
    expect(issueRow.querySelector("[data-comments-link]")).toBeNull();
  });

  it("one comment reads in the singular; a Watch being saved waits, a saved one reads Watching", async () => {
    const onWatch = vi.fn();
    const one: WorkItem = { ...counted, comments: { total: 1, needs_you: 1 } };
    const { rerender } = render(list({ items: [one, unread], onWatch, watchingKeys: [unread.key] }));
    expect(screen.getByRole("link", { name: /^1 comment · 1 needs you:/ })).toBeVisible();
    const menu = screen.getByRole("button", { name: `More actions for ${unread.id}` });
    await userEvent.click(menu);
    const saving = screen.getByRole("menuitemcheckbox", { name: "Watch comments" });
    expect(saving).toHaveAttribute("aria-disabled", "true");
    await userEvent.click(saving);
    expect(onWatch).not.toHaveBeenCalled();
    await userEvent.keyboard("{Escape}");
    const watched = { ...unread, watched: true };
    rerender(list({ items: [one, watched], onWatch }));
    expect(screen.getByText("Watching, comments not read yet")).toBeVisible();
    await userEvent.click(menu);
    const watching = screen.getByRole("menuitemcheckbox", { name: "Watch comments" });
    expect(watching).toHaveAttribute("aria-checked", "true");
    await userEvent.click(watching);
    expect(onWatch).toHaveBeenCalledWith(watched, false);
  });

  it("a pull request the user watches whose comments were never read reads Watching, not Watch", () => {
    render(list({ items: [{ ...unread, watched: true }], onWatch: vi.fn() }));
    expect(screen.queryByRole("button", { name: /^Watch/ })).toBeNull();
    expect(screen.getByText("Watching, comments not read yet")).toBeVisible();
  });

  it("a private pull request without counts says why, and offers no Watch", () => {
    render(list({ items: [{ ...unread, private: true }], onWatch: vi.fn() }));
    expect(screen.queryByRole("button", { name: /^Watch/ })).toBeNull();
    expect(screen.getByText("comments not read: private repository")).toBeVisible();
  });

  it("without onWatch no Watch is offered", () => {
    render(list({}));
    expect(screen.queryByRole("button", { name: /^Watch/ })).toBeNull();
  });
});
