import type { FactTag } from "../../../api/generated/types";
import { Tooltip } from "../../tooltip/Tooltip";

const PILL =
  "inline-block cursor-help rounded border px-1.5 text-[11px] leading-[18px] font-semibold whitespace-nowrap";

/** Deterministic tags from scan facts: one neutral look (no tone on the wire), the fact in the tooltip. */
export function FactTags({ tags }: { tags: FactTag[] }) {
  return (
    <>
      {tags.map((tag, i) => (
        <Tooltip key={`${tag.text}-${i}`} tip={tag.tip}>
          <span data-fact-tag tabIndex={0} className={`${PILL} border-mv-bd bg-surface text-text`}>
            {tag.text}
          </span>
        </Tooltip>
      ))}
    </>
  );
}

/** Dashed amber pill of a value that is not final yet (`~ provisional`, `~ not judged yet`, `~ 17 Nov`). */
export function ProvisionalPill({ text, tip }: { text: string; tip: string }) {
  return (
    <Tooltip tip={tip}>
      <span
        data-provisional
        tabIndex={0}
        className={`${PILL} border-dashed border-med-bd bg-transparent text-med`}
      >
        ~ {text}
      </span>
    </Tooltip>
  );
}
