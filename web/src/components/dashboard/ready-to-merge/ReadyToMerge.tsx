import type { ReadyToMergeItem } from "../../../api/generated/types";
import { CopyListButton } from "../common/CopyListButton";
import { OpenExternal, openLabel } from "../common/OpenExternal";
import { FactTags, SizePill } from "../common/Tags";
import { AsideSection } from "../side-panel/AsideSection";

/** The user's PRs GitHub lets them merge now, in the server's order (by id) (side panel). */
export function ReadyToMerge({ items }: { items: ReadyToMergeItem[] }) {
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
        <p className="text-xs text-muted">None of your PRs can be merged on GitHub right now.</p>
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
      <CopyListButton lines={items} what="PRs ready to merge" />
      <p className="mt-1.5 text-xs text-muted">
        Your PRs whose merge button on GitHub is active: not a draft, you can write to the repository, and
        GitHub reports the merge as clean or only checks that are not required as not passing. By id; they
        stay in the ranking too.
      </p>
    </AsideSection>
  );
}
