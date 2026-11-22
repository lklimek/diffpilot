import type { AwaitingItem } from "../../../api/generated/types";
import { CopyLineButton } from "../common/CopyLineButton";
import { CopyListButton } from "../common/CopyListButton";
import { daysSince } from "../common/format";
import { OpenExternal, openLabel } from "../common/OpenExternal";
import { FactTags, SizePill } from "../common/Tags";
import { AsideSection } from "../side-panel/AsideSection";

const who = (login: string) => (login.startsWith("team:") ? login : `@${login}`);

/** Why the PR waits, in a few words; a plug-in's reason is attributed and links to its evidence. */
function Reason({ item }: { item: AwaitingItem }) {
  const reason = item.reason;
  switch (reason.kind) {
    case "ready_label":
      return <span>{reason.label}</span>;
    case "review_requested":
      return <span>review requested</span>;
    case "plugin":
      return (
        <span className="[overflow-wrap:anywhere]">
          <span className="font-semibold">{reason.source}:</span> {reason.reason}{" "}
          <OpenExternal href={reason.url} label={`${reason.source} data for ${item.id}`} />
        </span>
      );
    default:
      return null;
  }
}

/**
 * The user's PRs waiting on other people, longest wait first (side panel): whom to ping (requested reviewers, or
 * those a plug-in says owe a review), why, and for how long.
 */
export function WaitingOnOthers({ items, now }: { items: AwaitingItem[]; now: Date }) {
  const sorted = [...items].sort((a, b) => a.waiting_since.localeCompare(b.waiting_since));
  const oldest = sorted[0] ? daysSince(sorted[0].waiting_since, now) : null;
  const waits = (item: AwaitingItem) =>
    item.waiting_on.length === 0 ? "waits for" : `waits on ${item.waiting_on.map(who).join(", ")} for`;
  const line = (item: AwaitingItem) => ({
    ...item,
    suffix: `${waits(item)} ${daysSince(item.waiting_since, now)} d`,
  });
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
        <p className="text-xs text-muted">None of your PRs waits on other people.</p>
      ) : (
        <ul className="m-0 list-none p-0">
          {sorted.map((item) => (
            <li key={item.url} className="grid gap-0.5 border-t border-border py-1.5 first:border-t-0">
              <div className="flex items-baseline gap-2">
                <span className="font-mono text-[12.5px] font-semibold">{item.id}</span>
                <OpenExternal href={item.url} label={openLabel(item)} />
                <CopyLineButton line={line(item)} id={item.id} />
              </div>
              <div className="[overflow-wrap:anywhere]">{item.title}</div>
              <div className="flex flex-wrap items-center gap-1.5 text-xs text-muted">
                <span>
                  {waits(item)} <b className="text-text">{daysSince(item.waiting_since, now)} d</b>
                </span>
                <SizePill mark={item.change_size} />
                <FactTags tags={item.tags} />
              </div>
              <div className="text-xs text-muted" data-reason={item.reason.kind}>
                <Reason item={item} />
              </div>
            </li>
          ))}
        </ul>
      )}
      <CopyListButton lines={sorted.map(line)} what="PRs waiting on others" />
      <p className="mt-1.5 text-xs text-muted">
        Your PRs where nothing is your move and other people have to act: the ready label (it needs a human,
        even when you could merge), a plug-in saying reviewers or maintainers have the move, or a review
        requested from others that is not approved. Longest wait first; ping the people named.
      </p>
    </AsideSection>
  );
}
