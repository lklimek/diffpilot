import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import type { Exclusions, Tier, TodosState, WorkItem, StatusCounts } from "../../../api/generated/types";
import { AlarmRow } from "../alarm-row/AlarmRow";
import { TIER_RULE, tierName } from "../common/format";
import type { Navigate } from "../common/navigate";
import { httpUrl } from "../common/OpenExternal";
import { ToggleGroup } from "../common/ToggleGroup";
import { WorkItemRow } from "../work-item/WorkItemRow";
import { ExcludedItems, exclusionDescription } from "../excluded-items/ExcludedItems";
import { FilterChips } from "../filter-chips/FilterChips";
import {
  addFilter,
  itemFilters,
  matchesFilters,
  tagFilter,
  type Filter,
  type FilterValue,
} from "../filter-chips/filters";
import { workTags } from "../work-item/WorkFacts";
import { getStatusCounts, matchesStatus, StatusFilter, type WorkStatus } from "./StatusFilter";
import { dashboardShortcut, isTyping } from "./shortcuts";

/** A kind of listed item: pull request, issue or MemCan TODO. */
export type ItemKind = WorkItem["kind"];

/** Every kind, in toggle order. */
export const ALL_KINDS: ItemKind[] = ["pr", "issue", "todo"];

interface WorkListProps {
  /** Ranked items in rank order (R1b). */
  items: WorkItem[];
  /** Kinds shown (independent toggles; all on by default). */
  kinds: ItemKind[];
  onKindsChange: (kinds: ItemKind[]) => void;
  filters?: Filter[];
  onFiltersChange?: (filters: Filter[]) => void;
  statusCounts?: StatusCounts;
  statusDescriptions?: Partial<Record<WorkStatus, string>>;
  exclusions?: Exclusions | null;
  guideOrigin?: string;
  onWake?: (key: string) => void;
  wakingKeys?: string[];
  onRejudge?: (key: string) => void;
  rejudgingKeys?: string[];
  rankingNotice?: string | null;
  usageHref?: string;
  snapshotAt?: string | null;
  /** In-app href of a pull request's review (plan §F.1); built by the screen. */
  reviewHref: (item: WorkItem) => string;
  /** In-app href of a pull request's Comments (owner 2026-11-23); built by the screen. */
  commentsHref?: (item: WorkItem) => string;
  onWatch?: (item: WorkItem, watching: boolean) => void;
  /** Keys of the pull requests whose Watch is being saved. */
  watchingKeys?: string[];
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
  empty?: ReactNode;
  /** P0 alarms are never snoozed. */
  onSnooze?: (item: WorkItem, days: number) => void;
  /** Keys of the items whose snooze is being saved. */
  snoozingKeys?: string[];
  now: Date;
}

const KIND_LABEL: Record<ItemKind, string> = { pr: "PRs", issue: "Issues", todo: "TODOs" };

const KIND_TEXT: Record<ItemKind, string> = {
  pr: "pull requests",
  issue: "issues",
  todo: "TODOs",
};

/** `issues`, `issues or TODOs`, `pull requests, issues or TODOs`. */
const kindsText = (kinds: ItemKind[]) => {
  const names = kinds.map((kind) => KIND_TEXT[kind]);
  return names.length > 1 ? `${names.slice(0, -1).join(", ")} or ${names.at(-1)}` : (names[0] ?? "");
};

/** MemCan TODOs can be in the snapshot: TODOs are read (or kept), or some are listed. */
const todosPossible = (items: WorkItem[], todos?: TodosState) =>
  todos?.status === "read" || todos?.status === "not_refreshed" || items.some((item) => item.kind === "todo");

/** Groups of the list: P0 alarms, the tiers P1–P3, then "Not judged yet"; each keeps the server's rank order. */
export function groupItems(items: WorkItem[]) {
  const alarms = items.filter((item) => item.score.tier === "p0");
  const ranked = items.filter((item) => item.score.tier !== "p0");
  const notJudged = (item: WorkItem) => matchesStatus(item, "not_judged");
  const tiers = (["p1", "p2", "p3"] as Exclude<Tier, "p0">[]).map((tier) => ({
    tier,
    items: ranked.filter((item) => !notJudged(item) && item.score.tier === tier),
  }));
  return { alarms, tiers, notJudged: ranked.filter(notJudged) };
}

