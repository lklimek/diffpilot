import { useMemo, type ReactNode } from "react";
import { useNavigate } from "react-router";
import { useFiles, useTour } from "../../api/queries";
import { displaySide, filesHash } from "../../components/files/model";
import { githubLineUrl } from "../../components/github/links";
import { PathLinks, pathMatcher } from "../../components/section/pathLinks";

export function AnalysisPathLinks({
  analysisId,
  head,
  children,
}: {
  analysisId: string | null;
  head?: string | null;
  children: ReactNode;
}) {
  const tour = useTour(analysisId ?? "", analysisId !== null);
  const pr = tour.data?.analysis.pr;
  const matchesHead = !!pr && (head === undefined || head === pr.head_sha);
  // Review screens already read Files; the Comments route needs its own read after checking the head.
  const files = useFiles(analysisId ?? "", analysisId !== null && head !== undefined && matchesHead);
  const navigate = useNavigate();
  const links = useMemo(() => {
    if (analysisId === null || !pr || !files.data || !matchesHead) return null;
    const byPath = new Map(files.data.files.map((file) => [file.path, file]));
    const match = pathMatcher([...byPath.keys()], pr.repo);
    return {
      resolve: (mention: string) => {
        const at = match(mention);
        if (!at) return;
        const file = byPath.get(at.path)!;
        const side = displaySide(file);
        const line = at.line ?? 1;
        const range = at.end === undefined ? at.line : `${line}–${at.end}`;
        const location = range === undefined ? "" : ` at ${at.end === undefined ? "line" : "lines"} ${range}`;
        if (file.not_rendered) {
          const href = githubLineUrl(pr, { path: at.path, side, start: line, end: at.end ?? line }, false);
          return {
            href: at.line === undefined ? href.split("#")[0]! : href,
            tip: `Open ${at.path}${location} on GitHub at the analysed commit (new tab)`,
            github: true,
          };
        }
        return {
          href: `/analyses/${encodeURIComponent(analysisId)}/files${filesHash({ ...at, file: at.path, side })}`,
          tip: `Open ${at.path}${location} in Files`,
        };
      },
      onNavigate: (href: string) => void navigate(href),
    };
  }, [analysisId, files.data, matchesHead, navigate, pr]);
  return <PathLinks.Provider value={links}>{children}</PathLinks.Provider>;
}
