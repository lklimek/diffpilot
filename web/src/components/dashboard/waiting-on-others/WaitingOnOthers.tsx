import type { AwaitingItem } from "../../../api/generated/types";
import { Tooltip } from "../../tooltip/Tooltip";
import { daysSince } from "../common/format";
import { OpenExternal, openLabel } from "../common/OpenExternal";
import { FactTags } from "../common/Tags";
import { AsideSection } from "../side-panel/AsideSection";

const who = (login: string) => (login.startsWith("team:") ? login : `@${login}`);

/** The user's ready PRs waiting on other people's review, longest wait first (side panel). */
export function WaitingOnOthers({ items, now }: { items: AwaitingItem[]; now: Date }) {
  const sorted = [...items].sort((a, b) => a.waiting_since.localeCompare(b.waiting_since));
  const oldest = sorted[0] ? daysSince(sorted[0].waiting_since, now) : null;
  return (
    <AsideSection
      title="Waiting on others"
      defaultOpen
      summary={
        <>
          <b>{items.length}</b> PRs
          {oldest !== null && (
            <>
              {" "}
              · oldest <b>{oldest} d</b>
            </>
          )}
        </>
      }
    >
      {sorted.length === 0 ? (
        <p className="text-xs text-muted">None of your PRs waits on a requested review.</p>
      ) : (
        <ul className="m-0 list-none p-0">
          {sorted.map((item) => (
            <li key={item.url} className="grid gap-0.5 border-t border-border py-1.5 first:border-t-0">
              <div className="flex items-baseline gap-2">
                <span className="font-mono text-[12.5px] font-semibold">{item.id}</span>
                <OpenExternal href={item.url} label={openLabel(item)} />
                <span className="flex-1" />
                {item.total_tenths === null ? (
                  <Tooltip tip="Not ranked: it waits on others, nothing is your move">
                    <span tabIndex={0} className="cursor-help text-xs text-muted">
                      —
                    </span>
                  </Tooltip>
                ) : (
                  <Tooltip tip="Score of your PR in the full ranking">
                    <span
                      tabIndex={0}
                      className="cursor-help rounded border border-border px-1.5 font-mono text-[11px] leading-[18px] font-semibold"
                    >
                      {Math.round(item.total_tenths / 10)}
                    </span>
                  </Tooltip>
                )}
              </div>
              <div className="[overflow-wrap:anywhere]">{item.title}</div>
              <div className="flex flex-wrap items-center gap-1.5 text-xs text-muted">
                <span>
                  waits on {item.waiting_on.map(who).join(", ")} for{" "}
                  <b className="text-text">{daysSince(item.waiting_since, now)} d</b>
                </span>
                <FactTags tags={item.tags} />
              </div>
            </li>
          ))}
        </ul>
      )}
      <p className="mt-1.5 text-xs text-muted">
        Your ready PRs with a pending review request to someone else; longest wait first. A long wait is a
        hint to nudge on GitHub yourself.
      </p>
    </AsideSection>
  );
}