/**
 * The ranked list "Needs you" (mockup `renderList`): kind toggles, P0 alarms (of every kind), tier groups and
 * "Not judged yet";
 * keys `j`/`k` move the cursor, Enter expands, `v` opens the PR's review, `g` the item, `?` the help.
 */
export function WorkList({
  items,
  kinds,
  onKindsChange,
  filters = [],
  onFiltersChange,
  statusCounts,
  statusDescriptions,
  exclusions,
  guideOrigin,
  onWake,
  wakingKeys,
  onRejudge,
  rejudgingKeys = [],
  rankingNotice,
  usageHref,
  snapshotAt,
  reviewHref,
  commentsHref,
  onWatch,
  watchingKeys = [],
  onNavigate,
  onHelp,
  keysEnabled = true,
  stale,
  todos,
  empty,
  onSnooze,
  snoozingKeys = [],
  now,
}: WorkListProps) {
  const [status, setStatus] = useState<WorkStatus>("all");
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
  const [cursor, setCursor] = useState<string | null>(null);
  const [lastRejudge, setLastRejudge] = useState<{ key: string; judgedAt: string | null }>();
  /** Focus targets of the rows for `j`/`k`: a row's expander, an alarm's frame. */
  const targets = useRef(new Map<string, HTMLElement>());
  const titleId = useId();
  const staleId = useId();

  const offered = todosPossible(items, todos) ? ALL_KINDS : ALL_KINDS.filter((kind) => kind !== "todo");
  // Kinds on among those offered; none (TODOs only, now hidden) shows every offered kind.
  const chosen = offered.filter((kind) => kinds.includes(kind));
  const active = chosen.length > 0 ? chosen : offered;
  const tags = (item: WorkItem) => workTags(item).map(tagFilter);
  const shown = items.filter(
    (item) =>
      active.includes(item.kind) &&
      matchesStatus(item, status) &&
      (filters.length === 0 || matchesFilters(item, filters, tags(item))),
  );
  const alarms = groupItems(items).alarms;
  const { tiers, notJudged } = groupItems(shown);
  const rows = [...alarms, ...tiers.flatMap((group) => group.items), ...notJudged];
  const counts = statusCounts ?? getStatusCounts(items, exclusions?.total ?? 0);
  const rejudging = items.filter(
    ({ key, judgement }) => rejudgingKeys.includes(key) || judgement?.state === "judging",
  ).length;
  const lastItem = lastRejudge && items.find(({ key }) => key === lastRejudge.key);
  const updated =
    lastItem?.judgement?.state === "current" && lastItem.judgement.judged_at !== lastRejudge?.judgedAt;
  const judgementAnnouncement = rejudging
    ? `Re-judging ${rejudging} ${rejudging === 1 ? "item" : "items"}.`
    : updated
      ? `Re-judged ${lastItem.title}.`
      : null;
  const count = (kind: ItemKind) => items.filter((item) => item.kind === kind).length;
  const filterBy =
    onFiltersChange &&
    ((value: FilterValue, excluded: boolean) => onFiltersChange(addFilter(filters, value, excluded)));

  const toggle = (key: string) =>
    setExpanded((open) => {
      const next = new Set(open);
      if (!next.delete(key)) next.add(key);
      return next;
    });

  useEffect(() => {
    if (!keysEnabled) return;
    const onKey = (event: KeyboardEvent) => {
      if (
        event.defaultPrevented ||
        isTyping(event) ||
        (event.target instanceof Element && event.target.closest('[role="menu"]'))
      )
        return;
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
        event.preventDefault();
        toggle(current.key);
      } else if (action === "review" && current.kind === "pr") {
        event.preventDefault();
        const href = reviewHref(current);
        if (onNavigate) onNavigate(href);
        else window.location.assign(href);
      } else if (action === "open") {
        const url = httpUrl(current.url);
        if (!url) return;
        event.preventDefault();
        window.open(url, "_blank", "noopener,noreferrer");
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  });

  const register = (key: string) => (element: HTMLElement | null) => {
    if (element) targets.current.set(key, element);
    else targets.current.delete(key);
  };
  const rowProps = (item: WorkItem) => ({
    commentsHref: item.kind === "pr" ? commentsHref?.(item) : undefined,
    onWatch: onWatch && ((watching: boolean) => onWatch(item, watching)),
    watched: Boolean(item.watched),
    watching: watchingKeys.includes(item.key),
    snapshotAt,
    onFilter: filterBy,
    onRejudge:
      onRejudge &&
      (() => {
        setLastRejudge({ key: item.key, judgedAt: item.judgement?.judged_at ?? null });
        onRejudge(item.key);
      }),
    rejudging: rejudgingKeys.includes(item.key),
    usageHref,
  });
  const row = (item: WorkItem) => (
    <WorkItemRow
      key={item.key}
      item={item}
      expanded={expanded.has(item.key)}
      onToggle={() => toggle(item.key)}
      current={item.key === cursor}
      reviewHref={item.kind === "pr" ? reviewHref(item) : undefined}
      {...rowProps(item)}
      onNavigate={onNavigate}
      expanderRef={register(item.key)}
      todos={todos}
      onSnooze={onSnooze && ((days) => onSnooze(item, days))}
      snoozing={snoozingKeys.includes(item.key)}
      now={now}
    />
  );
  const group = (title: string, rule: string, list: WorkItem[]) => (
    <div key={title} className="mt-1.5">
      <h3 className="flex items-baseline gap-2.5 px-0.5 pt-1 pb-1.5 text-xs font-bold tracking-wide text-muted uppercase">
        <span className="text-sm tracking-normal text-text">{title}</span>
        {rule && <span>{rule}</span>}
        <span>· {list.length}</span>
      </h3>
      <ul className="m-0 list-none rounded-lg border border-border bg-surface p-0">{list.map(row)}</ul>
    </div>
  );

  return (
    <section aria-labelledby={titleId} className="@container min-w-0">
      <div className="flex flex-wrap items-center gap-x-4 gap-y-2.5">
        <h2 id={titleId} className="text-[17px] font-semibold">
          Needs you
        </h2>
        <span role="status" className="text-xs text-muted">
          {status === "excluded"
            ? `${counts.excluded} excluded or snoozed`
            : `${rows.length} of ${items.length} items, ranked`}
          <span
            className={
              rankingNotice
                ? "fixed right-4 bottom-4 z-40 rounded-md border border-border bg-surface px-3 py-2 text-sm text-text shadow"
                : "sr-only"
            }
          >
            {rankingNotice ?? judgementAnnouncement}
          </span>
        </span>
        <span className="flex-1" />
        <ToggleGroup
          label="Kinds shown"
          values={active}
          onChange={onKindsChange}
          options={offered.map((kind) => ({
            value: kind,
            label: (
              <>
                {KIND_LABEL[kind]} <b>{count(kind)}</b>
              </>
            ),
            tip: `Show or hide ${kind === "todo" ? "your MemCan TODOs" : KIND_TEXT[kind]}`,
          }))}
        />
        <StatusFilter
          status={status}
          counts={counts}
          total={items.length}
          onChange={setStatus}
          descriptions={statusDescriptions}
          excludedDescription={exclusionDescription(exclusions)}
        />
      </div>
      {onFiltersChange && (
        <FilterChips
          filters={filters}
          labels={filters.length ? items.flatMap((item) => itemFilters(item, tags(item))) : []}
          onChange={onFiltersChange}
        />
      )}
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
                {...rowProps(item)}
                onNavigate={onNavigate}
                expanded={expanded.has(item.key)}
                onToggle={() => toggle(item.key)}
                now={now}
              />
            ))}
          </div>
        )}
        {tiers.map(
          ({ tier, items: list }) => list.length > 0 && group(tierName(tier), TIER_RULE[tier], list),
        )}
        {notJudged.length > 0 && group("Not judged yet", "", notJudged)}
        {status === "excluded" ? (
          <ExcludedItems
            exclusions={exclusions}
            guideOrigin={guideOrigin}
            onWake={onWake}
            wakingKeys={wakingKeys}
            onNavigate={onNavigate}
          />
        ) : (
          shown.length === 0 &&
          (items.length === 0 && status === "all" && filters.length === 0 && empty ? (
            empty
          ) : (
            <p className="px-0.5 py-2 text-[12.5px] text-muted">
              {status !== "all" || filters.length
                ? "No ranked items match these filters."
                : `No ${kindsText(active)} need you.`}
            </p>
          ))
        )}
      </div>
    </section>
  );
}
