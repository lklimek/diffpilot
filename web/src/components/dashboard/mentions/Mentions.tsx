import { useState } from "react";
import type { Mention, MentionCoverage } from "../../../api/generated/types";
import { Tooltip } from "../../tooltip/Tooltip";
import { dateTimeText } from "../../format/time";
import { Segmented } from "../common/Segmented";
import { OpenExternal } from "../common/OpenExternal";

interface Props {
  items: Mention[];
  coverage?: MentionCoverage;
  withBots: boolean;
  onWithBotsChange: (withBots: boolean) => void;
  expanded: boolean;
  onExpandedChange: (expanded: boolean) => void;
  onHide: (id: string) => void;
  hidingIds: string[];
  now: Date;
}

const waiting = (at: string, now: Date) => {
  const minutes = Math.max(0, Math.floor((now.getTime() - Date.parse(at)) / 60_000));
  return minutes < 60
    ? `${minutes} min`
    : minutes < 1440
      ? `${Math.floor(minutes / 60)} h`
      : `${Math.floor(minutes / 1440)} d`;
};

/** Quick replies outside the ranking; all untrusted strings render as text. */
export function Mentions({
  items,
  coverage,
  withBots,
  onWithBotsChange,
  expanded,
  onExpandedChange,
  onHide,
  hidingIds,
  now,
}: Props) {
  const [all, setAll] = useState(false);
  const notes: string[] = [];
  if (coverage?.unchecked_items) {
    const count = coverage.unchecked_items;
    notes.push(`Mentions not checked on ${count} ${count === 1 ? "item" : "items"} this refresh.`);
  }
  for (const login of coverage?.unchecked_logins ?? []) {
    notes.push(`Mentions of @${login} were not checked because GitHub search is unavailable for this login.`);
  }
  for (const login of coverage?.timed_out_logins ?? []) {
    notes.push(`PR mention search for @${login} was not completed within its request budget.`);
  }
  if (items.length === 0 && notes.length === 0) return null;
  const visible = items
    .filter((item) => withBots || !item.is_bot)
    .sort((a, b) => Date.parse(a.created_at) - Date.parse(b.created_at) || a.id.localeCompare(b.id));
  const rows = all ? visible : visible.slice(0, 3);
  return (
    <section
      aria-label="Mentions"
      className="min-w-0 rounded-lg border border-border bg-surface px-2 sm:px-4"
    >
      <header className="flex min-w-0 items-center justify-between gap-2">
        <h2 className="shrink-0 font-semibold">
          <Tooltip tip={expanded ? "Collapse the mentions list" : "Expand the mentions list"}>
            <button
              type="button"
              className="min-h-6 whitespace-nowrap"
              aria-expanded={expanded}
              onClick={() => onExpandedChange(!expanded)}
            >
              Mentions ({visible.length})
            </button>
          </Tooltip>
        </h2>
        {!expanded && items.length > 0 && visible.length === 0 && (
          <span className="truncate text-xs text-muted">Only bot mentions ({items.length})</span>
        )}
        {!expanded && notes.length > 0 && (
          <Tooltip tip={notes.join(" ")} className="min-w-0">
            <span tabIndex={0} className="block truncate text-xs text-muted">
              {notes.join(" ")}
            </span>
          </Tooltip>
        )}
      </header>
      {expanded &&
        notes.map((note) => (
          <p key={note} className="text-xs text-muted">
            {note}
          </p>
        ))}
      {expanded && items.length > 0 && (
        <>
          <div className="[&_[role=group]]:max-w-full [&_button]:px-1">
            <Segmented
              label="Mention authors"
              value={withBots ? "all" : "human"}
              onChange={(value) => onWithBotsChange(value === "all")}
              options={[
                { value: "human", label: "without bots", tip: "Show mentions from people" },
                { value: "all", label: "with bots", tip: "Show mentions from people and bots" },
              ]}
            />
          </div>
          {visible.length === 0 ? (
            <p className="text-sm text-muted">
              Only bot mentions are waiting ({items.length}). Choose with bots to show them.
            </p>
          ) : (
            <ul className="m-0 list-none p-0">
              {rows.map((item) => (
                <li
                  key={item.id}
                  className="flex min-w-0 items-center gap-2 border-t border-border py-1 first:border-t-0"
                >
                  <div className="min-w-0 flex-1 text-sm">
                    <div
                      className="truncate"
                      title={`@${item.author} · ${item.repo.owner}/${item.repo.name}#${item.number} · ${item.title}`}
                    >
                      @{item.author}
                      {item.is_bot && <span> · bot</span>} · {item.repo.owner}/{item.repo.name}#{item.number}{" "}
                      · {item.title}
                    </div>
                    <p className="truncate text-xs text-muted" title={item.text}>
                      <time dateTime={item.created_at} title={dateTimeText(item.created_at)}>
                        waiting {waiting(item.created_at, now)}
                      </time>{" "}
                      {item.partly_read && <span>· partly read </span>}· {item.text}
                    </p>
                  </div>
                  <OpenExternal
                    href={item.url}
                    label={`Open mention on ${item.repo.owner}/${item.repo.name}#${item.number} on GitHub`}
                  />
                  <Tooltip
                    tip="Hide this mention across your devices. Later mentions still appear."
                    className="shrink-0"
                  >
                    <button
                      type="button"
                      className="min-h-6 text-sm text-muted underline"
                      aria-disabled={hidingIds.includes(item.id)}
                      aria-label={`Hide mention from @${item.author} on ${item.repo.owner}/${item.repo.name}#${item.number}`}
                      onClick={() => {
                        if (!hidingIds.includes(item.id)) onHide(item.id);
                      }}
                    >
                      Hide
                    </button>
                  </Tooltip>
                </li>
              ))}
            </ul>
          )}
          {visible.length > 3 && (
            <Tooltip tip={all ? "Show the three oldest mentions" : "Show all waiting mentions"}>
              <button
                type="button"
                className="min-h-6 text-sm text-accent underline"
                aria-expanded={all}
                onClick={() => setAll(!all)}
              >
                {all ? "Show less" : `+${visible.length - 3} more`}
              </button>
            </Tooltip>
          )}
        </>
      )}
    </section>
  );
}
