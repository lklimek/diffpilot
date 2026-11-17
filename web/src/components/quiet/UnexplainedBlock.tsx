import type { ReactNode } from "react";
import type { FileRanges } from "../../api/generated/types";
import { rangeText } from "../section/format";
import { fileCount, lineCount, plural } from "./labels";
import { QuietCard } from "./QuietCard";

interface UnexplainedBlockProps {
  /** Changed lines no flow explains (`QuietRows.unexplained`). */
  items: FileRanges[];
  /** Code of an item (code view or its loading / error state); text only without it. */
  code?: (item: FileRanges) => ReactNode;
}

function rangesText(item: FileRanges): string {
  const count = lineCount(item.ranges);
  const ranges = item.ranges.map(rangeText).join(", ");
  if (item.side === "base") return `removed ${count === 1 ? "line" : "lines"} ${ranges} (base commit)`;
  return `${count === 1 ? "line" : "lines"} ${ranges}`;
}

/** "Unexplained — read these yourself": changed lines the analysis did not assign to any flow. */
export function UnexplainedBlock({ items, code }: UnexplainedBlockProps) {
  const lines = items.reduce((n, item) => n + lineCount(item.ranges), 0);
  return (
    <QuietCard title="Unexplained — read these yourself">
      {items.length === 0 ? (
        <p>The analysis assigned every changed line to a flow or a Not analysed section.</p>
      ) : (
        <>
          <p>
            The analysis did not explain these lines: {plural(lines, "changed line")} in{" "}
            {plural(fileCount(items), "file")} were not assigned to any flow. Read them yourself.
          </p>
          <ul className="m-0 grid list-none gap-3 p-0">
            {items.map((item) => (
              <li key={`${item.side}:${item.file}`} className="grid gap-1">
                <p className="flex flex-wrap gap-2">
                  <code className="font-semibold">{item.file}</code>
                  <span className="text-muted">{rangesText(item)}</span>
                </p>
                {code?.(item)}
              </li>
            ))}
          </ul>
        </>
      )}
    </QuietCard>
  );
}
