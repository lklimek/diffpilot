import { useMemo, type ReactNode } from "react";
import { useNavigate } from "react-router";
import type { PrRef } from "../../api/generated/types";
import { useFiles, useTour } from "../../api/queries";
import { displaySide, filesHash } from "../../components/files/model";
import { githubLineUrl } from "../../components/github/links";
import { PathLinks, pathMatcher } from "../../components/section/pathLinks";

import { analysisPath } from "../pr/paths";

export function AnalysisPathLinks({
  analysisId,
  pr,
  head,
  children,
}: {
  analysisId: string | null;
  pr: PrRef;
  head?: string | null;
  children: ReactNode;
}) {
  const tour = useTour(analysisId ?? "", analysisId !== null);
  const analysedPr = tour.data?.analysis.pr;
  const matchesHead = !!analysedPr && (head === undefined || head === analysedPr.head_sha);
  // Review screens already read Files; the Comments route needs its own read after checking the head.
  const files = useFiles(analysisId ?? "", analysisId !== null && head !== undefined && matchesHead);
  const navigate = useNavigate();
  const filesPath = analysisId === null ? null : analysisPath(pr, analysisId, "files");
  const links = useMemo(() => {
    if (!filesPath || !analysedPr || !files.data || !matchesHead) return null;
    const byPath = new Map(files.data.files.map((file) => [file.path, file]));
    const match = pathMatcher([...byPath.keys()], analysedPr.repo);
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
          const href = githubLineUrl(
            analysedPr,
            { path: at.path, side, start: line, end: at.end ?? line },
            false,
          );
          return {
            href: at.line === undefined ? href.split("#")[0]! : href,
            tip: `Open ${at.path}${location} on GitHub at the analysed commit (new tab)`,
            github: true,
          };
        }
        return {
          href: `${filesPath}${filesHash({ ...at, file: at.path, side })}`,
          tip: `Open ${at.path}${location} in Files`,
        };
      },
      onNavigate: (href: string) => void navigate(href),
    };
  }, [analysedPr, files.data, filesPath, matchesHead, navigate]);
  return <PathLinks.Provider value={links}>{children}</PathLinks.Provider>;
}
