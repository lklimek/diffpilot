import type { PluginStatus, PrRef, ReadyToMergeItem } from "../../../api/generated/types";
import { CopyListButton } from "../common/CopyListButton";
import { OpenExternal, openLabel } from "../common/OpenExternal";
import { FactTags, SizePill } from "../common/Tags";
import { PluginStatusNote } from "../plugin-status/PluginStatusNote";
import { AsideSection } from "../side-panel/AsideSection";
import { WorkTitleLink } from "../common/WorkTitleLink";
import type { Navigate } from "../common/navigate";

const prs = (n: number) => `${n} ${n === 1 ? "PR" : "PRs"}`;

/**
 * The user's PRs ready to merge at the last scan, in the server's order (by id) (side panel). `pending`: how many
 * merge states GitHub had not computed yet; null when the snapshot did not record merge states. A PR a dashboard
 * plug-in holds back (ADR-022) is not listed; it stays in the ranking with the plug-in's note. `plugins`: the
 * configured plug-ins; one that can hold PRs back and whose statements are not applied now is named.
 */
export function ReadyToMerge({
  items,
  pending,
  plugins,
  prHref,
  onNavigate,
}: {
  items: ReadyToMergeItem[];
  pending: number | null;
  plugins?: PluginStatus[];
  prHref?: (pr: PrRef) => string;
  onNavigate?: Navigate;
}) {
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
              <WorkTitleLink
                className="block min-h-6 [overflow-wrap:anywhere]"
                href={prHref?.(item)}
                onNavigate={onNavigate}
              >
                {item.title}
              </WorkTitleLink>
              <div className="flex flex-wrap items-center gap-1.5 text-xs text-muted">
                <SizePill mark={item.change_size} />
                <FactTags tags={item.tags} />
              </div>
            </li>
          ))}
        </ul>
      )}
      <PluginStatusNote
        plugins={plugins?.filter((plugin) => plugin.holds_ready_to_merge)}
        what="Ready to merge is shown"
      />
      {pending > 0 && (
        <p className="mt-1.5 text-xs text-muted">
          {prs(pending)}: GitHub had not computed the merge state yet; the next refresh checks again.
        </p>
      )}
      <CopyListButton lines={items} what="PRs ready to merge" />
      <p className="mt-1.5 text-xs text-muted">
        Your PRs into the repository's default branch or a release branch that were ready to merge at the last
        scan: not a draft, you can write to the repository, no conflicts, every check passed except ignored
        ones, no unresolved thread, change request or comment awaiting your reply, and no ready label (that
        one needs a human: Waiting on others). A PR another source holds back stays in the ranking with its
        note. By id; they stay in the ranking too.
      </p>
    </AsideSection>
  );
}
