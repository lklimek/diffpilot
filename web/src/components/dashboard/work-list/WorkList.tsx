import { useEffect, useId, useRef, useState } from "react";
import type { Tier, TodosState, WorkItem } from "../../../api/generated/types";
import { AlarmRow } from "../alarm-row/AlarmRow";
import { TIER_RULE, tierName } from "../common/format";
import type { Navigate } from "../common/navigate";
import { Segmented } from "../common/Segmented";
import { WorkItemRow } from "../work-item/WorkItemRow";
import { dashboardShortcut, isTyping } from "./shortcuts";

/** Which GitHub items the list shows; MemCan TODOs are listed with each (http-api.md "Items"). */
export type KindFilter = "pr" | "issue" | "both";

interface WorkListProps {
  /** Ranked items in rank order (R1b). */
  items: WorkItem[];
  /** Kind switch; the screen opens it on `pr` (owner decision). */
  kind: KindFilter;
  onKindChange: (kind: KindFilter) => void;
  /** In-app href of a pull request's review (plan §F.1); built by the screen. */
  reviewHref: (item: WorkItem) => string;
  /** In-app navigation (links and `v`); without it links load the page. */
  onNavigate?: Navigate;
  /** `?` pressed. */
  onHelp?: () => void;
  /** Shortcuts on (off while a dialog is open). */
  keysEnabled?: boolean;
  /** The list is an earlier snapshot: its clock time (`08:12`) and what happens next; tints and frames the rows. */
  stale?: { at: string; note: string } | null;
  /** State of the MemCan TODOs in the snapshot (marks TODO rows MemCan could not refresh). */
  todos?: TodosState;
  now: Date;
}

const shows = (filter: KindFilter, item: WorkItem) =>
  item.kind === "todo" || filter === "both" || item.kind === filter;

const KIND_TEXT: Record<KindFilter, string> = {
  pr: "pull requests",
  issue: "issues",
  both: "pull requests or issues",
};

/** Groups of the list: P0 alarms, the tiers P1–P3, then "Not judged yet"; each keeps the server's rank order. */
export function groupItems(items: WorkItem[]) {
  const alarms = items.filter((item) => item.score.tier === "p0");
  const ranked = items.filter((item) => item.score.tier !== "p0");
  const tiers = (["p1", "p2", "p3"] as Exclude<Tier, "p0">[]).map((tier) => ({
    tier,
    items: ranked.filter((item) => item.score.state !== "unjudged" && item.score.tier === tier),
  }));
  return { alarms, tiers, notJudged: ranked.filter((item) => item.score.state === "unjudged") };
}

/**
 * The ranked list "Needs you" (mockup `renderList`): kind switch, P0 alarms (of every kind), tier groups and
 * "Not judged yet";
 * keys `j`/`k` move the cursor, Enter expands, `v` opens the PR's review, `g` the item, `?` the help.
 */
