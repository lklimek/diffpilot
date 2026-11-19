import type { ReactNode } from "react";
import type { IdenticalMove } from "../../api/generated/types";
import { ProvenanceBadge } from "../badge/Badge";
import { rangeText } from "../section/format";
import { lineCount, plural } from "./labels";
import { Note, QuietCard } from "./QuietCard";

interface MovesBlockProps {
  /** Moves whose text is deterministically identical (`QuietRows.moves`). */
  moves: IdenticalMove[];
  /** Code of a moved block at the head commit; text only without it. */
  code?: (move: IdenticalMove) => ReactNode;
}

/** Quiet row of moved blocks with identical text. */
export function MovesBlock({ moves, code }: MovesBlockProps) {
  if (moves.length === 0) return null;
  return (
    <QuietCard title="Moves with identical text">
      <Note>
        A deterministic check found the text of each block equal to the original after whitespace
        normalisation. Identical text does not mean unchanged behaviour: callers and surrounding code may
        differ. Moves whose text differs are steps of their flows, with ≠ on the differing lines.
      </Note>

      <ul className="m-0 grid list-none gap-3 p-0">
        {moves.map((move) => (
          <li key={`${move.to.file}:${move.to.range.start_line}`} className="grid gap-1">
            <p className="flex flex-wrap items-center gap-2">
              <code className="font-semibold">
                {move.to.file}:{rangeText(move.to.range)}
              </code>
              <ProvenanceBadge badge={move.badge} />
            </p>
            <p>
              Body text identical. Moved from {move.from.file}:{rangeText(move.from.range)} at the base
              commit; {plural(lineCount([move.to.range]), "line")}.
            </p>
            {code?.(move)}
          </li>
        ))}
      </ul>
    </QuietCard>
  );
}
