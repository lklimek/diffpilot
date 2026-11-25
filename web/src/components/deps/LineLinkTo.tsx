import type { ReactNode } from "react";
import { inApp } from "../inApp";
import { Tooltip } from "../tooltip/Tooltip";
import type { DepsLinks, LineTarget } from "./model";

// Base lines are of the base commit: said, since their numbers differ from the head's.
const lineText = (t: LineTarget) =>
  `${t.side === "base" ? "base " : ""}${t.file}:${t.line}${t.end !== undefined && t.end !== t.line ? `–${t.end}` : ""}`;

/** A line link: the Files view for a file of the pull request, else GitHub at the analysed commit. */
export function LineLinkTo({
  target,
  links,
  children,
}: {
  target: LineTarget;
  links: DepsLinks;
  /** Link text; `file:line` by default. */
  children?: ReactNode;
}) {
  const { href, external } = links.line(target);
  const text = children ?? lineText(target);
  return external ? (
    <Tooltip tip="Opens this line on GitHub at the analysed commit (new tab): the Files view does not show this file's lines (not changed by this pull request, or generated, too large or binary).">
      <a href={href} target="_blank" rel="noopener noreferrer" className="font-mono text-[12.5px]">
        {text}
      </a>
    </Tooltip>
  ) : (
    <Tooltip tip="Opens this line in the Files view.">
      <a
        href={href}
        onClick={(event) => inApp(event, () => links.onNavigate(href))}
        className="font-mono text-[12.5px]"
      >
        {text}
      </a>
    </Tooltip>
  );
}
