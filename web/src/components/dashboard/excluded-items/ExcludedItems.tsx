import { useId, useRef, useState } from "react";
import type { ExcludedItem, ExclusionCount, ExclusionReason, Exclusions } from "../../../api/generated/types";
import { navigateOnClick, type Navigate } from "../common/navigate";
import { HideButton } from "../../button/HideButton";
import { Tooltip } from "../../tooltip/Tooltip";
import { helpHref, type HelpTopic } from "../../settings/settingsHelp";
import { OpenExternal, openLabel } from "../common/OpenExternal";
import { buttonClass, busyTip } from "../../button/button";
import { dateTimeText } from "../../format/time";

interface ExcludedItemsProps {
  /** The snapshot's exclusions; absent when the snapshot did not record them. */
  exclusions: Exclusions | null | undefined;
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

/**
 * What the ranking rules left out of Needs you (B4): one quiet line with the counts per reason, and a disclosure
 * listing the items by reason with links out. No LLM text; titles are plain text. Two reasons
 * (judged not eligible, merged into another item) are LLM decisions and say so. Snoozed items show their end and
 * "Wake now" (owner SN1).
 */
export function ExcludedItems({ exclusions, onNavigate, onWake, wakingKeys = [] }: ExcludedItemsProps) {
  const [open, setOpen] = useState(false);
  const toggle = useRef<HTMLButtonElement>(null);
  const panelId = useId();
  const quiet = "text-xs text-muted";
  if (!exclusions) {
    return <p className={quiet}>Excluded: not recorded for this snapshot — refresh to see it.</p>;
  }
  if (exclusions.total === 0) return <p className={quiet}>Excluded: none</p>;
  const counts = byCount(exclusions.counts);
  const line = `Excluded: ${exclusions.total} — ${counts.map((c) => textOf(c.reason).count(c.count)).join(" · ")}`;
  const items = exclusions.items ?? [];
  return (
    <div className="mt-1">
      <Tooltip
        tip={open ? "Hide the excluded items" : "List the excluded items by reason, with links to GitHub"}
      >
        <button
          ref={toggle}
          type="button"
          aria-expanded={open}
          aria-controls={panelId}
          onClick={() => setOpen((was) => !was)}
          className={`${quiet} inline-flex min-h-6 items-center gap-1 rounded text-left hover:text-text focus-visible:outline-2 focus-visible:outline-accent`}
        >
          <span aria-hidden="true">{open ? "▾" : "▸"}</span>
          <span>{line}</span>
        </button>
      </Tooltip>
      {open && (
        <div
          id={panelId}
          className="mt-1.5 flex items-start gap-2 rounded-lg border border-border bg-surface p-3"
        >
          <div className="flex min-w-0 flex-1 flex-col gap-3">
            {counts.map(({ reason, count }) => (
              <Group
                key={reason}
                text={textOf(reason)}
                count={count}
                items={items.filter((item) => item.reason === reason)}
                onNavigate={onNavigate}
                onWake={onWake}
                wakingKeys={wakingKeys}
              />
            ))}
            {items.length < exclusions.total && (
              <p className={quiet}>
                Listed: the first {items.length} of {exclusions.total}.
              </p>
            )}
          </div>
          <HideButton
            name="Hide excluded items"
            tip="Hides the list of excluded items; the line above shows it again."
            onHide={() => setOpen(false)}
            focusAfter={() => toggle.current}
          />
        </div>
      )}
    </div>
  );
}

function Group({
  text,
  count,
  items,
  onNavigate,
  onWake,
  wakingKeys,
}: {
  text: ReasonText;
  count: number;
  items: ExcludedItem[];
  onNavigate?: Navigate;
  onWake?: (key: string) => void;
  wakingKeys: string[];
}) {
  const label = `${text.title}: ${count}`;
  return (
    <div role="group" aria-label={label} className="text-xs">
      <p className="font-semibold text-text">{label}</p>
      <p className="text-muted">
        {text.rule}
        {text.setting && (
          <>
            {" "}
            <a
              href={helpHref(text.setting.topic)}
              onClick={navigateOnClick(helpHref(text.setting.topic), onNavigate)}
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
            <li key={item.key} className="flex min-w-0 items-center gap-2">
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
