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
      data-line-github
      className="invisible col-start-1 row-start-1 z-10 inline-flex size-6 items-center justify-center justify-self-start rounded group-focus-within/line:visible group-hover/line:visible group-data-[touch-line]/line:visible hover:bg-accent-bg focus-visible:outline-accent"
      href={link.url}
      target="_blank"
      rel="noopener noreferrer"
      aria-label={`Open line ${line} of ${path} on GitHub, new tab`}
      title={link.tip}
      tabIndex={tabbable ? 0 : -1}
      onFocus={onFocus}
    >
      <GithubIcon />
    </a>
  );
}
