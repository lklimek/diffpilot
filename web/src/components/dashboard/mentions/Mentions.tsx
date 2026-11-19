import { useState } from "react";
import type { Mention } from "../../../api/generated/types";
import { Tooltip } from "../../tooltip/Tooltip";
import { dateTimeText } from "../../format/time";

interface Props {
  items: Mention[];
  withBots: boolean;
  onWithBotsChange: (withBots: boolean) => void;
  onHide: (id: string) => void;
  hiding: boolean;
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
export function Mentions({ items, withBots, onWithBotsChange, onHide, hiding, now }: Props) {
  const [expanded, setExpanded] = useState(false);
  if (items.length === 0) return null;
  const visible = items
    .filter((item) => withBots || !item.is_bot)
    .sort((a, b) => Date.parse(a.created_at) - Date.parse(b.created_at) || a.id.localeCompare(b.id));
  const rows = expanded ? visible : visible.slice(0, 3);
  return (
    <section
      aria-label="Mentions"
      className="min-w-0 rounded-lg border border-border bg-surface px-2 py-3 [overflow-wrap:anywhere] sm:px-4"
    >
      <header className="flex flex-wrap items-center justify-between gap-2">
        <h2 className="font-semibold">
          Mentions <span className="text-muted">({visible.length})</span>
        </h2>
        <label className="flex min-h-6 flex-wrap items-center gap-2 text-sm">
          <input
            type="checkbox"
            aria-label="with bots"
            checked={withBots}
            onChange={(event) => onWithBotsChange(event.target.checked)}
          />
          {withBots ? "with bots" : "without bots"}
        </label>
      </header>
      <ul className="m-0 list-none p-0">
        {rows.map((item) => (
          <li
            key={item.id}
            className="flex flex-wrap items-start gap-x-3 gap-y-1 border-t border-border py-2 first:border-t-0"
          >
            <div className="min-w-0 flex-[1_1_14rem]">
              <div className="flex flex-wrap gap-x-2 text-xs text-muted [overflow-wrap:anywhere]">
                <span>@{item.author}</span>
                {item.is_bot && <span>bot</span>}
                <time dateTime={item.created_at} title={dateTimeText(item.created_at)}>
                  waiting {waiting(item.created_at, now)}
                </time>
              </div>
              <a
                className="block min-h-6 text-accent underline [overflow-wrap:anywhere]"
                href={item.url}
                target="_blank"
                rel="noreferrer"
              >
                {item.repo.owner}/{item.repo.name}#{item.number} · {item.title}
              </a>
              <p className="truncate text-sm text-muted" title={item.text}>
                {item.text}
              </p>
            </div>
            <Tooltip
              tip="Hide this mention across your devices. Later mentions still appear."
              className="shrink-0"
            >
              <button
                type="button"
                className="min-h-6 shrink-0 text-sm text-muted underline"
                aria-disabled={hiding}
                aria-label={`Hide mention from @${item.author} on ${item.repo.owner}/${item.repo.name}#${item.number}`}
                onClick={() => {
                  if (!hiding) onHide(item.id);
                }}
              >
                Hide
              </button>
            </Tooltip>
          </li>
        ))}
      </ul>
      {visible.length > 3 && (
        <Tooltip tip={expanded ? "Show the three oldest mentions" : "Show all waiting mentions"}>
          <button
            type="button"
            className="min-h-6 text-sm text-accent underline"
            aria-expanded={expanded}
            onClick={() => setExpanded(!expanded)}
          >
            {expanded ? "Show less" : `+${visible.length - 3} more`}
          </button>
        </Tooltip>
      )}
    </section>
  );
}