export function WorkList({
  items,
  kind,
  onKindChange,
  reviewHref,
  onNavigate,
  onHelp,
  keysEnabled = true,
  stale,
  todos,
  now,
}: WorkListProps) {
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
  const [cursor, setCursor] = useState<string | null>(null);
  /** Focus targets of the rows for `j`/`k`: a row's expander, an alarm's frame. */
  const targets = useRef(new Map<string, HTMLElement>());
  const titleId = useId();
  const staleId = useId();

  const shown = items.filter((item) => shows(kind, item));
  // P0 alarms stay visible whatever the kind switch says.
  const alarms = groupItems(items).alarms;
  const { tiers, notJudged } = groupItems(shown);
  const rows = [...alarms, ...tiers.flatMap((group) => group.items), ...notJudged];
  const listed = shown.length + alarms.filter((alarm) => !shows(kind, alarm)).length;
  const count = (k: KindFilter) => items.filter((item) => item.kind !== "todo" && shows(k, item)).length;
  const todoCount = items.filter((item) => item.kind === "todo").length;

  const toggle = (key: string) =>
    setExpanded((open) => {
      const next = new Set(open);
      if (!next.delete(key)) next.add(key);
      return next;
    });

  useEffect(() => {
    if (!keysEnabled) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.defaultPrevented || isTyping(event)) return;
      const action = dashboardShortcut(event.key);
      if (action === "help" && onHelp) {
        event.preventDefault();
        onHelp();
        return;
      }
      if (rows.length === 0) return;
      const at = rows.findIndex((item) => item.key === cursor);
      const current = at >= 0 ? rows[at] : undefined;
      if (action === "next" || action === "previous") {
        event.preventDefault();
        const next = rows[action === "next" ? Math.min(rows.length - 1, at + 1) : Math.max(0, at - 1)]!;
        setCursor(next.key);
        const target = targets.current.get(next.key);
        target?.focus();
        target?.scrollIntoView?.({ block: "nearest" });
      } else if (!current) {
        return;
      } else if (action === "toggle") {
        // A focused control handles Enter itself.
        if (event.target instanceof HTMLElement && event.target.closest("a, button, summary")) return;
        if (current.score.tier === "p0") return;
        event.preventDefault();
        toggle(current.key);
      } else if (action === "review" && current.kind === "pr") {
        event.preventDefault();
        const href = reviewHref(current);
        if (onNavigate) onNavigate(href);
        else window.location.assign(href);
      } else if (action === "open" && current.url) {
        event.preventDefault();
        window.open(current.url, "_blank", "noopener,noreferrer");
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  });

  const register = (key: string) => (element: HTMLElement | null) => {
    if (element) targets.current.set(key, element);
    else targets.current.delete(key);
  };
  const row = (item: WorkItem) => (
    <WorkItemRow
      key={item.key}
      item={item}
      expanded={expanded.has(item.key)}
      onToggle={() => toggle(item.key)}
      current={item.key === cursor}
      reviewHref={item.kind === "pr" ? reviewHref(item) : undefined}
      onNavigate={onNavigate}
      expanderRef={register(item.key)}
      todos={todos}
      now={now}
    />
  );
  const group = (title: string, rule: string, list: WorkItem[]) => (
    <div key={title} className="mt-1.5">
      <h3 className="flex items-baseline gap-2.5 px-0.5 pt-1 pb-1.5 text-xs font-bold tracking-wide text-muted uppercase">
        <span className="text-sm tracking-normal text-text">{title}</span>
        <span>{rule}</span>
        <span>· {list.length}</span>
      </h3>
      <ul className="m-0 list-none overflow-hidden rounded-lg border border-border bg-surface p-0">
        {list.map(row)}
      </ul>
    </div>
  );

  return (
    <section aria-labelledby={titleId} className="@container min-w-0">
      <div className="flex flex-wrap items-center gap-x-4 gap-y-2.5">
        <h2 id={titleId} className="text-[17px] font-semibold">
          Needs you
        </h2>
        <span className="text-xs text-muted">
          {listed} of {items.length} items, ranked
          {todoCount > 0 ? `; your ${todoCount} MemCan TODOs are listed with every choice` : ""}
        </span>
        <span className="flex-1" />
        <Segmented
          label="Show pull requests, issues or both"
          value={kind}
          onChange={onKindChange}
          options={[
            {
              value: "pr",
              label: (
                <>
                  PRs <b>{count("pr")}</b>
                </>
              ),
              tip: "Show pull requests (and your MemCan TODOs)",
            },
            {
              value: "issue",
              label: (
                <>
                  Issues <b>{count("issue")}</b>
                </>
              ),
              tip: "Show issues (and your MemCan TODOs)",
            },
            {
              value: "both",
              label: (
                <>
                  Both <b>{count("both")}</b>
                </>
              ),
              tip: "Show pull requests and issues (and your MemCan TODOs)",
            },
          ]}
        />
      </div>
      {/* A stale list keeps full text contrast: a tinted dashed frame and its chip mark it. */}
      <div
        data-stale={stale ? true : undefined}
        aria-describedby={stale ? staleId : undefined}
        className={
          stale ? "mt-2.5 rounded-md border border-dashed border-border bg-surface-2 px-2 pb-1.5" : ""
        }
      >
        {stale && (
          <p id={staleId} className="m-0 flex flex-wrap items-center gap-x-2 pt-0.5 text-xs text-muted">
            <span className="relative -top-2 rounded-full border border-border bg-surface px-2 py-px font-semibold text-text">
              Previous snapshot · {stale.at}
            </span>{" "}
            {stale.note}
          </p>
        )}
        {alarms.length > 0 && (
          <div className="mt-2.5 grid gap-2.5">
            {alarms.map((item) => (
              <AlarmRow
                key={item.key}
                item={item}
                current={item.key === cursor}
                focusRef={register(item.key)}
                reviewHref={item.kind === "pr" ? reviewHref(item) : undefined}
                onNavigate={onNavigate}
              />
            ))}
          </div>
        )}
        {tiers.map(
          ({ tier, items: list }) => list.length > 0 && group(tierName(tier), TIER_RULE[tier], list),
        )}
        {notJudged.length > 0 && group("Not judged yet", "new since the last ranking", notJudged)}
        {count(kind) === 0 && (
          <p className="px-0.5 py-2 text-[12.5px] text-muted">No {KIND_TEXT[kind]} need you.</p>
        )}
      </div>
    </section>
  );
}
