import type { LineSide } from "../../api/generated/types";
import { GithubIcon } from "../github/GithubMark";
import type { GithubLineLink } from "../github/links";

export type LineGithubLink = (side: LineSide, line: number, inDiff: boolean) => GithubLineLink;

export function LineGithub({
  path,
  line,
  link,
  tabbable,
  onFocus,
}: {
  path: string;
  line: number;
  link: GithubLineLink;
  tabbable: boolean;
  onFocus: () => void;
}) {
  return (
    <a
      // Widen the 16 px mark into the gutter without changing the row or clipping the target at the left edge.
      data-line-github
      className="invisible col-start-1 row-start-1 relative z-10 inline-flex size-4 items-center justify-center justify-self-start rounded group-focus-within/line:visible group-hover/line:visible group-data-[touch-line]/line:visible before:absolute before:-inset-y-1 before:-right-2 before:left-0 hover:bg-accent-bg focus-visible:outline-accent"
      href={link.url}
      target="_blank"
      rel="noopener noreferrer"
      aria-label={`Open line ${line} of ${path} on GitHub, new tab`}
      title={link.tip}
      tabIndex={tabbable ? 0 : -1}
      onFocus={onFocus}
    >
      <GithubIcon className="size-4" />
    </a>
  );
}
