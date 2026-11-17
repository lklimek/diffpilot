import { useMemo } from "react";
import type { DiffHunk, DiffLineKind, DiffResponse } from "../../api/generated/types";
import type { Highlight, Token } from "./highlight";
import { CodeText, useTokens } from "./tokens";

/** Props of the classic diff ("Show old code", inspector section 1). */
export interface ClassicDiffProps {
  /** The diff of a range (`GET …/diff`). */
  diff: DiffResponse;
  /** Syntax colouring, as in the code view. */
  highlight?: Highlight;
}

const KINDS: Record<DiffLineKind, { sign: string; text: string; look: string }> = {
  context: { sign: " ", text: "unchanged", look: "" },
  removed: { sign: "−", text: "removed", look: "bg-rm-bg" },
  added: { sign: "+", text: "added", look: "bg-ad-bg" },
};

function Hunk({ hunk, tokens }: { hunk: DiffHunk; tokens: Token[][] | undefined }) {
  return (
    <>
      <div className="sticky left-0 w-max px-2 text-muted">{hunk.header}</div>
      {hunk.lines.map((l, i) => {
        const kind = KINDS[l.kind];
        return (
          <div
            key={i}
            data-diff-line={l.kind}
            className={`grid min-w-full grid-cols-[5ch_5ch_2ch_max-content] ${kind.look}`}
          >
            <span className="pr-1 text-right text-muted select-none">{l.old_line}</span>
            <span className="pr-1 text-right text-muted select-none">{l.new_line}</span>
            <span className="text-center font-bold select-none">
              <span aria-hidden>{kind.sign}</span>
              <span className="sr-only">{kind.text}</span>
            </span>
            <code className="pr-4 whitespace-pre">
              <CodeText text={l.text} tokens={tokens?.[i]} />
            </code>
          </div>
        );
      })}
    </>
  );
}

/** Classic diff of a range: hunks with old and new line numbers; removed and added lines named for screen readers. */
export function ClassicDiff({ diff, highlight }: ClassicDiffProps) {
  // One highlight call (one budget) for all hunks.
  const fragments = useMemo(() => diff.hunks.map((h) => h.lines.map((l) => l.text)), [diff]);
  const tokens = useTokens(highlight, diff.path, fragments);
  return (
    <div
      role="region"
      aria-label={`Classic diff of ${diff.path}`}
      tabIndex={0}
      className="overflow-auto rounded-md border border-border bg-code-bg py-0.5 font-mono text-[12.5px] leading-[1.55]"
    >
      {diff.hunks.length === 0 && <p className="m-0 px-2 font-sans text-muted">No change in this range.</p>}
      <div className="w-max min-w-full">
        {diff.hunks.map((hunk, i) => (
          <Hunk key={hunk.header} hunk={hunk} tokens={tokens?.[i] ?? undefined} />
        ))}
      </div>
    </div>
  );
}
