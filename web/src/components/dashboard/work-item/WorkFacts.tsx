import type { DashboardItem } from "../../../api/provisional/dashboard";
import type { DashboardFactTag } from "../../../api/provisional/dashboardFactFilter";
import { clockText } from "../../format/time";
import { isGithubUrl } from "../../github/links";
import { Tooltip } from "../../tooltip/Tooltip";
import { OpenExternal, rowOpenLabel } from "../common/OpenExternal";
import { FactTags } from "../common/Tags";
import { FilterButton, type OnFilter } from "../filter-chips/FilterChips";
import { authorFilter, repoFilter } from "../filter-chips/filters";

export function workTags(item: DashboardItem): DashboardFactTag[] {
  const tags = [...item.tags];
  if (item.private) tags.unshift({ text: "private", tip: "From a private repository; shown only to you." });
  if (item.stack?.position === "bottom") {
    const { size, dependents } = item.stack;
    const base = item.facts.find(({ label }) => label === "Base branch")?.value;
    const numbers = dependents.map((number) => `#${number}`);
    const rest =
      numbers.length === 1 ? numbers[0] : `${numbers.slice(0, -1).join(", ")} and ${numbers.at(-1)}`;
    tags.push({
      text: "⤓ merges first",
      tip: `Bottom of a stack of ${size}${base ? `: base is ${base}` : ""}${numbers.length ? `; ${rest} ${numbers.length === 1 ? "is" : "are"} based on it` : ""}`,
      filter: { key: "stack_bottom", label: "merges first" },
    });
  }
  if (item.release_base)
    tags.push({
      text: `→ ${item.release_base}`,
      tip: `Base ${item.release_base} matches this repository’s configured release branches`,
      filter: { key: "release_base", label: "release base" },
    });
  return tags;
}

export function WorkTitle({ item }: { item: DashboardItem }) {
  return (
    <>
      {item.title}
      {item.url && isGithubUrl(item.url) && (
        <>
          {" "}
          <OpenExternal href={item.url} label={rowOpenLabel(item)} duplicate />
        </>
      )}
    </>
  );
}

export function WorkFacts({
  item,
  onFilter,
  snapshotAt,
  now,
}: {
  item: DashboardItem;
  onFilter?: OnFilter;
  snapshotAt?: string | null;
  now: Date;
}) {
  const repo = repoFilter(item);
  const author = item.author ? authorFilter(item.author) : null;
  return (
    <>
      {repo ? (
        <span className="min-w-0 [overflow-wrap:anywhere]">
          {onFilter ? (
            <FilterButton value={repo} onFilter={onFilter}>
              {repo.label}
            </FilterButton>
          ) : (
            repo.label
          )}
          {item.number !== null && `#${item.number}`}
        </span>
      ) : (
        <span>MemCan</span>
      )}
      {repo && item.live === false && (
        <Tooltip
          tip={`Updated only by the nightly scan and Refresh${snapshotAt ? `; facts as of ${clockText(snapshotAt, now)}` : ""}.`}
        >
          <span tabIndex={0} className="cursor-help rounded border border-border px-1.5 font-semibold">
            ◷ not live
          </span>
        </Tooltip>
      )}
      {author && (
        <>
          <span aria-hidden>·</span>
          {onFilter ? (
            <FilterButton value={author} onFilter={onFilter} className="max-w-full [overflow-wrap:anywhere]">
              {author.label}
            </FilterButton>
          ) : (
            author.label
          )}
        </>
      )}
      <FactTags tags={workTags(item)} onFilter={onFilter} />
    </>
  );
}
