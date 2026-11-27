import type { AwaitingItem, PrRef, SlackName } from "../../../api/generated/types";
import { CopyLineButton } from "../common/CopyLineButton";
import { CopyListButton } from "../common/CopyListButton";
import { atLogin, slackName } from "../common/copyList";
import { daysSince } from "../common/format";
import { OpenExternal, openLabel } from "../common/OpenExternal";
import { FactTags, SizePill } from "../common/Tags";
import { AsideSection } from "../side-panel/AsideSection";
import { WorkTitleLink } from "../common/WorkTitleLink";
import type { Navigate } from "../common/navigate";

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
 * those a plug-in says owe a review), why, and for how long. `slackNames`: the user's Slack names (Settings); with
 * them each row and the list also copy for Slack (K19), without them (not read) only the plain copy.
 */
export function WaitingOnOthers({
  items,
  now,
  slackNames,
  prHref,
  onNavigate,
}: {
  items: AwaitingItem[];
  now: Date;
  slackNames?: SlackName[];
  prHref?: (pr: PrRef) => string | undefined;
  onNavigate?: Navigate;
}) {
  const sorted = [...items].sort((a, b) => a.waiting_since.localeCompare(b.waiting_since));
  const oldest = sorted[0] ? daysSince(sorted[0].waiting_since, now) : null;
  const waits = (item: AwaitingItem, name: (login: string) => string = atLogin) =>
    item.waiting_on.length === 0 ? "waits for" : `waits on ${item.waiting_on.map(name).join(", ")} for`;
  const line = (item: AwaitingItem, name?: (login: string) => string) => ({
    ...item,
    suffix: `${waits(item, name)} ${daysSince(item.waiting_since, now)} d`,
  });
  const slackLine =
    slackNames && ((item: AwaitingItem) => line(item, (login) => slackName(login, slackNames)));
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
                {slackLine && <CopyLineButton line={slackLine(item)} id={item.id} slack />}
              </div>
              <WorkTitleLink
                className="block min-h-6 [overflow-wrap:anywhere]"
                href={prHref?.(item)}
                onNavigate={onNavigate}
              >
                {item.title}
              </WorkTitleLink>
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
      <CopyListButton
        lines={sorted.map((item) => line(item))}
        slackLines={slackLine && sorted.map(slackLine)}
        what="PRs waiting on others"
      />
      <p className="mt-1.5 text-xs text-muted">
        Your PRs where nothing is your move and other people have to act: the ready label (it needs a human,
        even when you could merge), a plug-in saying reviewers or maintainers have the move, or a review
        requested from others that is not approved. Longest wait first; ping the people named.
      </p>
    </AsideSection>
  );
}
