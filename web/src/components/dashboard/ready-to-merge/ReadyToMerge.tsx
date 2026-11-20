import type { ReadyToMergeItem } from "../../../api/generated/types";
import { CopyListButton } from "../common/CopyListButton";
import { OpenExternal, openLabel } from "../common/OpenExternal";
import { FactTags, SizePill } from "../common/Tags";
import { AsideSection } from "../side-panel/AsideSection";

const prs = (n: number) => `${n} ${n === 1 ? "PR" : "PRs"}`;

/**
 * The user's PRs GitHub let them merge at the last scan, in the server's order (by id) (side panel). `pending`:
 * how many merge states GitHub had not computed yet; null when the snapshot did not record merge states.
 */
export function ReadyToMerge({ items, pending }: { items: ReadyToMergeItem[]; pending: number | null }) {
  if (pending === null) {
    return (
      <AsideSection title="Ready to merge" defaultOpen summary="not recorded">
        <p className="text-xs text-muted">Not recorded for this snapshot — refresh to see it.</p>
      </AsideSection>
    );
  }
  return (
    <AsideSection
      title="Ready to merge"
      defaultOpen
      summary={
        <>
          <b>{items.length}</b> PRs
        </>
      }
    >
      {items.length === 0 ? (
        <p className="text-xs text-muted">None of your PRs was ready to merge at the last scan.</p>
      ) : (
        <ul className="m-0 list-none p-0">
          {items.map((item) => (
            <li key={item.url} className="grid gap-0.5 border-t border-border py-1.5 first:border-t-0">
              <div className="flex items-baseline gap-2">
                <span className="font-mono text-[12.5px] font-semibold">{item.id}</span>
                <OpenExternal href={item.url} label={openLabel(item)} />
              </div>
              <div className="[overflow-wrap:anywhere]">{item.title}</div>
              <div className="flex flex-wrap items-center gap-1.5 text-xs text-muted">
                <SizePill mark={item.change_size} />
                <FactTags tags={item.tags} />
              </div>
            </li>
          ))}
        </ul>
      )}
      {pending > 0 && (
        <p className="mt-1.5 text-xs text-muted">
          {prs(pending)}: GitHub had not computed the merge state yet; the next refresh checks again.
        </p>
      )}
      <CopyListButton lines={items} what="PRs ready to merge" />
      <p className="mt-1.5 text-xs text-muted">
        Your PRs into the repository's default branch that were ready to merge at the last scan: not a draft,
        you can write to the repository, every check passed, and no unresolved thread, change request or
        comment awaiting your reply. By id; they stay in the ranking too.
      </p>
    </AsideSection>
  );
}
