import type { MouseEvent, ReactNode } from "react";
import { Tooltip } from "../tooltip/Tooltip";
import type { DepsLinks, LineTarget } from "./model";

// A plain left click stays in the app; a modified or middle click opens the link the browser's way.
export const inApp = (event: MouseEvent, follow: () => void) => {
  if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
  event.preventDefault();
  follow();
};

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
    <Tooltip tip="Opens this line on GitHub at the analysed commit (new tab): the Files view shows only the files this pull request changes.">
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
