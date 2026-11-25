import type { ChangeSizeMark, FactTag } from "../../../api/generated/types";
import { FilterButton, type OnFilter } from "../filter-chips/FilterChips";
import { sizeFilter, tagFilter } from "../filter-chips/filters";
import { Tooltip } from "../../tooltip/Tooltip";
import { changedLines, SIZE_RULE } from "./size";

const PILL =
  "inline-block cursor-help rounded border px-1.5 text-[11px] leading-[18px] font-semibold whitespace-nowrap";

/** Deterministic tags from scan facts: one neutral look (no tone on the wire), the fact in the tooltip. */
export function FactTags({ tags, onFilter }: { tags: FactTag[]; onFilter?: OnFilter }) {
  const look = `${PILL} border-mv-bd bg-surface text-text`;
  return (
    <>
      {tags.map((tag, i) =>
        onFilter ? (
          <FilterButton
            key={`${tag.text}-${i}`}
            value={tagFilter(tag)}
            tip={tag.tip}
            onFilter={onFilter}
            data-fact-tag
            className={look}
          >
            {tag.text}
          </FilterButton>
        ) : (
          <Tooltip key={`${tag.text}-${i}`} tip={tag.tip}>
            <span data-fact-tag tabIndex={0} className={look}>
              {tag.text}
            </span>
          </Tooltip>
        ),
      )}
    </>
  );
}

/** A pull request's size mark as a neutral pill (a fact with its evidence); nothing without a mark. */
export function SizePill({
  mark,
  onFilter,
}: {
  mark: ChangeSizeMark | null | undefined;
  onFilter?: OnFilter;
}) {
  if (!mark) return null;
  const lines = changedLines(mark);
  const tip = `${lines} changed lines (+${mark.additions} −${mark.deletions}); ${SIZE_RULE}`;
  if (onFilter)
    return (
      <FilterButton
        value={sizeFilter(mark.size)}
        tip={tip}
        onFilter={onFilter}
        data-size-pill={mark.size}
        className="rounded-full border border-mv-bd bg-surface px-2 text-[11px] font-semibold text-text"
      >
        {mark.size}
      </FilterButton>
    );
  return (
    <Tooltip tip={tip}>
      <span
        data-size-pill={mark.size}
        role="img"
        aria-label={`Change size: ${mark.size}, ${lines} changed lines`}
        tabIndex={0}
        className="inline-block cursor-help rounded-full border border-mv-bd bg-surface px-2 text-[11px] leading-[18px] font-semibold whitespace-nowrap text-text"
      >
        {mark.size}
      </span>
    </Tooltip>
  );
}
