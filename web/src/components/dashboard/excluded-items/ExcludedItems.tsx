import type { ExcludedItem, ExclusionCount, ExclusionReason, Exclusions } from "../../../api/generated/types";
import { navigateOnClick, type Navigate } from "../common/navigate";
import { Tooltip } from "../../tooltip/Tooltip";
import { helpHref, type HelpTopic } from "../../settings/settingsHelp";
import { OpenExternal, openLabel } from "../common/OpenExternal";
import { buttonClass, busyTip } from "../../button/button";
import { dateTimeText } from "../../format/time";

interface ExcludedItemsProps {
  /** The snapshot's exclusions; absent when the snapshot did not record them. */
  exclusions: Exclusions | null | undefined;
  guideOrigin?: string;
  onNavigate?: Navigate;
  /** "Wake now" of a snoozed item (its key); without it no button. */
  onWake?: (key: string) => void;
  /** Keys of the snoozed items being woken. */
  wakingKeys?: string[];
}

interface ReasonText {
  /** Count phrase of the summary line. */
  count: (n: number) => string;
  title: string;
  /** The rule as a fact. */
  rule: string;
  /** The setting that holds the rule, if one: its settings-guide entry and name. */
  setting?: { topic: HelpTopic; name: string };
}

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

const REASONS: Record<ExclusionReason, ReasonText> = {
  excluded_author: {
    count: (n) => plural(n, "excluded author", "excluded authors"),
    title: "Excluded authors",
    rule: "Pull requests and issues written by an excluded author.",
    setting: { topic: "skip_authors", name: "Excluded authors" },
  },
  outside_owned_areas: {
    count: (n) => `${n} outside your areas`,
    title: "Outside your areas",
    rule: "Pull requests of others not directed at you, with no or only a minor change in your owned paths.",
    setting: { topic: "ownership_policy", name: "Ownership policy" },
  },
  draft: {
    count: (n) => plural(n, "draft", "drafts"),
    title: "Drafts",
    rule: "Draft pull requests of other authors; only your own drafts count.",
  },
  judged_not_eligible: {
    count: (n) => `${n} judged outside your areas by the LLM`,
    title: "Judged outside your areas (LLM)",
    rule: "The ownership policy file could not be read; the LLM judged these outside your areas.",
  },
  merged_into_another: {
    count: (n) => `${plural(n, "merged into another item", "merged into other items")} by the LLM`,
    title: "Merged into another item",
    rule: "The LLM merged each into another item covering the same work.",
  },
  snoozed: {
    count: (n) => `${n} snoozed`,
    title: "Snoozed",
    rule: "Items you snoozed. Each comes back at its date, or earlier when you get a new review request or mention on it.",
  },
};

const OTHER: ReasonText = {
  count: (n) => `${n} by another rule`,
  title: "Another rule",
  rule: "Left out by a rule this page does not know yet.",
};

// The server may add reasons; an unknown one reads as "another rule".
const textOf = (reason: string): ReasonText =>
  Object.hasOwn(REASONS, reason) ? REASONS[reason as ExclusionReason] : OTHER;

/** Largest count first; equal counts keep the server's order. */
const byCount = (counts: ExclusionCount[]) => [...counts].sort((a, b) => b.count - a.count);

export function exclusionDescription(exclusions: Exclusions | null | undefined): string {
  if (!exclusions) return "Exclusions were not recorded in this snapshot.";
  return byCount(exclusions.counts)
    .map(({ reason, count }) => textOf(reason).count(count))
    .join(" · ");
}

/** The excluded snapshot items by reason, with links to evidence and Wake now for snoozes. */
export function ExcludedItems({
  exclusions,
  guideOrigin,
  onNavigate,
  onWake,
  wakingKeys = [],
}: ExcludedItemsProps) {
  if (!exclusions?.total)
    return (
      <p className="py-2 text-xs text-muted">
        {exclusions ? "No items are excluded or snoozed." : exclusionDescription(exclusions)}
      </p>
    );
  const items = exclusions.items ?? [];
  return (
    <div className="mt-2 grid gap-3 rounded-lg border border-border bg-surface p-3">
      {byCount(exclusions.counts).map(({ reason, count }) => (
        <Group
          key={reason}
          text={textOf(reason)}
          count={count}
          items={items.filter((item) => item.reason === reason)}
          guideOrigin={guideOrigin}
          onNavigate={onNavigate}
          onWake={onWake}
          wakingKeys={wakingKeys}
        />
      ))}
      {items.length < exclusions.total && (
        <p className="text-xs text-muted">
          Listed: the first {items.length} of {exclusions.total}.
        </p>
      )}
    </div>
  );
}

function Group({
  text,
  count,
  items,
  guideOrigin,
  onNavigate,
  onWake,
  wakingKeys,
}: {
  text: ReasonText;
  count: number;
  items: ExcludedItem[];
  guideOrigin?: string;
  onNavigate?: Navigate;
  onWake?: (key: string) => void;
  wakingKeys: string[];
}) {
  const label = `${text.title}: ${count}`;
  const guideHref = text.setting && helpHref(text.setting.topic, guideOrigin);
  return (
    <div role="group" aria-label={label} className="text-xs">
      <p className="font-semibold text-text">{label}</p>
      <p className="text-muted">
        {text.rule}
        {text.setting && guideHref && (
          <>
            {" "}
            <a
              href={guideHref}
              onClick={navigateOnClick(guideHref, onNavigate)}
              className="text-accent underline"
            >
              {text.setting.name} in the settings guide
            </a>
          </>
        )}
      </p>
      {items.length > 0 && (
        <ul className="m-0 mt-1 flex list-none flex-col gap-1 p-0">
          {items.map((item) => (
            <li key={item.key} className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-0.5">
              <OpenExternal href={item.url} label={openLabel(item)} />
              <span className="shrink-0 font-mono">{item.id}</span>
              <span className="min-w-0 truncate">{item.title}</span>
              {item.author && <span className="shrink-0 text-muted">by @{item.author}</span>}
              {item.merged_into && (
                <span className="shrink-0 text-muted">merged into {item.merged_into}</span>
              )}
              {item.reason === "snoozed" && item.snoozed_until && (
                <span className="shrink-0 text-muted">Snoozed until {dateTimeText(item.snoozed_until)}</span>
              )}
              {item.reason === "snoozed" && onWake && (
                <WakeButton
                  item={item}
                  waking={wakingKeys.includes(item.key)}
                  onWake={() => onWake(item.key)}
                />
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

function WakeButton({ item, waking, onWake }: { item: ExcludedItem; waking: boolean; onWake: () => void }) {
  return (
    <Tooltip tip={busyTip("Ends the snooze: the item is back in your list now.", waking)}>
      <button
        type="button"
        aria-label={`Wake now ${item.id}`}
        aria-disabled={waking || undefined}
        onClick={() => {
          if (!waking) onWake();
        }}
        className={`${buttonClass("plain", true)} shrink-0`}
      >
        Wake now
      </button>
    </Tooltip>
  );
}
